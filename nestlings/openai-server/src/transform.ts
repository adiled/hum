// hum daemon thrum events → OpenAI chat completion SSE chunks.
//
// Thrum's chi vocabulary is richer than OpenAI's wire — we translate
// the full event surface we can express in the target format, and
// let consumers (AI-aware clients, dashboards, raw curl) extract what
// they understand. Clients with a faithful tool_calls implementation
// will render perch-internal tool invocations naturally; clients with
// run-loops that misinterpret tool_calls deltas as actionable
// (regardless of finish_reason) will see stalls — that's a consumer
// bug, not a translator one.
//
// Wire format the daemon emits to plugins (see humd/humd.ts onPetal):
//   { chi: "chunk",  sid, chunkType: <petal-type>, ...payload }
//   { chi: "finish", sid, finishReason, usage, providerMetadata }
//   { chi: "error",  sid, message }
// petal-type is one of: text_start | text_delta | reasoning_start |
// reasoning_delta | reasoning_end | tool_input_start | tool_input_delta |
// tool_call | tool_result | content_block_stop | stream_start.

// Upstream tools are namespaced `mcp__<server>__<Tool>` (MCP
// convention). Strip the prefix and lowercase the leaf so clients
// render with their idiomatic tool vocabulary. Pure namespacing
// normalization — no perch knowledge.
function normalizeToolName(name: string): string {
  const m = name.match(/^mcp__[^_]+__(.+)$/);
  const leaf = m ? m[1] : name;
  return leaf.toLowerCase();
}

export class OpenAITranslator {
  private chunkId: string;
  private model: string;
  private created: number;
  private nextToolIndex = 0;
  private sawToolCalls = false;
  private firstChunk = true;

  constructor(chunkId: string, model: string) {
    this.chunkId = chunkId;
    this.model = model;
    this.created = Math.floor(Date.now() / 1000);
  }

  private frame(delta: Record<string, unknown>, finish?: string | null): string {
    const chunk = {
      id: this.chunkId,
      object: "chat.completion.chunk",
      created: this.created,
      model: this.model,
      choices: [{ index: 0, delta, finish_reason: finish ?? null }],
    };
    return `data: ${JSON.stringify(chunk)}\n\n`;
  }

  private seedRole(out: string[]): void {
    if (this.firstChunk) {
      out.push(this.frame({ role: "assistant" }));
      this.firstChunk = false;
    }
  }

  ingest(msg: Record<string, unknown>): string[] {
    const chi = msg.chi as string | undefined;
    const out: string[] = [];

    if (chi === "tool-call") {
      // Daemon forwarded a nestler-declared tool call — the model is parked
      // awaiting a tool-result. Emit an OpenAI tool_calls frame, finish with
      // "tool_calls", close the SSE. The client will execute and POST a
      // continuation with role:tool messages.
      this.sawToolCalls = true;
      const callId = (msg.callId as string) ?? "";
      const name = (msg.name as string) ?? "";
      const args = msg.args !== undefined
        ? (typeof msg.args === "string" ? msg.args : JSON.stringify(msg.args))
        : "{}";
      this.seedRole(out);
      out.push(this.frame({
        tool_calls: [{
          index: this.nextToolIndex++,
          id: callId,
          type: "function",
          function: { name, arguments: args },
        }],
      }));
      out.push(this.frame({}, "tool_calls"));
      out.push("data: [DONE]\n\n");
      return out;
    }

    if (chi === "error") {
      const message = (msg.message as string) ?? "unknown error";
      out.push(this.frame({ content: `\n[error] ${message}\n` }));
      out.push(this.frame({}, "stop"));
      out.push("data: [DONE]\n\n");
      return out;
    }

    if (chi === "finish") {
      const finishReason = this.sawToolCalls ? "tool_calls" : "stop";
      // OpenAI's optional usage field on final chunk
      const usage = msg.usage as Record<string, number> | undefined;
      const finalDelta: Record<string, unknown> = {};
      out.push(this.frame(finalDelta, finishReason));
      if (usage) {
        const u = {
          prompt_tokens: (usage.input_tokens ?? 0) + (usage.cache_read_input_tokens ?? 0) + (usage.cache_creation_input_tokens ?? 0),
          completion_tokens: usage.output_tokens ?? 0,
          total_tokens: 0,
        };
        u.total_tokens = u.prompt_tokens + u.completion_tokens;
        out.push(`data: ${JSON.stringify({
          id: this.chunkId,
          object: "chat.completion.chunk",
          created: this.created,
          model: this.model,
          choices: [],
          usage: u,
        })}\n\n`);
      }
      out.push("data: [DONE]\n\n");
      return out;
    }

    if (chi !== "chunk") return out;

    const type = msg.chunkType as string | undefined;
    if (!type) return out;

    if (type === "text_delta") {
      const delta = msg.delta as string | undefined;
      if (typeof delta === "string" && delta.length > 0) {
        this.seedRole(out);
        out.push(this.frame({ content: delta }));
      }
      return out;
    }

    if (type === "reasoning_delta") {
      const delta = msg.delta as string | undefined;
      if (typeof delta === "string" && delta.length > 0) {
        this.seedRole(out);
        out.push(this.frame({ reasoning_content: delta }));
      }
      return out;
    }

    // Chunk-level tool events (tool_input_start / tool_input_delta /
    // tool_call) come from the perch executing an in-process tool —
    // humd's MCP server, brokered FS/bash, etc. Surface them as
    // OpenAI tool_calls deltas so AI-aware clients can render the
    // invocation. We do NOT set finish_reason="tool_calls" because the
    // perch already executed the tool and continues generating;
    // emitting both the tool_calls delta and subsequent text deltas
    // within a single finish_reason="stop" stream is the honest
    // signal: "this tool happened in-server, here's the call shape,
    // and here's what the model said next." Clients that gate their
    // tool-execution loop on finish_reason (per OpenAI's spec) get
    // perfect rendering. Clients that fire continuation on tool_calls
    // presence regardless of finish_reason will stall — that's a
    // consumer bug to fix at the consumer.
    if (type === "tool_input_start") {
      const id = (msg.toolCallId as string) ?? "";
      const name = normalizeToolName((msg.toolName as string) ?? "");
      const index = this.nextToolIndex++;
      this.seedRole(out);
      out.push(this.frame({
        tool_calls: [{ index, id, type: "function", function: { name, arguments: "" } }],
      }));
      this.toolStreamIndex.set(id, index);
      this.openToolId = id;
      return out;
    }

    if (type === "tool_input_delta") {
      const partial = (msg.partialJson as string) ?? "";
      if (partial.length === 0) return out;
      const id = this.openToolId;
      const index = id ? this.toolStreamIndex.get(id) : undefined;
      if (index === undefined) return out;
      out.push(this.frame({
        tool_calls: [{ index, function: { arguments: partial } }],
      }));
      return out;
    }

    if (type === "tool_call") {
      // Args fully received — perch executes upstream. No frame: UI
      // already shows the call via the start + delta sequence.
      return out;
    }

    // text_start / reasoning_start / reasoning_end / content_block_stop /
    // tool_result / stream_start — no SSE frame needed.
    return out;
  }

  // Tracks the index assigned to each in-flight tool-call id so
  // subsequent tool_input_delta tones (which don't repeat the id)
  // land on the right slot.
  private toolStreamIndex = new Map<string, number>();
  private openToolId: string = "";
}
