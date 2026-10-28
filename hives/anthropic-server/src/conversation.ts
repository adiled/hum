// An Anthropic conversation, as the blooms of a sigil.
//
// The Messages API sends the whole conversation every time; hum keeps it
// in a sigil, one bloom a turn. A conversation reads as blooms: each user
// turn opens one, its words, images and documents its content, and what
// the assistant said and thought, the tools it used and the results that
// came back are what grew in it. `system`, a string or text blocks, and
// the words of any message whose role is system, are the system prompt.
//
// An image or a document is pollen: one grain, its bytes inline or a url
// to fetch them, a plain-text document's words as the bytes of a
// text/plain file, and a document of content blocks as those blocks. A
// tool result is a grasp of its words. What only Anthropic can read, a
// Files API id, redacted thinking, a server tool's blocks, is left out.
//
// A request's last bloom is what it asks for. Its words are the prompt's
// content and the blooms before it the graft. A conversation that ends
// with tool results or with the assistant's own words asks for a bloom to
// grow on: the prompt has no content, and every bloom is its graft.
//
// A conversation keeps one sigil while it grows, for this tenant; tenants
// never share one, and one already growing elsewhere gets a sigil of its
// own. A client may name its line with `X-Hum-Sid`. Otherwise the sigil is
// found by the newest tool use or result id the conversation holds that
// this tenant was given, or by the words of its last two turns, and not by
// the whole history: a client compacts and reshapes it all the time,
// dropping old tool pairs, trimming, or sending a turn back without the
// words said between its tool uses, while a tool use's id is echoed as it
// was given, since its result needs it, and the newest turns are left
// alone. A client that edits an older turn keeps its sigil. A sigil goes
// on only from where it last left off, so a copy of the conversation taken
// before then starts its own. A conversation that ends in tool results is
// found by its last two turns before them too.

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

export interface Message {
  role: string;
  content?: unknown;
}

type Block = Record<string, any>;

const text = (t: string): Petal => ({ kind: PetalKind.text, text: t });
const pollen = (grain: Grain): Petal => ({ kind: PetalKind.pollen, grain });

function blocksOf(content: unknown): Block[] {
  if (typeof content === "string") return content.length > 0 ? [{ type: "text", text: content }] : [];
  return Array.isArray(content) ? content.filter((b) => b && typeof b === "object") : [];
}

function wordsOf(content: unknown, sep: string): string {
  return blocksOf(content).filter((b) => b.type === "text" && typeof b.text === "string").map((b) => b.text).join(sep);
}

export function systemOf(system: unknown): string | undefined {
  return wordsOf(system, "\n\n") || undefined;
}

function grainOf(source: Block | undefined, kind: string, fallbackType: string): Grain | undefined {
  switch (source?.type) {
    case "base64":
      return typeof source.data === "string" ? { kind, mediaType: source.media_type ?? fallbackType, data: source.data } : undefined;
    case "url":
      return typeof source.url === "string" ? { kind, mediaType: source.media_type ?? fallbackType, url: source.url } : undefined;
    case "text":
      return typeof source.data === "string" ? { kind, mediaType: source.media_type ?? "text/plain", data: Buffer.from(source.data).toString("base64") } : undefined;
  }
  return undefined;
}

function partsOf(content: unknown): Petal[] {
  const out: Petal[] = [];
  for (const b of blocksOf(content)) {
    switch (b.type) {
      case "text":
        if (typeof b.text === "string") out.push(text(b.text));
        break;
      case "image": {
        const grain = grainOf(b.source, "image", "image/*");
        if (grain) out.push(pollen(grain));
        break;
      }
      case "document": {
        if (b.source?.type === "content") {
          out.push(...partsOf(b.source.content));
          break;
        }
        const grain = grainOf(b.source, "file", "application/pdf");
        if (grain) out.push(pollen(grain));
        break;
      }
    }
  }
  return out;
}

function grewOf(content: unknown): Petal[] {
  const out: Petal[] = [];
  for (const b of blocksOf(content)) {
    if (b.type === "text" && typeof b.text === "string") out.push(text(b.text));
    else if (b.type === "thinking") out.push({ kind: PetalKind.sepal, text: b.thinking ?? "", ...(b.signature ? { propolis: b.signature } : {}) });
    else if (b.type === "tool_use") out.push({ kind: PetalKind.tendril, callId: b.id ?? "", name: b.name ?? "", args: b.input ?? {} });
  }
  return out;
}

class Reader {
  system: string[] = [];
  blooms: Bloom[] = [];
  tail: Tail = "none";

  said(petals: Petal[]): void {
    const last = this.blooms[this.blooms.length - 1];
    if (last && (last.grew ?? []).length === 0) last.content = [...(last.content ?? []), ...petals];
    else this.blooms.push({ content: petals, grew: [] });
    this.tail = "user";
  }

  grew(petals: Petal[], tail: Tail): void {
    let bloom = this.blooms[this.blooms.length - 1];
    if (!bloom) this.blooms.push((bloom = { content: [], grew: [] }));
    bloom.grew = [...(bloom.grew ?? []), ...petals];
    this.tail = tail;
  }

  done(): Said {
    const system = this.system.filter((s) => s.length > 0).join("\n\n");
    return { system: system || undefined, blooms: this.blooms, tail: this.tail };
  }
}

export function fromMessages(system: unknown, messages: Message[]): Said {
  const r = new Reader();
  const head = systemOf(system);
  if (head) r.system.push(head);
  for (const m of messages) {
    switch (m.role) {
      case "system":
        r.system.push(wordsOf(m.content, "\n\n"));
        break;
      case "assistant":
        r.grew(grewOf(m.content), "assistant");
        break;
      case "user": {
        const blocks = blocksOf(m.content);
        const results = blocks.filter((b) => b.type === "tool_result");
        if (results.length > 0) {
          r.grew(results.map((b) => ({
            kind: PetalKind.grasp,
            callId: b.tool_use_id ?? "",
            result: wordsOf(b.content, "\n"),
            ...(b.is_error === true ? { isError: true } : {}),
          })), "tool");
        }
        const rest = blocks.filter((b) => b.type !== "tool_result");
        if (rest.length > 0 || results.length === 0) r.said(partsOf(rest));
        break;
      }
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
    out.unshift({ callId: grew[i].callId ?? "", result: String(grew[i].result ?? ""), ...(grew[i].isError ? { isError: true } : {}) });
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
  readonly #cap: number;
  readonly #growing = new Set<string>();
  readonly #forget: (sid: string) => void;

  constructor(forget: (sid: string) => void, cap = 4096) {
    this.#forget = forget;
    this.#cap = cap;
  }

  fresh(label: string): string {
    return label === "default" ? randomUUID() : `${label}:${randomUUID()}`;
  }

  named(label: string, name: string): string {
    const line = createHash("sha256").update(`${label}\0${name}`).digest("hex").slice(0, 32);
    const sid = label === "default" ? `line-${line}` : `${label}:line-${line}`;
    return this.#growing.has(sid) ? this.fresh(label) : sid;
  }

  continuing(label: string, blooms: Bloom[]): string {
    if (blooms.length === 0) return this.fresh(label);
    const ids = callIds(blooms);
    const byCall = [...ids].reverse().map((id) => this.#byCall.get(`${label}\0${id}`)).find((found) => found !== undefined);
    const newest = byCall === undefined ? undefined : this.#latest.get(byCall)?.call;
    const sid = (newest !== undefined && ids.includes(newest) ? byCall : undefined) ?? [blooms, beforeAnswers(blooms, false), beforeAnswers(blooms, true)]
      .map((b) => this.#leftOff(tailOf(label, b)))
      .find((found) => found !== undefined);
    return sid && !this.#growing.has(sid) ? sid : this.fresh(label);
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

  remember(label: string, blooms: Bloom[], sid: string): void {
    const ids = callIds(blooms);
    for (const id of ids) this.#keep(this.#byCall, `${label}\0${id}`, sid);
    const tail = tailOf(label, blooms);
    if (tail !== undefined) this.#keep(this.#byTail, tail, sid);
    this.#latest.set(sid, { call: ids[ids.length - 1], tail });
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
    this.#forget(sid);
    this.#latest.delete(sid);
  }
}
