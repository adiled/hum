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
// A conversation keeps one sigil while it grows, for this bee; one already
// growing elsewhere gets a sigil of its own. A call may name its line with
// `providerOptions.hum.sid`. Otherwise the sigil is found by the newest
// tool call or result id the conversation holds that this bee was given,
// or by the words of its last two turns, and not by the whole history: an
// app compacts and reshapes it, dropping old tool pairs or trimming, while
// a call's id is echoed as it was given, since its result needs it, and
// the newest turns are left alone. An app that edits an older turn keeps
// its sigil. A sigil goes on only from where it last left off, so a copy
// of the conversation taken before then starts its own. A conversation
// that ends in tool results is found by its last two turns before them
// too.

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

function unspaced(s: string): string {
  return s.replace(/\s+/g, "");
}

function tailOf(blooms: Bloom[]): string | undefined {
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
  return worded ? createHash("sha256").update(said).digest("hex") : undefined;
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
  readonly #forget: (sid: string) => void;

  constructor(forget: (sid: string) => void, cap = 4096) {
    this.#forget = forget;
    this.#cap = cap;
  }

  fresh(): string {
    return `vai-${randomUUID()}`;
  }

  named(name: string): string {
    const sid = `vai-line-${createHash("sha256").update(name).digest("hex").slice(0, 32)}`;
    return this.#growing.has(sid) ? this.fresh() : sid;
  }

  continuing(blooms: Bloom[]): string {
    if (blooms.length === 0) return this.fresh();
    const ids = callIds(blooms);
    const byCall = [...ids].reverse().map((id) => this.#byCall.get(id)).find((found) => found !== undefined);
    const newest = byCall === undefined ? undefined : this.#latest.get(byCall)?.call;
    const sid = (newest !== undefined && ids.includes(newest) ? byCall : undefined) ?? [blooms, beforeAnswers(blooms, false), beforeAnswers(blooms, true)]
      .map((b) => this.#leftOff(tailOf(b)))
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

  remember(blooms: Bloom[], sid: string, ran: string[] = []): void {
    for (const id of ran) {
      this.#ran.delete(id);
      this.#ran.add(id);
    }
    while (this.#ran.size > this.#cap) this.#ran.delete(this.#ran.values().next().value as string);
    const ids = callIds(blooms);
    for (const id of ids) this.#keep(this.#byCall, id, sid);
    const tail = tailOf(blooms);
    if (tail !== undefined) this.#keep(this.#byTail, tail, sid);
    this.#latest.set(sid, { call: [...ids].reverse().find((id) => !this.#ran.has(id)), tail });
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
