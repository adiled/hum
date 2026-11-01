// POST /v1/chat/completions.
//
// The conversation becomes a prompt on its sigil (conversation.ts), or,
// when it ends with answers to tools the bloom is holding for, those
// answers. The bloom's words stream as `content` deltas and its thinking
// as `reasoning_content`; a tool the bloom reaches the caller for streams
// as `tool_calls` and finishes the turn with `finish_reason: "tool_calls"`.
// Chat completions has no place for a tool the server ran, so it rides on
// the message as `mcp_calls`, each shaped as the Responses API's
// `mcp_call` item, and is read back when the caller sends it. Without
// `stream` the same arrives as one `chat.completion`. Usage comes in a
// last chunk only when `stream_options.include_usage` asks for it, and
// always without stream.

import { randomUUID } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import { answers, fromChat, opening, type ChatMessage } from "./conversation.ts";
import { grow, mcpCallOf, usageOf, type Grown, type Listener } from "./growth.ts";
import { growthOf, modelFor, prompt, type Caller, type Ctx } from "./hive.ts";
import { bad, gone, HttpError, lineOf, readJson, sendJson, Sse } from "./http.ts";
import { toolsFromOpenAI } from "./tools.ts";

interface Body {
  model?: string;
  messages?: ChatMessage[];
  stream?: boolean;
  stream_options?: { include_usage?: boolean };
  tools?: unknown;
  tool_choice?: unknown;
  parallel_tool_calls?: boolean;
  temperature?: number;
  top_p?: number;
  max_tokens?: number;
  max_completion_tokens?: number;
  stop?: string | string[] | null;
  seed?: number;
  n?: number;
  logprobs?: boolean;
  top_logprobs?: number;
  response_format?: { type?: string; json_schema?: { schema?: unknown } };
  reasoning_effort?: string;
  frequency_penalty?: number;
  presence_penalty?: number;
  logit_bias?: Record<string, number>;
}

export async function chat(ctx: Ctx, who: Caller, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const body = await readJson<Body>(req);
  const messages = Array.isArray(body.messages) ? body.messages : [];
  if (messages.length === 0) throw bad("messages is required", "messages");
  if (typeof body.n === "number" && body.n > 1) throw bad("n above 1 is not grown here: one bloom answers each request", "n");
  if (body.logprobs === true || (typeof body.top_logprobs === "number" && body.top_logprobs > 0)) {
    throw bad("logprobs are not grown here", "logprobs");
  }
  const { rq } = who;
  const model = modelFor(rq, body.model);
  const said = fromChat(messages);
  if (said.tail === "none") throw bad("messages holds nothing to answer", "messages");

  const given = answers(said);
  const held = given.length > 0 ? rq.heldSid(given.map((a) => a.callId)) : undefined;
  const continuing = held !== undefined && !ctx.sigils.growing(held);
  const { graft, content } = opening(said);
  const line = lineOf(req);
  const sid = held !== undefined && continuing ? held : line !== undefined ? ctx.sigils.named(rq.label, line) : ctx.sigils.continuing(rq.label, graft);
  const open = continuing
    ? { answers: given }
    : {
      prompt: prompt(sid, model, {
        content,
        graft,
        systemPrompt: said.system,
        tools: toolsFromOpenAI(body.tools),
        ...growthOf({
          temperature: body.temperature,
          top_p: body.top_p,
          max: body.max_completion_tokens ?? body.max_tokens,
          stop: body.stop,
          seed: body.seed,
          format: body.response_format,
          tool_choice: body.tool_choice,
          parallel_tool_calls: body.parallel_tool_calls,
          effort: body.reasoning_effort,
          sampling: { frequency_penalty: body.frequency_penalty, presence_penalty: body.presence_penalty, logit_bias: body.logit_bias },
        }),
      }),
    };

  const id = `chatcmpl-${randomUUID()}`;
  const created = Math.floor(Date.now() / 1000);
  const echoed = model ?? body.model ?? "hum";
  const stream = body.stream === true;
  const sse = stream ? new Sse(res) : undefined;
  const chunk = (delta: Record<string, unknown>, finish: string | null = null) => ({
    id, object: "chat.completion.chunk", created, model: echoed,
    choices: [{ index: 0, delta, logprobs: null, finish_reason: finish }],
  });
  let spoke = false;
  let ran = 0;
  const speak = (delta: Record<string, unknown>) => {
    if (!sse) return;
    if (!spoke) {
      spoke = true;
      sse.data(chunk({ role: "assistant", content: "" }));
    }
    sse.data(chunk(delta));
  };
  const on: Listener = {
    text: (d) => speak({ content: d }),
    thought: (d) => speak({ reasoning_content: d }),
    call: (c) => speak({ tool_calls: [{ index: c.index, id: c.id, type: "function", function: { name: c.name, arguments: c.args } }] }),
    served: (s) => speak({ mcp_calls: [{ index: ran++, ...mcpCallOf(s) }] }),
  };

  ctx.sigils.begin(sid);
  let grown: Grown;
  try {
    grown = await grow(rq, sid, open, { timeoutMs: ctx.config.timeoutMs, gone: (fn) => gone(res, fn) }, on);
  } catch (e) {
    ctx.sigils.end(sid);
    if (sse) sse.fail(e instanceof HttpError ? e : new HttpError(500, String(e)));
    else throw e;
    return;
  }
  ctx.sigils.end(sid);

  const after = said.blooms.map((b) => ({ content: b.content, grew: [...(b.grew ?? [])] }));
  const last = after[after.length - 1];
  if (last) last.grew = [...(last.grew ?? []), ...grown.grew];
  ctx.sigils.remember(rq.label, after, sid, grown.served.map((s) => s.callId));

  const used = usageOf(grown.nectar);
  ctx.usage.track(who.key, used.prompt, used.completion);
  const usage = {
    prompt_tokens: used.prompt,
    completion_tokens: used.completion,
    total_tokens: used.prompt + used.completion,
    prompt_tokens_details: { cached_tokens: used.cached },
  };

  if (sse) {
    if (!spoke) sse.data(chunk({ role: "assistant", content: "" }));
    sse.data(chunk({}, grown.finish));
    if (body.stream_options?.include_usage === true) {
      sse.data({ id, object: "chat.completion.chunk", created, model: echoed, choices: [], usage });
    }
    sse.data("[DONE]");
    sse.end();
    return;
  }
  sendJson(res, 200, {
    id,
    object: "chat.completion",
    created,
    model: echoed,
    choices: [{
      index: 0,
      message: {
        role: "assistant",
        content: grown.text.length > 0 ? grown.text : grown.calls.length > 0 ? null : "",
        refusal: null,
        ...(grown.thought ? { reasoning_content: grown.thought } : {}),
        ...(grown.served.length > 0 ? { mcp_calls: grown.served.map(mcpCallOf) } : {}),
        ...(grown.calls.length > 0
          ? { tool_calls: grown.calls.map((c) => ({ id: c.id, type: "function", function: { name: c.name, arguments: c.args } })) }
          : {}),
      },
      logprobs: null,
      finish_reason: grown.finish,
    }],
    usage,
  });
}
