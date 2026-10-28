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
// A conversation keeps one sigil while it grows, by this caller; strangers
// never share one, and one already growing elsewhere gets a sigil of its
// own. A client may name its line with `X-Hum-Sid`. Otherwise the sigil is
// found by the newest tool call or answer id the conversation holds that
// this caller was given, or by the words of its last two turns, and not by
// the whole history: a client compacts and reshapes it all the time,
// dropping old tool pairs, trimming, sending a turn back as one message or
// without the words said between its calls, while a call's id is echoed as
// it was given, since the answer needs it, and the newest turns are left
// alone. A client that edits an older turn keeps its sigil. A sigil goes
// on only from where it last left off, so a copy of the conversation taken
// before then starts its own. A conversation that ends in answers is found
// by its last two turns before them too: answers that came after their
// call was let go, or a call and its answer a client made up to hand the
// model news.

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

function servedPetals(id: string, name: string, args: unknown, output: unknown, error: unknown): Petal[] {
  return [
    { kind: PetalKind.tendril, callId: id, name, args: argsOf(args) },
    { kind: PetalKind.grasp, callId: id, result: resultOf(output ?? error), isError: Boolean(error) },
  ];
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

export interface McpCall {
  id?: string;
  name?: string;
  arguments?: unknown;
  output?: unknown;
  error?: unknown;
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
  mcp_calls?: McpCall[];
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
        for (const c of Array.isArray(m.mcp_calls) ? m.mcp_calls : []) {
          petals.push(...servedPetals(c.id ?? "", c.name ?? "", c.arguments, c.output, c.error));
        }
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
        r.grew(servedPetals(item.id ?? "", item.name ?? "", item.arguments, item.output, item.error), "assistant");
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

function tailOf(label: string, blooms: Bloom[]): string | undefined {
  let said = "";
  let worded = false;
  for (const b of blooms.slice(-2)) {
    let user = "";
    for (const p of b.content ?? []) {
      if (p.kind === PetalKind.text) user += `${squeeze(p.text ?? "")}\0`;
      else if (p.kind === PetalKind.pollen) user += `p${p.grain?.kind ?? ""}\0`;
    }
    const words = unspaced((b.grew ?? []).filter((p) => p.kind === PetalKind.text).map((p) => p.text ?? "").join(""));
    worded ||= words.length > 0 || (b.content ?? []).some((p) => p.kind === PetalKind.text && squeeze(p.text ?? "").length > 0);
    said += `U${user}G${words}\0`;
  }
  return worded ? createHash("sha256").update(`${label}\0${said}`).digest("hex") : undefined;
}

function callIds(blooms: Bloom[]): string[] {
  return blooms.flatMap((b) => (b.grew ?? []).filter((p) => (p.kind === PetalKind.tendril || p.kind === PetalKind.grasp) && p.callId).map((p) => p.callId as string));
}

function beforeAnswers(blooms: Bloom[], withTheirCalls: boolean): Bloom[] {
  const last = blooms[blooms.length - 1];
  const grew = [...(last?.grew ?? [])];
  const answered = new Set<string>();
  while (grew.length > 0 && (grew[grew.length - 1].kind === PetalKind.grasp || grew[grew.length - 1].kind === PetalKind.sepal)) {
    const p = grew.pop()!;
    if (p.kind === PetalKind.grasp) answered.add(p.callId ?? "");
  }
  const kept = withTheirCalls ? grew.filter((p) => !(p.kind === PetalKind.tendril && answered.has(p.callId ?? ""))) : grew;
  return [...blooms.slice(0, -1), { ...last, grew: kept }];
}

export class Sigils {
  readonly #byCall = new Map<string, string>();
  readonly #byTail = new Map<string, string>();
  readonly #keys = new Map<string, number>();
  readonly #latest = new Map<string, { call?: string; tail?: string }>();
  readonly #ran = new Set<string>();
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

  named(label: string, name: string): string {
    const sid = `oai-${createHash("sha256").update(`${label}\0${name}`).digest("hex").slice(0, 32)}`;
    return this.#growing.has(sid) ? this.fresh() : sid;
  }

  continuing(label: string, blooms: Bloom[]): string {
    if (blooms.length === 0) return this.fresh();
    const ids = callIds(blooms);
    const byCall = [...ids].reverse().map((id) => this.#byCall.get(`${label}\0${id}`)).find((found) => found !== undefined);
    const newest = byCall === undefined ? undefined : this.#latest.get(byCall)?.call;
    const sid = (newest !== undefined && ids.includes(newest) ? byCall : undefined) ?? [blooms, beforeAnswers(blooms, false), beforeAnswers(blooms, true)]
      .map((b) => this.#leftOff(tailOf(label, b)))
      .find((found) => found !== undefined);
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

  remember(label: string, blooms: Bloom[], sid: string, ran: string[] = []): void {
    for (const id of ran) {
      this.#ran.delete(`${label}\0${id}`);
      this.#ran.add(`${label}\0${id}`);
    }
    while (this.#ran.size > this.#cap) this.#ran.delete(this.#ran.values().next().value as string);
    this.#owner.set(sid, label);
    const ids = callIds(blooms);
    for (const id of ids) this.#keep(this.#byCall, `${label}\0${id}`, sid);
    const tail = tailOf(label, blooms);
    if (tail !== undefined) this.#keep(this.#byTail, tail, sid);
    this.#latest.set(sid, { call: [...ids].reverse().find((id) => !this.#ran.has(`${label}\0${id}`)), tail });
    for (const map of [this.#byCall, this.#byTail]) {
      while (map.size > this.#cap) {
        const [oldest, gone] = map.entries().next().value as [string, string];
        map.delete(oldest);
        this.#drop(gone);
      }
    }
  }

  #leftOff(key: string | undefined): string | undefined {
    const sid = key === undefined ? undefined : this.#byTail.get(key);
    return sid !== undefined && this.#latest.get(sid)?.tail === key ? sid : undefined;
  }

  #keep(map: Map<string, string>, key: string, sid: string): void {
    const was = map.get(key);
    map.delete(key);
    map.set(key, sid);
    if (was === sid) return;
    this.#keys.set(sid, (this.#keys.get(sid) ?? 0) + 1);
    if (was !== undefined) this.#drop(was);
  }

  #drop(sid: string): void {
    const left = (this.#keys.get(sid) ?? 1) - 1;
    if (left > 0) {
      this.#keys.set(sid, left);
      return;
    }
    this.#keys.delete(sid);
    if (this.#growing.has(sid)) return;
    this.#forget(sid, this.#owner.get(sid) ?? "");
    this.#owner.delete(sid);
    this.#latest.delete(sid);
  }
}
