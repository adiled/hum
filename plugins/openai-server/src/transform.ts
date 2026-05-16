// hum daemon thrum events → OpenAI chat completion SSE chunks.
//
// Wire format the daemon emits to plugins (see humd/humd.ts onPetal):
//   { chi: "chunk",  sid, chunkType: <petal-type>, ...payload }
//   { chi: "finish", sid, finishReason, usage, providerMetadata }
//   { chi: "error",  sid, message }
// petal-type is one of: text_start | text_delta | reasoning_start |
// reasoning_delta | reasoning_end | tool_input_start | tool_input_delta |
// tool_call | tool_result | content_block_stop | stream_start.

export interface ToolCallSlot {
  index: number;
  id: string;
  name: string;
}

export class OpenAITranslator {
  private chunkId: string;
  private model: string;
  private created: number;
  private toolByCallId = new Map<string, ToolCallSlot>();
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

    if (type === "tool_input_start") {
      this.sawToolCalls = true;
      const toolCallId = (msg.toolCallId as string) ?? "";
      const toolName = (msg.toolName as string) ?? "";
      if (toolCallId && !this.toolByCallId.has(toolCallId)) {
        const slot: ToolCallSlot = { index: this.nextToolIndex++, id: toolCallId, name: toolName };
        this.toolByCallId.set(toolCallId, slot);
        this.seedRole(out);
        out.push(this.frame({
          tool_calls: [{
            index: slot.index,
            id: slot.id,
            type: "function",
            function: { name: slot.name, arguments: "" },
          }],
        }));
      }
      return out;
    }

    if (type === "tool_input_delta") {
      const partial = msg.partialJson as string | undefined;
      const toolCallId = (msg.toolCallId as string) ?? "";
      const slot = toolCallId ? this.toolByCallId.get(toolCallId) : undefined;
      if (typeof partial === "string" && slot) {
        out.push(this.frame({
          tool_calls: [{ index: slot.index, function: { arguments: partial } }],
        }));
      }
      return out;
    }

    if (type === "tool_call") {
      // The consolidated tool_call after deltas. If we didn't see input_start
      // (rare path) seed the tool here; otherwise emit the final arguments
      // string in case the deltas didn't flush completely.
      this.sawToolCalls = true;
      const toolCallId = (msg.toolCallId as string) ?? "";
      const toolName = (msg.toolName as string) ?? "";
      let slot = this.toolByCallId.get(toolCallId);
      if (!slot && toolCallId) {
        slot = { index: this.nextToolIndex++, id: toolCallId, name: toolName };
        this.toolByCallId.set(toolCallId, slot);
        this.seedRole(out);
        const inputStr = msg.input !== undefined
          ? (typeof msg.input === "string" ? msg.input : JSON.stringify(msg.input))
          : "";
        out.push(this.frame({
          tool_calls: [{
            index: slot.index,
            id: slot.id,
            type: "function",
            function: { name: slot.name, arguments: inputStr },
          }],
        }));
      }
      return out;
    }

    // text_start / reasoning_start / reasoning_end / content_block_stop /
    // tool_result / stream_start — no SSE frame needed.
    return out;
  }
}
