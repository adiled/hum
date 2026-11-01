// Translate a bloom's thrum tones into Vercel AI SDK v3 stream parts.
//
// What the bee hears for a sid:
//   { chi: "unfurl", petal, kind, text | sap | callId, name | closed }
//   { chi: "tendril-reach", callId, name, args }   one of the call's tools, reaching
//   { chi: "tendril-info", callId, name, args, result, isError }   a tendril already back
//   { chi: "wilt", reason, nectar }                the bloom closes
//   { chi: "error", message }                      the bloom broke off
//
// Each petal is one block: a text petal text-start/delta/end, a sepal
// reasoning-start/delta/end, a tendril tool-input-start/delta/end. A reach
// for one of the call's tools is a tool-call the SDK runs; one whose petal
// already unfurled only adds its tool-call. A tendril served inside humd,
// by an attendant or a builder, is told whole by tendril-info and becomes a
// provider-executed tool-call and its tool-result; tendril-info for a reach
// the call itself answers, or reached, is old news. What grew is kept as
// petals, in order, for the conversation's sigil. One translator serves one
// doStream call, so no end is ever emitted without its start. Anything else
// (perf-mark, pulse, breath, echo, drone-retrofit, …) is out-of-band
// signalling this pure provider ignores.

import type {
  JSONValue,
  LanguageModelV3StreamPart,
  LanguageModelV3FinishReason,
  LanguageModelV3Usage,
} from "@ai-sdk/provider";

import { PetalKind, type Petal, type Tone } from "../../../thrum-clients/ts/index.ts";

export function emptyUsage(): LanguageModelV3Usage {
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

function inputOf(args: unknown): string {
  if (args === undefined) return "{}";
  return typeof args === "string" ? args : JSON.stringify(args);
}

type Block = { kind: string; id: string; petal?: Petal };

export class HumTranslator {
  readonly grew: Petal[] = [];
  readonly served: string[] = [];
  private blocks = new Map<number, Block>();
  private started = new Set<string>();
  private called = new Set<string>();
  private n = 0;
  private finished = false;

  constructor(private own: Set<string>, private answered: Set<string>) {}

  *ingest(msg: Tone): Iterable<LanguageModelV3StreamPart> {
    if (this.finished) return;

    if (msg.chi === "error") {
      yield* this.closeAll();
      yield { type: "error", error: new Error(msg.message || "the bloom broke off") };
      yield { type: "finish", finishReason: { unified: "error", raw: msg.code ?? "error" }, usage: mapUsage(msg.nectar) };
      this.finished = true;
      return;
    }

    if (msg.chi === "wilt") {
      yield* this.end(mapFinish(msg.reason), mapUsage(msg.nectar));
      return;
    }

    if (msg.chi === "tendril-reach") {
      if (!this.own.has(msg.name) || this.called.has(msg.callId)) return;
      yield* this.closeAll();
      this.called.add(msg.callId);
      if (!this.started.has(msg.callId)) {
        yield { type: "tool-input-start", id: msg.callId, toolName: msg.name };
        yield { type: "tool-input-end", id: msg.callId };
      }
      yield { type: "tool-call", toolCallId: msg.callId, toolName: msg.name, input: inputOf(msg.args) };
      this.grew.push({ kind: PetalKind.tendril, callId: msg.callId, name: msg.name, args: msg.args ?? {} });
      return;
    }

    if (msg.chi === "tendril-info") {
      if (this.answered.has(msg.callId) || this.called.has(msg.callId)) return;
      const toolName = msg.name ?? "";
      const served = { providerExecuted: true, dynamic: true } as const;
      if (!this.started.has(msg.callId)) {
        yield { type: "tool-input-start", id: msg.callId, toolName, ...served };
        yield { type: "tool-input-end", id: msg.callId };
      }
      yield { type: "tool-call", toolCallId: msg.callId, toolName, input: inputOf(msg.args), ...served };
      yield { type: "tool-result", toolCallId: msg.callId, toolName, result: (msg.result ?? "") as NonNullable<JSONValue>, ...(msg.isError ? { isError: true } : {}), dynamic: true };
      this.served.push(msg.callId);
      this.grew.push(
        { kind: PetalKind.tendril, callId: msg.callId, name: toolName, args: msg.args ?? {} },
        { kind: PetalKind.grasp, callId: msg.callId, result: msg.result ?? "", ...(msg.isError ? { isError: true } : {}) },
      );
      return;
    }

    if (msg.chi !== "unfurl") return;

    if (msg.closed) {
      const block = this.blocks.get(msg.petal);
      if (block) {
        this.blocks.delete(msg.petal);
        yield this.endOf(block);
      }
      return;
    }

    let block = this.blocks.get(msg.petal);
    if (!block) {
      if (msg.kind === PetalKind.text || msg.kind === PetalKind.sepal) {
        const petal: Petal = { kind: msg.kind, text: "" };
        this.grew.push(petal);
        block = { kind: msg.kind, id: `${msg.kind === PetalKind.text ? "t" : "r"}${this.n++}`, petal };
      } else if (msg.kind === PetalKind.tendril && msg.callId && msg.name) {
        block = { kind: msg.kind, id: msg.callId };
      } else return;
      this.blocks.set(msg.petal, block);
      if (block.kind === PetalKind.text) yield { type: "text-start", id: block.id };
      if (block.kind === PetalKind.sepal) yield { type: "reasoning-start", id: block.id };
      if (block.kind === PetalKind.tendril) {
        this.started.add(block.id);
        const served = this.own.has(msg.name ?? "") ? {} : { providerExecuted: true, dynamic: true };
        yield { type: "tool-input-start", id: block.id, toolName: msg.name ?? "", ...served };
      }
    }

    if (block.petal && msg.text) block.petal.text += msg.text;
    if (block.kind === PetalKind.text && msg.text) yield { type: "text-delta", id: block.id, delta: msg.text };
    if (block.kind === PetalKind.sepal && msg.text) yield { type: "reasoning-delta", id: block.id, delta: msg.text };
    if (block.kind === PetalKind.tendril && msg.sap) yield { type: "tool-input-delta", id: block.id, delta: msg.sap };
  }

  *end(finishReason: LanguageModelV3FinishReason, usage = emptyUsage()): Iterable<LanguageModelV3StreamPart> {
    if (this.finished) return;
    yield* this.closeAll();
    yield { type: "finish", finishReason, usage };
    this.finished = true;
  }

  *flush(): Iterable<LanguageModelV3StreamPart> {
    yield* this.end({ unified: "other", raw: "aborted" });
  }

  private endOf(block: Block): LanguageModelV3StreamPart {
    if (block.kind === PetalKind.text) return { type: "text-end", id: block.id };
    if (block.kind === PetalKind.sepal) return { type: "reasoning-end", id: block.id };
    return { type: "tool-input-end", id: block.id };
  }

  private *closeAll(): Iterable<LanguageModelV3StreamPart> {
    for (const block of this.blocks.values()) yield this.endOf(block);
    this.blocks.clear();
  }
}
