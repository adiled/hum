// hum daemon thrum events → OpenAI chat completion SSE chunks.
//
// Wire format the daemon emits to plugins (see humd/humd.ts onPetal):
//   { chi: "chunk",  sid, chunkType: <petal-type>, ...payload }
//   { chi: "finish", sid, finishReason, usage, providerMetadata }
//   { chi: "error",  sid, message }
// petal-type is one of: text_start | text_delta | reasoning_start |
// reasoning_delta | reasoning_end | tool_input_start | tool_input_delta |
// tool_call | tool_result | content_block_stop | stream_start.

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
    // humd's MCP server, brokered FS/bash, etc. We deliberately
    // suppress them. Emitting `tool_calls` deltas here tells the
    // OpenAI client to execute the tool itself, but the perch already
    // did. OC's session run-loop responds by firing a continuation
    // request that has nothing to say, producing an "empty response"
    // and stalling the next user turn. Externally-declared tools (the
    // ones the client sent on body.tools) flow through chi:"tool-call"
    // above; that path is what should actually trigger client-side
    // tool execution.
    //
    // text_start / reasoning_start / reasoning_end / content_block_stop /
    // tool_result / stream_start — no SSE frame needed either.
    return out;
  }
}
