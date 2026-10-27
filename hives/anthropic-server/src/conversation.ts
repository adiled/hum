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
// A conversation keeps one sigil while it only grows. The sid a request
// continues is the one its blooms so far were last grown in, for this
// tenant; a conversation no one has seen, one whose history was edited,
// or one already growing elsewhere gets a sigil of its own. The trace that
// finds it is the tenant, the system prompt, the words of every turn and
// the ids of every tool use, which a client sends back as it was given
// them.

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

export function trace(label: string, system: string | undefined, blooms: Bloom[]): string {
  const h = createHash("sha256");
  h.update(`${label}\0${squeeze(system ?? "")}\0`);
  for (const b of blooms) {
    h.update("U");
    for (const p of b.content ?? []) {
      if (p.kind === PetalKind.text) h.update(`t${squeeze(p.text ?? "")}\0`);
      else if (p.kind === PetalKind.pollen) h.update(`p${p.grain?.kind ?? ""}\0`);
    }
    h.update("G");
    let words = "";
    for (const p of b.grew ?? []) {
      if (p.kind === PetalKind.text) {
        words += p.text ?? "";
        continue;
      }
      if (p.kind === PetalKind.tendril || p.kind === PetalKind.grasp) {
        if (squeeze(words)) h.update(`t${squeeze(words)}\0`);
        words = "";
        h.update(`${p.kind === PetalKind.tendril ? "c" : "r"}${p.callId ?? ""}\0`);
      }
    }
    if (squeeze(words)) h.update(`t${squeeze(words)}\0`);
  }
  return h.digest("hex");
}

export class Sigils {
  readonly #byTrace = new Map<string, string>();
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

  continuing(label: string, system: string | undefined, blooms: Bloom[]): string {
    if (blooms.length === 0) return this.fresh(label);
    const sid = this.#byTrace.get(trace(label, system, blooms));
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

  remember(label: string, system: string | undefined, blooms: Bloom[], sid: string): void {
    const key = trace(label, system, blooms);
    this.#byTrace.delete(key);
    this.#byTrace.set(key, sid);
    while (this.#byTrace.size > this.#cap) {
      const [oldest, gone] = this.#byTrace.entries().next().value as [string, string];
      this.#byTrace.delete(oldest);
      if (![...this.#byTrace.values()].includes(gone) && !this.#growing.has(gone)) this.#forget(gone);
    }
  }
}
