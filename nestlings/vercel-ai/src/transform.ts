// Translate hum's thrum tones into Vercel AI SDK v3 stream parts.
//
// Daemon→nestler tones:
//   { chi: "chunk", chunkType, ... }      — claude's stream, partwise
//   { chi: "tool-call", name, args, callId } — nestler-declared tool dispatch
//   { chi: "finish", finishReason, usage } — turn complete
//   { chi: "error", message }              — turn aborted
//
// Anything else (perf-mark, pulse, breath, echo, drone-retrofit, …) is
// out-of-band signalling for richer consumers; this pure provider ignores
// it.

import type {
  LanguageModelV3StreamPart,
  LanguageModelV3FinishReason,
  LanguageModelV3Usage,
} from "@ai-sdk/provider";

type Tone = Record<string, unknown>;

function emptyUsage(): LanguageModelV3Usage {
  return {
    inputTokens: { total: 0, noCache: 0, cacheRead: 0, cacheWrite: 0 },
    outputTokens: { total: 0, text: 0, reasoning: 0 },
  };
}

function mapFinish(raw: string | undefined): LanguageModelV3FinishReason {
  const r = (raw ?? "stop").toLowerCase();
  const unified: LanguageModelV3FinishReason["unified"] =
      r === "stop" || r === "end_turn" ? "stop"
    : r === "length" || r === "max_tokens" ? "length"
    : r === "tool_calls" || r === "tool-use" || r === "tool_use" ? "tool-calls"
    : r === "content_filter" ? "content-filter"
    : r === "error" ? "error"
    : "other";
  return { unified, raw: r };
}

function mapUsage(u: Record<string, number> | undefined): LanguageModelV3Usage {
  if (!u) return emptyUsage();
  const inputTotal = (u.input_tokens ?? 0) + (u.cache_read_input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0);
  return {
    inputTokens: {
      total: inputTotal,
      noCache: u.input_tokens ?? 0,
      cacheRead: u.cache_read_input_tokens ?? 0,
      cacheWrite: u.cache_creation_input_tokens ?? 0,
    },
    outputTokens: {
      total: u.output_tokens ?? 0,
      text: u.output_tokens ?? 0,
      reasoning: 0,
    },
  };
}

// One translator per doStream call. Tracks block ids and finish state so
// we never emit an end without its matching start.
export class HumTranslator {
  private textOpen = false;
  private textId = "";
  private reasoningOpen = false;
  private reasoningId = "";
  // toolCallId → toolName, for tool-call frames that arrive after tool-input-*
  private openTools = new Map<string, string>();
  private finished = false;

  *ingest(msg: Tone): Iterable<LanguageModelV3StreamPart> {
    if (this.finished) return;
    const chi = msg.chi as string | undefined;

    if (chi === "error") {
      yield* this.closeOpenBlocks();
      yield { type: "error", error: new Error((msg.message as string) ?? "thrum error") };
      yield { type: "finish", finishReason: { unified: "error", raw: "error" }, usage: emptyUsage() };
      this.finished = true;
      return;
    }

    if (chi === "finish") {
      yield* this.closeOpenBlocks();
      yield {
        type: "finish",
        finishReason: mapFinish(msg.finishReason as string | undefined),
        usage: mapUsage(msg.usage as Record<string, number> | undefined),
      };
      this.finished = true;
      return;
    }

    if (chi === "tool-call") {
      // Nestler-declared tool. Emit a self-contained input-start/tool-call
      // pair so consumers see a complete call. Daemon awaits a chi:"tool-result"
      // tone the consumer is expected to deliver out-of-band.
      yield* this.closeOpenBlocks();
      const callId = (msg.callId as string) ?? "";
      const name = (msg.name as string) ?? "";
      const input = msg.args !== undefined
        ? (typeof msg.args === "string" ? msg.args : JSON.stringify(msg.args))
        : "{}";
      yield { type: "tool-input-start", id: callId, toolName: name };
      yield { type: "tool-input-end", id: callId };
      yield { type: "tool-call", toolCallId: callId, toolName: name, input };
      return;
    }

    if (chi !== "chunk") return;
    const ct = msg.chunkType as string | undefined;
    if (!ct) return;

    if (ct === "text_start" || (ct === "text_delta" && !this.textOpen)) {
      if (!this.textOpen) {
        this.textId = `t${Date.now()}`;
        this.textOpen = true;
        yield { type: "text-start", id: this.textId };
      }
    }
    if (ct === "text_delta") {
      const delta = msg.delta as string | undefined;
      if (typeof delta === "string" && delta.length > 0) {
        yield { type: "text-delta", id: this.textId, delta };
      }
    }

    if (ct === "reasoning_start" || (ct === "reasoning_delta" && !this.reasoningOpen)) {
      if (!this.reasoningOpen) {
        this.reasoningId = `r${Date.now()}`;
        this.reasoningOpen = true;
        yield { type: "reasoning-start", id: this.reasoningId };
      }
    }
    if (ct === "reasoning_delta") {
      const delta = msg.delta as string | undefined;
      if (typeof delta === "string" && delta.length > 0) {
        yield { type: "reasoning-delta", id: this.reasoningId, delta };
      }
    }
    if (ct === "reasoning_end" && this.reasoningOpen) {
      yield { type: "reasoning-end", id: this.reasoningId };
      this.reasoningOpen = false;
    }

    if (ct === "tool_input_start") {
      yield* this.closeTextish();
      const id = (msg.toolCallId as string) ?? "";
      const toolName = (msg.toolName as string) ?? "";
      if (id) {
        this.openTools.set(id, toolName);
        yield { type: "tool-input-start", id, toolName };
      }
    }
    if (ct === "tool_input_delta") {
      const id = (msg.toolCallId as string) ?? "";
      const delta = msg.partialJson as string | undefined;
      if (id && typeof delta === "string") {
        yield { type: "tool-input-delta", id, delta };
      }
    }
    if (ct === "tool_call") {
      yield* this.closeTextish();
      const id = (msg.toolCallId as string) ?? "";
      const toolName = (msg.toolName as string) ?? this.openTools.get(id) ?? "";
      if (!this.openTools.has(id)) yield { type: "tool-input-start", id, toolName };
      this.openTools.delete(id);
      const input = msg.input !== undefined
        ? (typeof msg.input === "string" ? msg.input : JSON.stringify(msg.input))
        : "{}";
      yield { type: "tool-input-end", id };
      yield { type: "tool-call", toolCallId: id, toolName, input };
    }
  }

  // Forced close — caller hit abort/disconnect before a chi:"finish".
  *flush(): Iterable<LanguageModelV3StreamPart> {
    if (this.finished) return;
    yield* this.closeOpenBlocks();
    yield { type: "finish", finishReason: { unified: "other", raw: "aborted" }, usage: emptyUsage() };
    this.finished = true;
  }

  private *closeTextish(): Iterable<LanguageModelV3StreamPart> {
    if (this.textOpen) { yield { type: "text-end", id: this.textId }; this.textOpen = false; }
    if (this.reasoningOpen) { yield { type: "reasoning-end", id: this.reasoningId }; this.reasoningOpen = false; }
  }

  private *closeOpenBlocks(): Iterable<LanguageModelV3StreamPart> {
    yield* this.closeTextish();
    for (const [id, _name] of this.openTools) {
      yield { type: "tool-input-end", id };
    }
    this.openTools.clear();
  }
}
