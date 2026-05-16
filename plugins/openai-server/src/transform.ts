// petal-cell events (hum's daemon-side dispatch) → OpenAI chat completion
// SSE chunks. Stateful per request because tool_calls have to track their
// emitted index across deltas, and finish_reason has to know whether the
// turn ended with a tool call or plain stop.

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
      choices: [{
        index: 0,
        delta,
        finish_reason: finish ?? null,
      }],
    };
    return `data: ${JSON.stringify(chunk)}\n\n`;
  }

  // Called for every incoming hum tone. Returns 0..N SSE frames to emit.
  // Returns the special string "DONE" when [DONE] sentinel should be sent
  // and the connection closed.
  ingest(msg: Record<string, unknown>): string[] {
    const chi = msg.chi as string | undefined;
    const out: string[] = [];

    if (chi !== "petal-cell" && chi !== "petal" && msg.type === undefined) {
      // Non-content tone (pulse, echo, log) — ignore for the SSE stream.
      return out;
    }

    // The first emitted chunk needs role:"assistant" to satisfy OpenAI shape.
    const seedRole = (): void => {
      if (this.firstChunk) {
        out.push(this.frame({ role: "assistant" }));
        this.firstChunk = false;
      }
    };

    const type = (msg.type as string | undefined) ?? (msg.petal as string | undefined);
    const payload = (msg.payload as Record<string, unknown> | undefined) ?? msg;

    if (type === "text_delta") {
      const delta = payload.delta as string | undefined;
      if (typeof delta === "string" && delta.length > 0) {
        seedRole();
        out.push(this.frame({ content: delta }));
      }
      return out;
    }

    if (type === "reasoning_delta") {
      const delta = payload.delta as string | undefined;
      if (typeof delta === "string" && delta.length > 0) {
        seedRole();
        out.push(this.frame({ reasoning_content: delta }));
      }
      return out;
    }

    if (type === "tool_input_start" || type === "tool_call") {
      this.sawToolCalls = true;
      const toolCallId = (payload.toolCallId as string) ?? (payload.id as string) ?? "";
      const toolName = (payload.toolName as string) ?? (payload.name as string) ?? "";
      let slot = this.toolByCallId.get(toolCallId);
      if (!slot) {
        slot = { index: this.nextToolIndex++, id: toolCallId, name: toolName };
        this.toolByCallId.set(toolCallId, slot);
        seedRole();
        out.push(this.frame({
          tool_calls: [{
            index: slot.index,
            id: slot.id,
            type: "function",
            function: { name: slot.name, arguments: "" },
          }],
        }));
      }
      // tool_call (the consolidated one) carries the full input.
      if (type === "tool_call" && payload.input !== undefined) {
        const inputStr = typeof payload.input === "string"
          ? payload.input
          : JSON.stringify(payload.input ?? {});
        out.push(this.frame({
          tool_calls: [{ index: slot.index, function: { arguments: inputStr } }],
        }));
      }
      return out;
    }

    if (type === "tool_input_delta") {
      const partial = payload.partialJson as string | undefined;
      const toolCallId = (payload.toolCallId as string) ?? "";
      const slot = toolCallId ? this.toolByCallId.get(toolCallId) : undefined;
      if (typeof partial === "string" && slot) {
        out.push(this.frame({
          tool_calls: [{ index: slot.index, function: { arguments: partial } }],
        }));
      }
      return out;
    }

    if (type === "finish" || chi === "finish") {
      const finishReason = this.sawToolCalls ? "tool_calls" : "stop";
      out.push(this.frame({}, finishReason));
      out.push("data: [DONE]\n\n");
      return out;
    }

    return out;
  }
}
