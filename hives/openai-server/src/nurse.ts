// The nurse half: blooms grown by the OpenAI-compatible API upstream.
//
// The nurse keeps each sigil's conversation as OpenAI messages, its own
// replies among them, since they hold the calls the server ran that a
// chat client may never send back. A prompt whose graft holds blooms the
// nurse did not grow, or a sigil it has never seen, starts the
// conversation over from the graft. Tool answers a caller adds to the last
// bloom join the kept conversation by call id, which the caller was given
// as the upstream gave it. The prompt's words, images, audio and files
// are the next user message, and how the bloom grows becomes the
// request's: `temperature`, `top_p`, `max_completion_tokens`, `stop`,
// `seed`, `response_format`, `tool_choice`, `parallel_tool_calls`,
// `reasoning_effort`, and the penalties under this hive's `ext`. The
// prompt's tools are the request's function tools.
//
// The reply streams back as petals: its words as text, its reasoning as a
// sepal. Tool calls close its petals as tendrils and reach, every one, and
// the bloom grows on once every reach has come back, each answer told to
// the forager as `tendril-info`. A bloom wilts with the reason the model
// stopped, `max_tokens` when it ran out, and its nectar.
//
// A prompt with no words grows on from its graft: a graft that is plain
// text, grown by no one's prompt, is completed through `/completions`,
// its scion the suffix; any other is the conversation so far, asked again.
// A cancel aborts the request upstream and wilts the bloom as `cancel`.

import { randomUUID } from "node:crypto";
import { Bee, Chi, PetalKind, Role, type Bloom, type Petal, type PetalKindT, type Tone } from "../../../thrum-clients/ts/index.ts";
import { HIVE } from "./config.ts";
import { SOURCE, VERSION } from "./requester.ts";
import { events, post } from "./upstream.ts";

const SIGILS_MOST = 512;

type Message = Record<string, any>;

interface Sigil {
  messages: Message[];
  grown: number;
  model: string;
  system?: string;
  ask: Record<string, unknown>;
  tools?: Array<Record<string, unknown>>;
  waiting: Map<string, { name: string; args: unknown }>;
  aborter?: AbortController;
  petal: number;
  open: boolean;
}

function parts(petals: Petal[]): string | Array<Record<string, unknown>> {
  if (petals.every((p) => p.kind === PetalKind.text)) return petals.map((p) => p.text ?? "").join("\n");
  const out: Array<Record<string, unknown>> = [];
  for (const p of petals) {
    if (p.kind === PetalKind.text && p.text) out.push({ type: "text", text: p.text });
    if (p.kind !== PetalKind.pollen || !p.grain) continue;
    const g = p.grain;
    const url = g.url ?? (g.data ? `data:${g.mediaType};base64,${g.data}` : undefined);
    if (g.kind === "image" && url) out.push({ type: "image_url", image_url: { url } });
    else if (g.kind === "audio" && g.data) out.push({ type: "input_audio", input_audio: { data: g.data, format: g.mediaType.split("/")[1] ?? "wav" } });
    else if (g.kind === "file" && url) out.push({ type: "file", file: { file_data: url } });
  }
  return out;
}

function said(petals: Petal[]): Message | undefined {
  const content = parts(petals);
  return (typeof content === "string" ? content.length > 0 : content.length > 0) ? { role: "user", content } : undefined;
}

function grown(petals: Petal[], into: Message[]): void {
  let text = "";
  let calls: Message[] = [];
  const flush = () => {
    if (!text && calls.length === 0) return;
    into.push({ role: "assistant", content: text || null, ...(calls.length > 0 ? { tool_calls: calls } : {}) });
    text = "";
    calls = [];
  };
  for (const p of petals) {
    if (p.kind === PetalKind.text) {
      if (calls.length > 0) flush();
      text += p.text ?? "";
    } else if (p.kind === PetalKind.tendril) {
      calls.push({ id: p.callId ?? "", type: "function", function: { name: p.name ?? "", arguments: typeof p.args === "string" ? p.args : JSON.stringify(p.args ?? {}) } });
    } else if (p.kind === PetalKind.grasp) {
      flush();
      into.push({ role: "tool", tool_call_id: p.callId ?? "", content: typeof p.result === "string" ? p.result : JSON.stringify(p.result ?? "") });
    }
  }
  flush();
}

export function messagesOf(graft: Bloom[]): Message[] {
  const out: Message[] = [];
  for (const b of graft) {
    const m = said(b.content ?? []);
    if (m) out.push(m);
    grown(b.grew ?? [], out);
  }
  return out;
}

export function lateAnswers(known: Message[], bloom: Bloom | undefined): Message[] {
  const answered = new Set(known.filter((m) => m.role === "tool").map((m) => m.tool_call_id));
  const called = new Set(known.flatMap((m) => (m.tool_calls ?? []).map((c: Message) => c.id)));
  const grew = bloom?.grew ?? [];
  const callable = (id: string) => called.has(id) || grew.some((p) => p.kind === PetalKind.tendril && p.callId === id);
  const late = new Set(grew.filter((p) => p.kind === PetalKind.grasp && !answered.has(p.callId) && callable(p.callId ?? "")).map((p) => p.callId));
  const out: Message[] = [];
  grown(grew.filter((p) => late.has(p.callId) && (p.kind === PetalKind.grasp || (p.kind === PetalKind.tendril && !called.has(p.callId)))), out);
  return out;
}

export function plainText(graft: Bloom[]): string | undefined {
  if (graft.length === 0) return undefined;
  const plain = graft.every((b) => (b.content ?? []).length === 0 && (b.grew ?? []).every((p) => p.kind === PetalKind.text));
  return plain ? graft.map((b) => (b.grew ?? []).map((p) => p.text ?? "").join("")).join("") : undefined;
}

export function askOf(t: Tone<"prompt">): Record<string, unknown> {
  const ask: Record<string, unknown> = {};
  if (typeof t.temperature === "number") ask.temperature = t.temperature;
  if (typeof t.prune?.p === "number") ask.top_p = t.prune.p;
  if (typeof t.season === "number") ask.max_completion_tokens = t.season;
  if (t.frost && t.frost.length > 0) ask.stop = t.frost;
  if (typeof t.seed === "number") ask.seed = t.seed;
  if (t.trellis === "json") ask.response_format = { type: "json_object" };
  else if (t.trellis && typeof t.trellis === "object") ask.response_format = { type: "json_schema", json_schema: { name: "trellis", schema: t.trellis } };
  const reach = t.tendrils?.reach;
  if (reach) ask.tool_choice = ["auto", "none", "required"].includes(reach) ? reach : { type: "function", function: { name: reach } };
  if (typeof t.tendrils?.parallel === "boolean") ask.parallel_tool_calls = t.tendrils.parallel;
  if (t.sepal?.effort) ask.reasoning_effort = t.sepal.effort;
  const sampling = t.ext?.[HIVE]?.sampling;
  if (sampling && typeof sampling === "object") Object.assign(ask, sampling);
  return ask;
}

function reasonOf(finish: string | undefined): string {
  if (finish === "length") return "max_tokens";
  if (finish === "content_filter") return "refusal";
  return "stop";
}

function nectarOf(usage: any): Record<string, number> {
  const cached = usage?.prompt_tokens_details?.cached_tokens ?? 0;
  return {
    input_tokens: Math.max(0, (usage?.prompt_tokens ?? 0) - cached),
    cache_read_input_tokens: cached,
    output_tokens: usage?.completion_tokens ?? 0,
  };
}

export class Nurse {
  readonly bee: Bee;
  #models: string[] = [];
  readonly #sigils = new Map<string, Sigil>();

  constructor(hum?: string, bind?: { host: string; port: number; scheme: string }) {
    this.bee = new Bee({
      hive: HIVE,
      as: "main",
      role: Role.nurse,
      hum,
      hello: {
        version: VERSION,
        models: [],
        propensity: { statefulness: "stateless_per_call", wire: "openai/chat-completions" },
        chis: ["hello", "prompt", "cancel", "cleanup", "tendril-result", "unfurl", "wilt", "tendril-reach", "tendril-info", "error"],
        source: SOURCE,
        ...(bind ? { bind } : {}),
      },
    });
    this.bee.on(Chi.prompt, (t) => this.#prompted(t).catch((e) => this.#fail(t.sid ?? "", String(e))));
    this.bee.on(Chi.tendrilResult, (t) => this.#answered(t).catch((e) => this.#fail(t.sid ?? "", String(e))));
    this.bee.on(Chi.cancel, (t) => this.#cancel(t.sid ?? ""));
    this.bee.on(Chi.cleanup, (t) => this.#sigils.delete(t.sid ?? ""));
  }

  serve(models: string[]): void {
    this.#models = models;
    this.bee.hello({ models });
  }

  #send(tone: Tone): void {
    this.bee.send(tone);
  }

  #fail(sid: string, message: string, code = "upstream_error"): void {
    const s = this.#sigils.get(sid);
    if (s) {
      s.open = false;
      s.waiting.clear();
    }
    if (sid) this.#send({ chi: "error", sid, code, message });
  }

  #sigil(sid: string, graft: Bloom[]): Sigil {
    const known = this.#sigils.get(sid);
    if (known && (graft.length === 0 || known.grown === graft.length)) {
      known.messages.push(...lateAnswers(known.messages, graft[graft.length - 1]));
      this.#sigils.delete(sid);
      this.#sigils.set(sid, known);
      return known;
    }
    const fresh: Sigil = { messages: messagesOf(graft), grown: graft.length, model: "", ask: {}, waiting: new Map(), petal: 0, open: false };
    this.#sigils.delete(sid);
    this.#sigils.set(sid, fresh);
    while (this.#sigils.size > SIGILS_MOST) {
      const [oldest] = this.#sigils.keys();
      this.#sigils.delete(oldest);
    }
    return fresh;
  }

  async #prompted(t: Tone<"prompt">): Promise<void> {
    const sid = t.sid ?? "";
    if (!sid) return;
    const model = t.modelId || this.#models[0] || "";
    if (!this.#models.includes(model)) return this.#fail(sid, `this nurse does not grow ${model || "any model"}`, "no-nurse");
    const graft = t.graft ?? [];
    const s = this.#sigil(sid, graft);
    s.aborter?.abort();
    s.model = model;
    s.system = t.systemPrompt ?? s.system;
    s.ask = askOf(t);
    s.tools = (t.tools as Array<{ name: string; description?: string; inputSchema?: unknown }> | undefined)?.map((tool) => ({
      type: "function",
      function: { name: tool.name, ...(tool.description ? { description: tool.description } : {}), parameters: tool.inputSchema ?? { type: "object", properties: {} } },
    }));
    s.petal = 0;
    s.open = true;
    s.waiting.clear();
    const content = t.content && t.content.length > 0 ? t.content : t.text ? [{ kind: PetalKind.text, text: t.text }] : [];
    const asked = said(content);
    if (asked) {
      s.messages.push(asked);
      return this.#grow(sid, s);
    }
    const text = plainText(graft);
    if (text !== undefined) return this.#complete(sid, s, text, t.scion);
    if (s.messages.length === 0) return this.#fail(sid, "a prompt with no words grows on from its graft, and this one carries none", "empty-prompt");
    return this.#grow(sid, s);
  }

  async #answered(t: Tone<"tendril-result">): Promise<void> {
    const sid = t.sid ?? "";
    const s = this.#sigils.get(sid);
    const call = s?.waiting.get(t.callId);
    if (!s || !call) return;
    s.waiting.delete(t.callId);
    const output = typeof t.output === "string" ? t.output : typeof t.result === "string" ? t.result : JSON.stringify(t.result ?? "");
    s.messages.push({ role: "tool", tool_call_id: t.callId, content: output });
    this.#send({ chi: "tendril-info", sid, callId: t.callId, name: call.name, args: call.args, result: output, isError: t.isError === true });
    if (s.waiting.size === 0 && s.open) await this.#grow(sid, s);
  }

  #cancel(sid: string): void {
    const s = this.#sigils.get(sid);
    if (!s || !s.open) return;
    s.aborter?.abort();
    s.open = false;
    s.waiting.clear();
    this.#send({ chi: "wilt", sid, reason: "cancel" });
  }

  #unfurl(sid: string, s: Sigil, opened: Map<string, number>, kind: PetalKindT, grew: Partial<Tone<"unfurl">>): number {
    let petal = opened.get(kind === PetalKind.tendril ? `tendril:${grew.callId}` : kind);
    if (petal === undefined) {
      petal = s.petal++;
      opened.set(kind === PetalKind.tendril ? `tendril:${grew.callId}` : kind, petal);
    }
    this.#send({ chi: "unfurl", sid, petal, kind, ...grew });
    return petal;
  }

  async #grow(sid: string, s: Sigil): Promise<void> {
    const aborter = new AbortController();
    s.aborter = aborter;
    const body = {
      model: s.model,
      messages: [...(s.system ? [{ role: "system", content: s.system }] : []), ...s.messages],
      stream: true,
      stream_options: { include_usage: true },
      ...(s.tools && s.tools.length > 0 ? { tools: s.tools } : {}),
      ...s.ask,
    };
    if (!body.tools) {
      delete (body as Record<string, unknown>).tool_choice;
      delete (body as Record<string, unknown>).parallel_tool_calls;
    }
    let resp: Response;
    try {
      resp = await post("/chat/completions", body, aborter.signal);
    } catch (e) {
      if (aborter.signal.aborted) return;
      return this.#fail(sid, `the API upstream did not answer: ${(e as Error).message}`);
    }
    if (!resp.ok) return this.#fail(sid, `the API upstream answered ${resp.status}: ${(await resp.text().catch(() => "")).slice(0, 500)}`);
    const opened = new Map<string, number>();
    const calls = new Map<number, { id: string; name: string; args: string }>();
    let text = "";
    let finish: string | undefined;
    let usage: unknown;
    try {
      for await (const ev of events(resp)) {
        if (aborter.signal.aborted) return;
        if (ev?.error) return this.#fail(sid, `the API upstream broke off: ${ev.error.message ?? JSON.stringify(ev.error)}`);
        if (ev?.usage) usage = ev.usage;
        const choice = ev?.choices?.[0];
        if (!choice) continue;
        const d = choice.delta ?? {};
        const thought = d.reasoning_content ?? d.reasoning;
        if (typeof thought === "string" && thought) this.#unfurl(sid, s, opened, PetalKind.sepal, { text: thought });
        if (typeof d.content === "string" && d.content) {
          text += d.content;
          this.#unfurl(sid, s, opened, PetalKind.text, { text: d.content });
        }
        for (const tc of d.tool_calls ?? []) {
          const at = tc.index ?? 0;
          const c = calls.get(at) ?? { id: "", name: "", args: "" };
          if (tc.id) c.id = tc.id;
          if (tc.function?.name) c.name += tc.function.name;
          if (tc.function?.arguments) c.args += tc.function.arguments;
          calls.set(at, c);
        }
        if (choice.finish_reason) finish = choice.finish_reason;
      }
    } catch (e) {
      if (aborter.signal.aborted) return;
      return this.#fail(sid, `the API upstream broke off: ${(e as Error).message}`);
    }
    if (aborter.signal.aborted || !s.open) return;
    for (const [kind, petal] of opened) {
      if (!kind.startsWith("tendril:")) this.#send({ chi: "unfurl", sid, petal, kind: kind as PetalKindT, closed: true });
    }
    if (calls.size > 0) {
      const reached = [...calls.values()].map((c) => ({ ...c, id: c.id || `call_${randomUUID().replace(/-/g, "").slice(0, 24)}` }));
      s.messages.push({
        role: "assistant",
        content: text || null,
        tool_calls: reached.map((c) => ({ id: c.id, type: "function", function: { name: c.name, arguments: c.args } })),
      });
      for (const c of reached) {
        let args: unknown = c.args;
        try {
          args = JSON.parse(c.args || "{}");
        } catch {}
        s.waiting.set(c.id, { name: c.name, args });
        const petal = this.#unfurl(sid, s, opened, PetalKind.tendril, { callId: c.id, name: c.name, sap: c.args });
        this.#send({ chi: "unfurl", sid, petal, kind: PetalKind.tendril, closed: true });
      }
      for (const c of reached) this.#send({ chi: "tendril-reach", sid, callId: c.id, name: c.name, args: s.waiting.get(c.id)?.args ?? {} });
      return;
    }
    s.messages.push({ role: "assistant", content: text });
    s.grown += 1;
    s.open = false;
    this.#send({ chi: "wilt", sid, reason: reasonOf(finish), nectar: nectarOf(usage) });
  }

  async #complete(sid: string, s: Sigil, text: string, scion?: string): Promise<void> {
    const aborter = new AbortController();
    s.aborter = aborter;
    const ask = { ...s.ask } as Record<string, unknown>;
    const max = ask.max_completion_tokens;
    delete ask.max_completion_tokens;
    delete ask.response_format;
    delete ask.tool_choice;
    delete ask.parallel_tool_calls;
    delete ask.reasoning_effort;
    let resp: Response;
    try {
      resp = await post("/completions", {
        model: s.model,
        prompt: text,
        ...(scion ? { suffix: scion } : {}),
        max_tokens: typeof max === "number" ? max : 16,
        stream: true,
        stream_options: { include_usage: true },
        ...ask,
      }, aborter.signal);
    } catch (e) {
      if (aborter.signal.aborted) return;
      return this.#fail(sid, `the API upstream did not answer: ${(e as Error).message}`);
    }
    if (!resp.ok) return this.#fail(sid, `the API upstream answered ${resp.status}: ${(await resp.text().catch(() => "")).slice(0, 500)}`, "cannot-grow-on");
    let finish: string | undefined;
    let usage: unknown;
    let opened = false;
    try {
      for await (const ev of events(resp)) {
        if (aborter.signal.aborted) return;
        if (ev?.usage) usage = ev.usage;
        const choice = ev?.choices?.[0];
        if (!choice) continue;
        if (typeof choice.text === "string" && choice.text) {
          opened = true;
          this.#send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.text, text: choice.text });
        }
        if (choice.finish_reason) finish = choice.finish_reason;
      }
    } catch (e) {
      if (aborter.signal.aborted) return;
      return this.#fail(sid, `the API upstream broke off: ${(e as Error).message}`);
    }
    if (aborter.signal.aborted || !s.open) return;
    if (opened) this.#send({ chi: "unfurl", sid, petal: 0, kind: PetalKind.text, closed: true });
    s.open = false;
    s.grown += 1;
    this.#send({ chi: "wilt", sid, reason: reasonOf(finish), nectar: nectarOf(usage) });
  }
}
