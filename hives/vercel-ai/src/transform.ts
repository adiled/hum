// Translate a bloom's thrum tones into Vercel AI SDK v3 stream parts.
//
// What the bee hears for a sid:
//   { chi: "unfurl", petal, kind, text | sap | callId, name | closed }
//   { chi: "tendril-reach", callId, name, args } — a declared tool, reaching
//   { chi: "wilt", reason, nectar }              — the bloom closes
//   { chi: "error", message }                    — the bloom broke off
//
// Each petal is one block: a text petal text-start/delta/end, a sepal
// reasoning-start/delta/end, a tendril tool-input-start/delta/end. A
// tendril that reaches is a tool-call the consumer answers out of band
// with chi:"tendril-result"; one whose petal already unfurled only adds
// its tool-call. One translator serves one doStream call, so no end is
// ever emitted without its start. Anything else (perf-mark, pulse,
// breath, echo, drone-retrofit, …) is out-of-band signalling for richer
// consumers, and this pure provider ignores it.

import type {
  LanguageModelV3StreamPart,
  LanguageModelV3FinishReason,
  LanguageModelV3Usage,
} from "@ai-sdk/provider";

import { PetalKind, type Tone } from "../../../thrum-clients/ts/index.ts";

function emptyUsage(): LanguageModelV3Usage {
  return {
    inputTokens: { total: 0, noCache: 0, cacheRead: 0, cacheWrite: 0 },
    outputTokens: { total: 0, text: 0, reasoning: 0 },
  };
}

function mapFinish(raw: string | undefined): LanguageModelV3FinishReason {
  const r = (raw ?? "stop").toLowerCase();
  const unified: LanguageModelV3FinishReason["unified"] =
      r === "stop" || r === "end_turn" || r === "success" ? "stop"
    : r === "length" || r === "max_tokens" ? "length"
    : r === "tool_calls" || r === "tool-use" || r === "tool_use" ? "tool-calls"
    : r === "content_filter" ? "content-filter"
    : r === "error" ? "error"
    : "other";
  return { unified, raw: r };
}

function mapUsage(nectar: unknown): LanguageModelV3Usage {
  if (!nectar || typeof nectar !== "object") return emptyUsage();
  const n = nectar as Record<string, number>;
  const inputTotal = (n.input_tokens ?? 0) + (n.cache_read_input_tokens ?? 0) + (n.cache_creation_input_tokens ?? 0);
  return {
    inputTokens: {
      total: inputTotal,
      noCache: n.input_tokens ?? 0,
      cacheRead: n.cache_read_input_tokens ?? 0,
      cacheWrite: n.cache_creation_input_tokens ?? 0,
    },
    outputTokens: {
      total: n.output_tokens ?? 0,
      text: n.output_tokens ?? 0,
      reasoning: 0,
    },
  };
}

type Block = { kind: string; id: string };

export class HumTranslator {
  private blocks = new Map<number, Block>();
  private unfurledCalls = new Set<string>();
  private finished = false;

  *ingest(msg: Tone): Iterable<LanguageModelV3StreamPart> {
    if (this.finished) return;

    if (msg.chi === "error") {
      yield* this.closeAll();
      yield { type: "error", error: new Error(msg.message || "thrum error") };
      yield { type: "finish", finishReason: { unified: "error", raw: "error" }, usage: emptyUsage() };
      this.finished = true;
      return;
    }

    if (msg.chi === "wilt") {
      yield* this.closeAll();
      yield { type: "finish", finishReason: mapFinish(msg.reason), usage: mapUsage(msg.nectar) };
      this.finished = true;
      return;
    }

    if (msg.chi === "tendril-reach") {
      yield* this.closeAll();
      const input = msg.args !== undefined
        ? (typeof msg.args === "string" ? msg.args : JSON.stringify(msg.args))
        : "{}";
      if (!this.unfurledCalls.has(msg.callId)) {
        yield { type: "tool-input-start", id: msg.callId, toolName: msg.name };
        yield { type: "tool-input-end", id: msg.callId };
      }
      yield { type: "tool-call", toolCallId: msg.callId, toolName: msg.name, input };
      return;
    }

    if (msg.chi !== "unfurl") return;

    if (msg.closed) {
      const block = this.blocks.get(msg.petal);
      if (block) {
        this.blocks.delete(msg.petal);
        yield this.end(block);
      }
      return;
    }

    let block = this.blocks.get(msg.petal);
    if (!block) {
      if (msg.kind === PetalKind.text) block = { kind: msg.kind, id: `t${msg.petal}-${Date.now()}` };
      else if (msg.kind === PetalKind.sepal) block = { kind: msg.kind, id: `r${msg.petal}-${Date.now()}` };
      else if (msg.kind === PetalKind.tendril && msg.callId && msg.name) block = { kind: msg.kind, id: msg.callId };
      else return;
      this.blocks.set(msg.petal, block);
      if (block.kind === PetalKind.text) yield { type: "text-start", id: block.id };
      if (block.kind === PetalKind.sepal) yield { type: "reasoning-start", id: block.id };
      if (block.kind === PetalKind.tendril) {
        this.unfurledCalls.add(block.id);
        yield { type: "tool-input-start", id: block.id, toolName: msg.name ?? "" };
      }
    }

    if (block.kind === PetalKind.text && msg.text) yield { type: "text-delta", id: block.id, delta: msg.text };
    if (block.kind === PetalKind.sepal && msg.text) yield { type: "reasoning-delta", id: block.id, delta: msg.text };
    if (block.kind === PetalKind.tendril && msg.sap) yield { type: "tool-input-delta", id: block.id, delta: msg.sap };
  }

  *flush(): Iterable<LanguageModelV3StreamPart> {
    if (this.finished) return;
    yield* this.closeAll();
    yield { type: "finish", finishReason: { unified: "other", raw: "aborted" }, usage: emptyUsage() };
    this.finished = true;
  }

  private end(block: Block): LanguageModelV3StreamPart {
    if (block.kind === PetalKind.text) return { type: "text-end", id: block.id };
    if (block.kind === PetalKind.sepal) return { type: "reasoning-end", id: block.id };
    return { type: "tool-input-end", id: block.id };
  }

  private *closeAll(): Iterable<LanguageModelV3StreamPart> {
    for (const block of this.blocks.values()) yield this.end(block);
    this.blocks.clear();
  }
}
