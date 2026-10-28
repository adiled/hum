// An OpenAI conversation, as the blooms of a sigil.
//
// OpenAI's APIs send the whole conversation every time; hum keeps it in a
// sigil, one bloom a turn. A conversation reads as blooms: each user turn
// opens one, its words, images, audio and files its content, and what the
// assistant said, the tools it called and what came back from them are
// what grew in it. System and developer words are the system prompt.
//
// A request's last bloom is what it asks for. Its words are the prompt's
// content and the blooms before it the graft. A conversation that ends
// with tool answers or with the assistant's own words asks for a bloom to
// grow on: the prompt has no content, and every bloom is its graft.
//
// A conversation keeps one sigil while it only grows. The sid a request
// continues is the one its blooms so far were last grown in, by this
// caller; a conversation no one has seen, one whose history was edited,
// or one already growing elsewhere gets a sigil of its own. The trace that
// finds it is the caller's label, the system prompt, the words of every
// turn and the ids of every tool call and answer, which a client sends
// back as it was given them. A turn's words, calls and answers are each
// read in order but apart from one another, its words as one, whitespace
// aside: a client may keep a turn as one message of all its words and
// then all its calls, where the turn grew them in steps between.

import { createHash, randomUUID } from "node:crypto";
import { PetalKind, type Bloom, type Grain, type Petal } from "../../../thrum-clients/ts/index.ts";

export type Tail = "user" | "tool" | "assistant" | "none";

export interface Said {
  system?: string;
  blooms: Bloom[];
  tail: Tail;
}

export interface Answer {
  callId: string;
  result: string;
  isError?: boolean;
}

const DATA_URI = /^data:([^;,]+)?(;base64)?,(.*)$/s;

export function grainOf(url: string, kind: string, fallbackType: string): Grain {
  const m = url.match(DATA_URI);
  if (m) return { kind, mediaType: m[1] || fallbackType, data: m[2] ? m[3] : Buffer.from(decodeURIComponent(m[3])).toString("base64") };
  return { kind, mediaType: fallbackType, url };
}

const text = (t: string): Petal => ({ kind: PetalKind.text, text: t });
const pollen = (grain: Grain): Petal => ({ kind: PetalKind.pollen, grain });

function argsOf(raw: unknown): unknown {
  if (typeof raw !== "string") return raw ?? {};
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

function resultOf(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((p) => (typeof p === "string" ? p : typeof p?.text === "string" ? p.text : ""))
      .join("");
  }
  return content === undefined || content === null ? "" : JSON.stringify(content);
}

function partsOf(content: unknown): Petal[] {
  if (typeof content === "string") return content.length > 0 ? [text(content)] : [];
  if (!Array.isArray(content)) return [];
  const out: Petal[] = [];
  for (const part of content as Array<Record<string, any>>) {
    switch (part?.type) {
      case "text":
      case "input_text":
      case "output_text":
        if (typeof part.text === "string") out.push(text(part.text));
        break;
      case "refusal":
        if (typeof part.refusal === "string") out.push({ kind: PetalKind.thorn, text: part.refusal });
        break;
      case "image_url":
      case "input_image": {
        const url = typeof part.image_url === "string" ? part.image_url : part.image_url?.url;
        if (typeof url === "string") out.push(pollen(grainOf(url, "image", "image/*")));
        break;
      }
      case "input_audio":
        if (typeof part.input_audio?.data === "string") {
          out.push(pollen({ kind: "audio", mediaType: `audio/${part.input_audio.format ?? "wav"}`, data: part.input_audio.data }));
        }
        break;
      case "file":
      case "input_file": {
        const f = part.type === "file" ? part.file ?? {} : part;
        const data = f.file_data;
        if (typeof data === "string") out.push(pollen(grainOf(data.startsWith("data:") ? data : `data:application/octet-stream;base64,${data}`, "file", "application/octet-stream")));
        else if (typeof f.file_url === "string") out.push(pollen(grainOf(f.file_url, "file", "application/octet-stream")));
        break;
      }
    }
  }
  return out;
}

class Reader {
  system: string[] = [];
  blooms: Bloom[] = [];
  tail: Tail = "none";

  #open(): Bloom {
    let last = this.blooms[this.blooms.length - 1];
    if (!last) {
      last = { content: [], grew: [] };
      this.blooms.push(last);
    }
    return last;
  }

  said(petals: Petal[]): void {
    const last = this.blooms[this.blooms.length - 1];
    if (last && (last.grew ?? []).length === 0) last.content = [...(last.content ?? []), ...petals];
    else this.blooms.push({ content: petals, grew: [] });
    this.tail = "user";
  }

  grew(petals: Petal[], tail: Tail): void {
    const bloom = this.#open();
    bloom.grew = [...(bloom.grew ?? []), ...petals];
    this.tail = tail;
  }

  done(): Said {
    const system = this.system.filter((s) => s.length > 0).join("\n\n");
    return { system: system || undefined, blooms: this.blooms, tail: this.tail };
  }
}

export interface ChatMessage {
  role: string;
  content?: unknown;
  name?: string;
  tool_calls?: Array<{ id?: string; type?: string; function?: { name?: string; arguments?: unknown } }>;
  function_call?: { name?: string; arguments?: unknown };
  tool_call_id?: string;
  reasoning_content?: unknown;
  reasoning?: unknown;
}

export function fromChat(messages: ChatMessage[]): Said {
  const r = new Reader();
  for (const m of messages) {
    switch (m.role) {
      case "system":
      case "developer":
        r.system.push(resultOf(m.content));
        break;
      case "user":
        r.said(partsOf(m.content));
        break;
      case "assistant": {
        const petals: Petal[] = [];
        const thought = typeof m.reasoning_content === "string" ? m.reasoning_content : typeof m.reasoning === "string" ? m.reasoning : "";
        if (thought) petals.push({ kind: PetalKind.sepal, text: thought });
        petals.push(...partsOf(m.content));
        for (const c of m.tool_calls ?? []) {
          petals.push({ kind: PetalKind.tendril, callId: c.id ?? "", name: c.function?.name ?? "", args: argsOf(c.function?.arguments) });
        }
        if (m.function_call) {
          petals.push({ kind: PetalKind.tendril, callId: m.function_call.name ?? "", name: m.function_call.name ?? "", args: argsOf(m.function_call.arguments) });
        }
        r.grew(petals, "assistant");
        break;
      }
      case "tool":
        r.grew([{ kind: PetalKind.grasp, callId: m.tool_call_id ?? "", result: resultOf(m.content) }], "tool");
        break;
      case "function":
        r.grew([{ kind: PetalKind.grasp, callId: m.name ?? "", result: resultOf(m.content) }], "tool");
        break;
    }
  }
  return r.done();
}

export type ResponsesItem = Record<string, any>;

export function fromResponses(input: unknown, instructions?: string, before?: Said): Said {
  const r = new Reader();
  if (before) {
    if (before.system) r.system.push(before.system);
    r.blooms = before.blooms.map((b) => ({ content: [...(b.content ?? [])], grew: [...(b.grew ?? [])] }));
    r.tail = before.tail;
  }
  if (typeof instructions === "string" && instructions.length > 0) r.system = [instructions];
  const items: ResponsesItem[] = typeof input === "string" ? [{ type: "message", role: "user", content: input }] : Array.isArray(input) ? input : [];
  for (const item of items) {
    const type = item?.type ?? (item?.role ? "message" : undefined);
    switch (type) {
      case "message":
        if (item.role === "system" || item.role === "developer") r.system.push(resultOf(item.content));
        else if (item.role === "assistant") r.grew(partsOf(item.content), "assistant");
        else r.said(partsOf(item.content));
        break;
      case "function_call":
        r.grew([{ kind: PetalKind.tendril, callId: item.call_id ?? "", name: item.name ?? "", args: argsOf(item.arguments) }], "assistant");
        break;
      case "function_call_output":
        r.grew([{ kind: PetalKind.grasp, callId: item.call_id ?? "", result: resultOf(item.output) }], "tool");
        break;
      case "reasoning": {
        const thought = [...(item.summary ?? []), ...(item.content ?? [])].map((s: any) => s?.text ?? "").join("");
        if (thought) r.grew([{ kind: PetalKind.sepal, text: thought }], r.tail === "none" ? "assistant" : r.tail);
        break;
      }
      case "mcp_call":
        r.grew([
          { kind: PetalKind.tendril, callId: item.id ?? "", name: item.name ?? "", args: argsOf(item.arguments) },
          { kind: PetalKind.grasp, callId: item.id ?? "", result: resultOf(item.output ?? item.error), isError: Boolean(item.error) },
        ], "assistant");
        break;
    }
  }
  return r.done();
}

export function opening(said: Said): { graft: Bloom[]; content: Petal[] } {
  if (said.tail === "user") {
    const last = said.blooms[said.blooms.length - 1];
    return { graft: said.blooms.slice(0, -1), content: last?.content ?? [] };
  }
  return { graft: said.blooms, content: [] };
}

export function answers(said: Said): Answer[] {
  if (said.tail !== "tool") return [];
  const grew = said.blooms[said.blooms.length - 1]?.grew ?? [];
  const out: Answer[] = [];
  for (let i = grew.length - 1; i >= 0 && grew[i].kind === PetalKind.grasp; i--) {
    out.unshift({ callId: grew[i].callId ?? "", result: typeof grew[i].result === "string" ? (grew[i].result as string) : JSON.stringify(grew[i].result ?? ""), isError: grew[i].isError });
  }
  return out;
}

function squeeze(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}

function unspaced(s: string): string {
  return s.replace(/\s+/g, "");
}

export function trace(label: string, system: string | undefined, blooms: Bloom[]): string {
  const h = createHash("sha256");
  h.update(`${label}\0${squeeze(system ?? "")}\0`);
  for (const b of blooms) {
    h.update("U");
    for (const p of b.content ?? []) {
      if (p.kind === PetalKind.text) h.update(`t${squeeze(p.text ?? "")}\0`);
      else if (p.kind === PetalKind.pollen) h.update(`p${p.grain?.kind ?? ""}\0`);
    }
    let words = "";
    let calls = "";
    let answers = "";
    for (const p of b.grew ?? []) {
      if (p.kind === PetalKind.text) words += p.text ?? "";
      else if (p.kind === PetalKind.tendril) calls += `${p.callId ?? ""}\0`;
      else if (p.kind === PetalKind.grasp) answers += `${p.callId ?? ""}\0`;
    }
    h.update(`G${unspaced(words)}\0c${calls}r${answers}`);
  }
  return h.digest("hex");
}

export class Sigils {
  readonly #byTrace = new Map<string, string>();
  readonly #cap: number;
  readonly #growing = new Set<string>();
  readonly #owner = new Map<string, string>();
  readonly #forget: (sid: string, label: string) => void;

  constructor(forget: (sid: string, label: string) => void, cap = 4096) {
    this.#forget = forget;
    this.#cap = cap;
  }

  fresh(): string {
    return `oai-${randomUUID()}`;
  }

  continuing(label: string, system: string | undefined, blooms: Bloom[]): string {
    if (blooms.length === 0) return this.fresh();
    const sid = this.#byTrace.get(trace(label, system, blooms));
    return sid && !this.#growing.has(sid) ? sid : this.fresh();
  }

  growing(sid: string): boolean {
    return this.#growing.has(sid);
  }

  begin(sid: string): void {
    this.#growing.add(sid);
  }

  end(sid: string): void {
    this.#growing.delete(sid);
  }

  remember(label: string, system: string | undefined, blooms: Bloom[], sid: string): void {
    const key = trace(label, system, blooms);
    this.#byTrace.delete(key);
    this.#byTrace.set(key, sid);
    this.#owner.set(sid, label);
    while (this.#byTrace.size > this.#cap) {
      const [oldest, gone] = this.#byTrace.entries().next().value as [string, string];
      this.#byTrace.delete(oldest);
      if (![...this.#byTrace.values()].includes(gone) && !this.#growing.has(gone)) {
        this.#forget(gone, this.#owner.get(gone) ?? "");
        this.#owner.delete(gone);
      }
    }
  }
}
