// The Responses API: POST /v1/responses, and GET and DELETE of one.
//
// A request's input reads as blooms the way chat messages do, with the
// Responses API's own items: messages, function calls and their outputs,
// reasoning, and the MCP calls of earlier responses. A response that
// follows `previous_response_id`, or that belongs to a `conversation`,
// grows on from the blooms its predecessor left, which this server keeps
// for the caller that made it. A response is kept for that caller to read
// back, unless `store` is false: the latest 2048 of them, and the latest of
// each conversation only while it is among them.
//
// The output is items. The bloom's thinking is a reasoning item, its words
// a message, a tool the bloom reaches the caller for a function call that
// ends the response, and a tool the server ran an `mcp_call` item, its
// output and all. Streaming says each as OpenAI does: the response
// created and in progress, each item added, its parts and deltas, each
// item done, and the response completed, or incomplete when its tokens
// ran out, or failed. Background responses are not grown here.

import { randomUUID } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import { answers, fromResponses, opening, type Said } from "./conversation.ts";
import { grow, mcpCallOf, servedOf, usageOf, type Grown, type Listener } from "./growth.ts";
import { growthOf, modelFor, prompt, type Caller, type Ctx } from "./hive.ts";
import { bad, gone, HttpError, readJson, sendJson, Sse } from "./http.ts";
import { toolsFromOpenAI } from "./tools.ts";

interface Body {
  model?: string;
  input?: unknown;
  instructions?: string;
  previous_response_id?: string;
  conversation?: string | { id?: string } | null;
  store?: boolean;
  stream?: boolean;
  background?: boolean;
  tools?: unknown[];
  tool_choice?: unknown;
  parallel_tool_calls?: boolean;
  text?: { format?: { type?: string; schema?: unknown; name?: string } };
  reasoning?: { effort?: string; summary?: string };
  max_output_tokens?: number;
  temperature?: number;
  top_p?: number;
  metadata?: Record<string, string>;
  truncation?: string;
  user?: string;
  frequency_penalty?: number;
  presence_penalty?: number;
}

export interface Kept {
  label: string;
  sid: string;
  said: Said;
  response: Record<string, unknown>;
}

const KEPT_MOST = 2048;

export class Shelf {
  readonly #most: number;
  readonly #byId = new Map<string, Kept & { thread?: string }>();
  readonly #byConversation = new Map<string, string>();

  constructor(most = KEPT_MOST) {
    this.#most = most;
  }

  get size(): { responses: number; conversations: number } {
    return { responses: this.#byId.size, conversations: this.#byConversation.size };
  }

  get(label: string, id: string): Kept | undefined {
    const k = this.#byId.get(id);
    return k && k.label === label ? k : undefined;
  }

  latest(label: string, conversation: string): Kept | undefined {
    const id = this.#byConversation.get(`${label}\0${conversation}`);
    return id ? this.get(label, id) : undefined;
  }

  put(id: string, kept: Kept, conversation?: string): void {
    const thread = conversation ? `${kept.label}\0${conversation}` : undefined;
    this.#byId.set(id, { ...kept, thread });
    if (thread) this.#byConversation.set(thread, id);
    while (this.#byId.size > this.#most) {
      const [oldest] = this.#byId.keys();
      this.#forget(oldest);
    }
  }

  drop(label: string, id: string): boolean {
    if (!this.get(label, id)) return false;
    this.#forget(id);
    return true;
  }

  #forget(id: string): void {
    const thread = this.#byId.get(id)?.thread;
    this.#byId.delete(id);
    if (thread && this.#byConversation.get(thread) === id) this.#byConversation.delete(thread);
  }
}

const kept = new Shelf();

const short = () => randomUUID().replace(/-/g, "");

export async function responses(ctx: Ctx, who: Caller, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const body = await readJson<Body>(req);
  if (body.background === true) throw bad("background responses are not grown here; ask without background", "background");
  const { rq } = who;
  const model = modelFor(rq, body.model);
  const conversation = typeof body.conversation === "string" ? body.conversation : body.conversation?.id;
  let before: Kept | undefined;
  if (body.previous_response_id) {
    before = kept.get(rq.label, body.previous_response_id);
    if (!before) throw new HttpError(404, `Previous response with id '${body.previous_response_id}' not found.`, "previous_response_not_found", "previous_response_id");
  } else if (conversation) {
    before = kept.latest(rq.label, conversation);
  }
  const said = fromResponses(body.input, body.instructions, before?.said);
  if (said.tail === "none") throw bad("input holds nothing to answer", "input");

  const given = answers(said);
  const held = given.length > 0 ? rq.heldSid(given.map((a) => a.callId)) : undefined;
  const continuing = held !== undefined && !ctx.sigils.growing(held);
  const { graft, content } = opening(said);
  const sid = held !== undefined && continuing ? held : before?.sid && !ctx.sigils.growing(before.sid) ? before.sid : ctx.sigils.continuing(rq.label, said.system, graft, said.served);
  const format = body.text?.format;
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
          max: body.max_output_tokens,
          seed: undefined,
          format: format ? { type: format.type, schema: format.schema } : undefined,
          tool_choice: body.tool_choice,
          parallel_tool_calls: body.parallel_tool_calls,
          effort: body.reasoning?.effort,
          sampling: { frequency_penalty: body.frequency_penalty, presence_penalty: body.presence_penalty },
        }),
      }),
    };

  const id = `resp_${short()}`;
  const createdAt = Math.floor(Date.now() / 1000);
  const shape = (status: string, output: unknown[], extra: Record<string, unknown> = {}) => ({
    id,
    object: "response",
    created_at: createdAt,
    status,
    model: model ?? body.model ?? "hum",
    output,
    instructions: body.instructions ?? null,
    previous_response_id: body.previous_response_id ?? null,
    tools: body.tools ?? [],
    tool_choice: body.tool_choice ?? "auto",
    parallel_tool_calls: body.parallel_tool_calls ?? true,
    temperature: body.temperature ?? null,
    top_p: body.top_p ?? null,
    max_output_tokens: body.max_output_tokens ?? null,
    reasoning: { effort: body.reasoning?.effort ?? null, summary: body.reasoning?.summary ?? null },
    text: { format: format ?? { type: "text" } },
    truncation: body.truncation ?? "disabled",
    metadata: body.metadata ?? {},
    store: body.store !== false,
    user: body.user ?? null,
    error: null,
    incomplete_details: null,
    usage: null,
    ...extra,
  });

  const stream = body.stream === true;
  const sse = stream ? new Sse(res) : undefined;
  let seq = 0;
  const emit = (type: string, data: Record<string, unknown>) => sse?.data({ type, sequence_number: seq++, ...data }, type);
  const output: Array<Record<string, any>> = [];
  let open_: { kind: "reasoning" | "message"; index: number; item: Record<string, any> } | undefined;

  const close = () => {
    if (!open_) return;
    const { kind, index, item } = open_;
    open_ = undefined;
    if (kind === "reasoning") {
      const t = item.summary[0].text as string;
      emit("response.reasoning_summary_text.done", { item_id: item.id, output_index: index, summary_index: 0, text: t });
      emit("response.reasoning_summary_part.done", { item_id: item.id, output_index: index, summary_index: 0, part: { type: "summary_text", text: t } });
    } else {
      const t = item.content[0].text as string;
      emit("response.output_text.done", { item_id: item.id, output_index: index, content_index: 0, text: t, logprobs: [] });
      emit("response.content_part.done", { item_id: item.id, output_index: index, content_index: 0, part: item.content[0] });
    }
    item.status = "completed";
    emit("response.output_item.done", { output_index: index, item });
  };
  const opened = (kind: "reasoning" | "message") => {
    if (open_?.kind === kind) return open_;
    close();
    const index = output.length;
    const item = kind === "reasoning"
      ? { id: `rs_${short()}`, type: "reasoning", summary: [{ type: "summary_text", text: "" }] }
      : { id: `msg_${short()}`, type: "message", role: "assistant", status: "in_progress", content: [{ type: "output_text", text: "", annotations: [], logprobs: [] }] };
    output.push(item);
    emit("response.output_item.added", { output_index: index, item: kind === "reasoning" ? { ...item, summary: [] } : { ...item, content: [] } });
    if (kind === "reasoning") emit("response.reasoning_summary_part.added", { item_id: item.id, output_index: index, summary_index: 0, part: { type: "summary_text", text: "" } });
    else emit("response.content_part.added", { item_id: item.id, output_index: index, content_index: 0, part: { type: "output_text", text: "", annotations: [], logprobs: [] } });
    open_ = { kind, index, item };
    return open_;
  };
  const whole = (item: Record<string, any>, deltas: () => void) => {
    close();
    const index = output.length;
    output.push(item);
    emit("response.output_item.added", { output_index: index, item: { ...item, status: "in_progress", ...(item.type === "function_call" ? { arguments: "" } : { output: null }) } });
    deltas.call(null);
    emit("response.output_item.done", { output_index: index, item });
  };

  const on: Listener = {
    thought: (d) => {
      const o = opened("reasoning");
      o.item.summary[0].text += d;
      emit("response.reasoning_summary_text.delta", { item_id: o.item.id, output_index: o.index, summary_index: 0, delta: d });
    },
    text: (d) => {
      const o = opened("message");
      o.item.content[0].text += d;
      emit("response.output_text.delta", { item_id: o.item.id, output_index: o.index, content_index: 0, delta: d, logprobs: [] });
    },
    call: (c) => {
      const item = { id: `fc_${short()}`, type: "function_call", status: "completed", call_id: c.id, name: c.name, arguments: c.args };
      whole(item, () => {
        emit("response.function_call_arguments.delta", { item_id: item.id, output_index: output.length - 1, delta: c.args });
        emit("response.function_call_arguments.done", { item_id: item.id, output_index: output.length - 1, arguments: c.args });
      });
    },
    served: (s) => {
      whole({ ...mcpCallOf(s), id: s.callId || `mcp_${short()}` }, () => {});
    },
  };

  emit("response.created", { response: shape("in_progress", []) });
  emit("response.in_progress", { response: shape("in_progress", []) });

  ctx.sigils.begin(sid);
  let grown: Grown;
  try {
    grown = await grow(rq, sid, open, { timeoutMs: ctx.config.timeoutMs, gone: (fn) => gone(res, fn) }, on);
  } catch (e) {
    ctx.sigils.end(sid);
    const err = e instanceof HttpError ? e : new HttpError(500, String(e));
    if (sse?.begun) {
      emit("response.failed", { response: shape("failed", output, { error: { code: err.code ?? "server_error", message: err.message } }) });
      sse.end();
    } else if (sse) {
      sse.fail(err);
    } else {
      throw err;
    }
    return;
  }
  ctx.sigils.end(sid);
  close();

  const after = said.blooms.map((b) => ({ content: b.content, grew: [...(b.grew ?? [])] }));
  const last = after[after.length - 1];
  if (last) last.grew = [...(last.grew ?? []), ...grown.grew];
  const served = servedOf(said, grown);
  ctx.sigils.remember(rq.label, said.system, after, sid, served);

  const used = usageOf(grown.nectar);
  ctx.usage.track(who.key, used.prompt, used.completion);
  const usage = {
    input_tokens: used.prompt,
    input_tokens_details: { cached_tokens: used.cached },
    output_tokens: used.completion,
    output_tokens_details: { reasoning_tokens: 0 },
    total_tokens: used.prompt + used.completion,
  };
  const incomplete = grown.finish === "length";
  const final = shape(incomplete ? "incomplete" : "completed", output, {
    usage,
    ...(incomplete ? { incomplete_details: { reason: "max_output_tokens" } } : {}),
  });
  if (body.store !== false) kept.put(id, { label: rq.label, sid, said: { ...said, blooms: after, tail: "assistant", served }, response: final }, conversation);

  if (sse) {
    emit(incomplete ? "response.incomplete" : "response.completed", { response: final });
    sse.end();
    return;
  }
  sendJson(res, 200, final);
}

export function readResponse(who: Caller, id: string, res: ServerResponse): void {
  const k = kept.get(who.rq.label, id);
  if (!k) throw new HttpError(404, `No response found with id '${id}'.`, "response_not_found");
  sendJson(res, 200, k.response);
}

export function dropResponse(who: Caller, id: string, res: ServerResponse): void {
  if (!kept.drop(who.rq.label, id)) throw new HttpError(404, `No response found with id '${id}'.`, "response_not_found");
  sendJson(res, 200, { id, object: "response", deleted: true });
}
