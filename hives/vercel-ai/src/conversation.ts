// An AI SDK prompt, as the blooms of a sigil.
//
// The AI SDK sends the whole conversation on every call; hum keeps it in a
// sigil, one bloom a turn. A conversation reads as blooms: each user
// message opens one, its words and files its content, and what the
// assistant said, reasoned and called, and what came back from its tools,
// are what grew in it. System messages are the system prompt.
//
// A call's last bloom is what it asks for. Its words are the prompt's
// content and the blooms before it the graft. A conversation that ends
// with tool results or with the assistant's own words asks for a bloom to
// grow on: the prompt has no content, and every bloom is its graft. The
// results of the tool messages it ends with are its answers.
//
// A conversation keeps one sigil while it only grows. The sid a call
// continues is the one its blooms so far were last grown in, by this bee;
// a conversation no one has seen, one whose history was edited, or one
// already growing elsewhere gets a sigil of its own. The trace that finds
// it is the system prompt, the words of every turn and the ids of every
// tool call, which the SDK sends back as it was given them.

import { createHash, randomUUID } from "node:crypto";
import type { LanguageModelV3FilePart, LanguageModelV3Prompt, LanguageModelV3ToolResultOutput } from "@ai-sdk/provider";
import { PetalKind, type Bloom, type Petal } from "../../../thrum-clients/ts/index.ts";

export type Tail = "user" | "tool" | "assistant" | "none";

export interface Said {
  system?: string;
  blooms: Bloom[];
  tail: Tail;
  answers: Answer[];
}

export interface Answer {
  callId: string;
  result: string;
  isError?: boolean;
}

function pollenOf(part: LanguageModelV3FilePart): Petal {
  const mediaType = part.mediaType || "application/octet-stream";
  const kind = mediaType.startsWith("image/") ? "image" : mediaType.startsWith("audio/") ? "audio" : "file";
  const data = part.data;
  const grain = data instanceof URL
    ? { kind, mediaType, url: data.toString() }
    : { kind, mediaType, data: typeof data === "string" ? data : Buffer.from(data).toString("base64") };
  return { kind: PetalKind.pollen, grain };
}

export function graspOf(output: LanguageModelV3ToolResultOutput): Omit<Answer, "callId"> {
  switch (output.type) {
    case "text":
      return { result: output.value };
    case "json":
      return { result: JSON.stringify(output.value) };
    case "error-text":
      return { result: output.value, isError: true };
    case "error-json":
      return { result: JSON.stringify(output.value), isError: true };
    case "execution-denied":
      return { result: output.reason ?? "the caller denied this call", isError: true };
    case "content":
      return { result: output.value.map((p) => (p.type === "text" ? p.text : "")).join("") };
  }
}

function grasp(callId: string, output: LanguageModelV3ToolResultOutput): Petal {
  const { result, isError } = graspOf(output);
  return { kind: PetalKind.grasp, callId, result, ...(isError ? { isError } : {}) };
}

class Reader {
  system: string[] = [];
  blooms: Bloom[] = [];
  tail: Tail = "none";
  answers: Answer[] = [];

  said(petals: Petal[]): void {
    const last = this.blooms[this.blooms.length - 1];
    if (last && (last.grew ?? []).length === 0) last.content = [...(last.content ?? []), ...petals];
    else this.blooms.push({ content: petals, grew: [] });
    this.tail = "user";
    this.answers = [];
  }

  grew(petals: Petal[], tail: Tail): void {
    let bloom = this.blooms[this.blooms.length - 1];
    if (!bloom) this.blooms.push((bloom = { content: [], grew: [] }));
    bloom.grew = [...(bloom.grew ?? []), ...petals];
    if (tail !== "tool") this.answers = [];
    else for (const p of petals) this.answers.push({ callId: p.callId ?? "", result: String(p.result ?? ""), ...(p.isError ? { isError: true } : {}) });
    this.tail = tail;
  }

  done(): Said {
    const system = this.system.filter((s) => s.length > 0).join("\n\n");
    return { system: system || undefined, blooms: this.blooms, tail: this.tail, answers: this.answers };
  }
}

export function fromPrompt(prompt: LanguageModelV3Prompt): Said {
  const r = new Reader();
  for (const m of prompt) {
    switch (m.role) {
      case "system":
        r.system.push(m.content);
        break;
      case "user":
        r.said(m.content.map((p) => (p.type === "text" ? { kind: PetalKind.text, text: p.text } : pollenOf(p))));
        break;
      case "assistant": {
        const petals: Petal[] = [];
        for (const p of m.content) {
          if (p.type === "text") petals.push({ kind: PetalKind.text, text: p.text });
          else if (p.type === "reasoning") petals.push({ kind: PetalKind.sepal, text: p.text });
          else if (p.type === "file") petals.push(pollenOf(p));
          else if (p.type === "tool-call") petals.push({ kind: PetalKind.tendril, callId: p.toolCallId, name: p.toolName, args: p.input });
          else if (p.type === "tool-result") petals.push(grasp(p.toolCallId, p.output));
        }
        r.grew(petals, "assistant");
        break;
      }
      case "tool": {
        const petals: Petal[] = [];
        for (const p of m.content) if (p.type === "tool-result") petals.push(grasp(p.toolCallId, p.output));
        if (petals.length > 0) r.grew(petals, "tool");
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

function squeeze(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}

export function trace(system: string | undefined, blooms: Bloom[]): string {
  const h = createHash("sha256");
  h.update(`${squeeze(system ?? "")}\0`);
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

  fresh(): string {
    return `vai-${randomUUID()}`;
  }

  continuing(system: string | undefined, blooms: Bloom[]): string {
    if (blooms.length === 0) return this.fresh();
    const sid = this.#byTrace.get(trace(system, blooms));
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

  remember(system: string | undefined, blooms: Bloom[], sid: string): void {
    const key = trace(system, blooms);
    this.#byTrace.delete(key);
    this.#byTrace.set(key, sid);
    while (this.#byTrace.size > this.#cap) {
      const [oldest, gone] = this.#byTrace.entries().next().value as [string, string];
      this.#byTrace.delete(oldest);
      if (![...this.#byTrace.values()].includes(gone) && !this.#growing.has(gone)) this.#forget(gone);
    }
  }
}
