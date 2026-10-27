// POST /v1/messages.
//
// The conversation becomes a prompt on its sigil (conversation.ts), or,
// when it ends with results for tools the bloom is holding for, those
// results, sent as `tendril-result`s on the bloom's own sid, which grows
// on. A result the server no longer holds grows on from the graft instead.
// How the request asks its bloom to grow becomes the prompt's:
// `max_tokens` its season, `temperature`, its `top_p` and `top_k` as
// prune, `stop_sequences` as frost, a thinking budget and
// `output_config.effort` as sepal, `output_config.format` as trellis, and
// `tool_choice` as tendrils.
//
// Without `stream: true` the bloom comes back as one message. With it, it
// comes as Anthropic's events, `message_start` held until the bloom first
// grows, so a bloom that fails before then fails with its status.

import { randomUUID } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { Bloom, Petal, PromptBody, Tone } from "../../../thrum-clients/ts/index.ts";
import { answers, fromMessages, opening, type Message, type Sigils } from "./conversation.ts";
import type { Forager } from "./forager.ts";
import { grewOf, grow, type Block, type Grown, type Listener, type Usage } from "./growth.ts";
import { bad, gone, HttpError, readJson, sendJson, Sse } from "./http.ts";
import { tendrilsOf, toolsFromAnthropic } from "./tools.ts";

export interface Ctx {
  forager: Forager;
  sigils: Sigils;
  audit(entry: Record<string, unknown>): void;
  track(tenant: string, usage: Usage): void;
}

interface Body {
  model?: unknown;
  messages?: unknown;
  system?: unknown;
  max_tokens?: number;
  stream?: boolean;
  tools?: unknown;
  tool_choice?: unknown;
  temperature?: number;
  top_p?: number;
  top_k?: number;
  stop_sequences?: unknown;
  thinking?: { type?: string; budget_tokens?: number };
  output_config?: { effort?: string; format?: { type?: string; schema?: unknown } };
}

function promptOf(sid: string, model: string, content: Petal[], graft: Bloom[], system: string | undefined, b: Body): Tone<"prompt"> {
  const p: PromptBody = { modelId: model, content };
  if (graft.length > 0) p.graft = graft;
  if (system) p.systemPrompt = system;
  const tools = toolsFromAnthropic(b.tools);
  if (tools) p.tools = tools;
  if (typeof b.max_tokens === "number") p.season = b.max_tokens;
  if (typeof b.temperature === "number") p.temperature = b.temperature;
  if (typeof b.top_p === "number" || typeof b.top_k === "number") {
    p.prune = { ...(typeof b.top_p === "number" ? { p: b.top_p } : {}), ...(typeof b.top_k === "number" ? { k: b.top_k } : {}) };
  }
  if (Array.isArray(b.stop_sequences) && b.stop_sequences.length > 0) p.frost = b.stop_sequences.filter((s): s is string => typeof s === "string");
  const budget = b.thinking?.type === "enabled" ? b.thinking.budget_tokens : undefined;
  const effort = b.output_config?.effort;
  if (typeof budget === "number" || typeof effort === "string") {
    p.sepal = { ...(typeof effort === "string" ? { effort } : {}), ...(typeof budget === "number" ? { budget } : {}) };
  }
  if (b.output_config?.format?.type === "json_schema") p.trellis = b.output_config.format.schema ?? "json";
  const tendrils = tendrilsOf(b.tool_choice);
  if (tendrils) p.tendrils = tendrils;
  return { chi: "prompt", sid, ...p };
}

export async function messages(ctx: Ctx, tenant: string, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const body = await readJson<Body>(req);
  if (typeof body.model !== "string" || body.model.length === 0) throw bad("model: a model is required");
  if (!Array.isArray(body.messages) || body.messages.length === 0) throw bad("messages: at least one message is required");
  const model = body.model;
  const said = fromMessages(body.system, body.messages as Message[]);
  if (said.tail === "none") throw bad("messages: there is nothing to answer");
  ctx.forager.ready();

  const given = answers(said);
  const held = given.length > 0 ? ctx.forager.heldSid(given.map((a) => a.callId)) : undefined;
  const { graft, content } = opening(said);
  const sid = held !== undefined && !ctx.sigils.growing(held) ? held : ctx.sigils.continuing(tenant, said.system, graft);
  const continuing = sid === held;
  const stream = body.stream === true;
  ctx.audit({ endpoint: "messages", tenant, model, sid, stream });

  const id = `msg_${randomUUID().replace(/-/g, "")}`;
  const message = (blocks: Block[], stop: string | null, usage: Usage) => ({
    id, type: "message", role: "assistant", model, content: blocks, stop_reason: stop, stop_sequence: null, usage,
  });
  const sse = stream
    ? new Sse(res, () => ["message_start", { type: "message_start", message: message([], null, { input_tokens: 0, output_tokens: 0 }) }])
    : undefined;
  const on: Listener = sse
    ? {
      start: (index, block) => sse.event("content_block_start", { type: "content_block_start", index, content_block: block }),
      delta: (index, delta) => sse.event("content_block_delta", { type: "content_block_delta", index, delta }),
      stop: (index) => sse.event("content_block_stop", { type: "content_block_stop", index }),
    }
    : {};

  ctx.sigils.begin(sid);
  let grown: Grown;
  try {
    grown = await grow(ctx.forager, sid, continuing ? { answers: given } : { prompt: promptOf(sid, model, content, graft, said.system, body) }, (fn) => gone(res, fn), on);
  } catch (e) {
    ctx.sigils.end(sid);
    if (!sse) throw e;
    sse.fail(e instanceof HttpError ? e : new HttpError(500, String(e)));
    return;
  }
  ctx.sigils.end(sid);

  const after = said.blooms.map((b) => ({ content: b.content, grew: [...(b.grew ?? [])] }));
  const last = after[after.length - 1];
  if (last) last.grew = [...last.grew, ...grewOf(grown)];
  ctx.sigils.remember(tenant, said.system, after, sid);
  ctx.track(tenant, grown.usage);

  if (sse) {
    sse.event("message_delta", { type: "message_delta", delta: { stop_reason: grown.stop, stop_sequence: null }, usage: grown.usage });
    sse.event("message_stop", { type: "message_stop" });
    sse.end();
    return;
  }
  sendJson(res, 200, message(grown.content, grown.stop, grown.usage));
}
