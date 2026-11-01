// POST /v1/completions: text grown on from as it stands.
//
// Each prompt is a bloom of its own sigil that no one prompted: a graft of
// one bloom whose growth is the prompt's text, and no words of its own, so
// the nurse grows on from where the text stops. A `suffix` is the bloom's
// scion, the text it grows to meet, for filling in the middle. Several
// prompts grow side by side, one choice each. `max_tokens` is 16 unless
// said, as OpenAI has it; `echo` puts the prompt before what grew. Token
// prompts, logprobs, and more than one choice a prompt are not grown here.

import { randomUUID } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import { grow, usageOf, type Grown } from "./growth.ts";
import { growthOf, modelFor, prompt, type Caller, type Ctx } from "./hive.ts";
import { bad, gone, HttpError, readJson, sendJson, Sse } from "./http.ts";

interface Body {
  model?: string;
  prompt?: unknown;
  suffix?: string | null;
  max_tokens?: number | null;
  temperature?: number;
  top_p?: number;
  n?: number;
  best_of?: number;
  stream?: boolean;
  stream_options?: { include_usage?: boolean };
  stop?: string | string[] | null;
  echo?: boolean;
  logprobs?: number | null;
  seed?: number;
  frequency_penalty?: number;
  presence_penalty?: number;
  logit_bias?: Record<string, number>;
}

function promptsOf(p: unknown): string[] {
  if (p === undefined || p === null) return [""];
  if (typeof p === "string") return [p];
  if (Array.isArray(p) && p.every((s) => typeof s === "string")) return p.length > 0 ? (p as string[]) : [""];
  throw bad("prompt is text, or a list of texts; token prompts are not grown here", "prompt");
}

export async function completions(ctx: Ctx, who: Caller, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const body = await readJson<Body>(req);
  if ((body.n ?? 1) > 1 || (body.best_of ?? 1) > 1) throw bad("one choice a prompt is grown here", "n");
  if (typeof body.logprobs === "number") throw bad("logprobs are not grown here", "logprobs");
  const { rq } = who;
  const model = modelFor(rq, body.model);
  const texts = promptsOf(body.prompt);
  const growth = growthOf({
    temperature: body.temperature,
    top_p: body.top_p,
    max: body.max_tokens ?? 16,
    stop: body.stop,
    seed: body.seed,
    sampling: { frequency_penalty: body.frequency_penalty, presence_penalty: body.presence_penalty, logit_bias: body.logit_bias },
  });

  const id = `cmpl-${randomUUID()}`;
  const created = Math.floor(Date.now() / 1000);
  const echoed = model ?? body.model ?? "hum";
  const sse = body.stream === true ? new Sse(res) : undefined;
  const chunk = (index: number, text: string, finish: string | null = null) => ({
    id, object: "text_completion", created, model: echoed,
    choices: [{ text, index, logprobs: null, finish_reason: finish }],
  });
  const leaving: Array<() => void> = [];
  gone(res, () => leaving.forEach((fn) => fn()));

  const grownAll = await Promise.allSettled(texts.map(async (text, index) => {
    const sid = ctx.sigils.fresh();
    if (body.echo && sse) sse.data(chunk(index, text));
    try {
      const grown = await grow(rq, sid, {
        prompt: prompt(sid, model, {
          content: [],
          graft: [{ grew: [{ kind: "text", text }] }],
          ...(body.suffix ? { scion: body.suffix } : {}),
          ...growth,
        }),
      }, { timeoutMs: ctx.config.timeoutMs, gone: (fn) => leaving.push(fn) }, {
        text: (d) => sse?.data(chunk(index, d)),
      });
      sse?.data(chunk(index, "", grown.finish));
      return grown;
    } finally {
      rq.send({ chi: "cleanup", sid });
    }
  }));

  const failed = grownAll.find((g): g is PromiseRejectedResult => g.status === "rejected");
  if (failed) {
    const err = failed.reason instanceof HttpError ? failed.reason : new HttpError(500, String(failed.reason));
    if (sse) sse.fail(err);
    else throw err;
    return;
  }
  const grown = grownAll.map((g) => (g as PromiseFulfilledResult<Grown>).value);
  let prompt_tokens = 0;
  let completion_tokens = 0;
  for (const g of grown) {
    const used = usageOf(g.nectar);
    prompt_tokens += used.prompt;
    completion_tokens += used.completion;
  }
  ctx.usage.track(who.key, prompt_tokens, completion_tokens);
  const usage = { prompt_tokens, completion_tokens, total_tokens: prompt_tokens + completion_tokens };
  if (sse) {
    if (body.stream_options?.include_usage === true) sse.data({ id, object: "text_completion", created, model: echoed, choices: [], usage });
    sse.data("[DONE]");
    sse.end();
    return;
  }
  sendJson(res, 200, {
    id,
    object: "text_completion",
    created,
    model: echoed,
    choices: grown.map((g, index) => ({
      text: (body.echo ? texts[index] : "") + g.text,
      index,
      logprobs: null,
      finish_reason: g.finish === "tool_calls" ? "stop" : g.finish,
    })),
    usage,
  });
}
