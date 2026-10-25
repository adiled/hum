// A bloom's tones → OpenAI chat completion SSE chunks.
//
// What the bee hears for a sid:
//   { chi: "unfurl", sid, petal, kind, text | sap | callId, name | closed }
//   { chi: "tendril-reach", sid, callId, name, args }
//   { chi: "wilt", sid, reason, nectar }
//   { chi: "error", sid, message }
//
// A text petal becomes `content` deltas and a sepal `reasoning_content`
// deltas. A tendril petal that unfurls is a tool the worker runs in
// itself: its name and sap stream as `tool_calls` deltas and the stream
// still finishes `stop`, the honest signal that the call happened
// in-server and the model kept going. A client that gates its tool loop
// on finish_reason, as OpenAI specifies, renders it; one that fires on
// any tool_calls delta stalls, and that is the client's bug. A tendril
// that reaches is a tool the caller declared: the model is parked
// awaiting its result, so it is a `tool_calls` frame, finish
// `tool_calls`, and the end of the stream, and the caller posts the
// result as a role:tool message. One that already streamed as a petal
// is not repeated. A tool named `mcp__<server>__<Tool>` (the MCP
// convention) is shown by its lowercased leaf, the name a client knows
// it by.

import { PetalKind, type Tone } from "../../../thrum-clients/ts/index.ts";

function normalizeToolName(name: string): string {
  const m = name.match(/^mcp__[^_]+__(.+)$/);
  const leaf = m ? m[1] : name;
  return leaf.toLowerCase();
}

type Nectar = { input_tokens?: number; output_tokens?: number; cache_read_input_tokens?: number; cache_creation_input_tokens?: number };

export class OpenAITranslator {
  private chunkId: string;
  private model: string;
  private created: number;
  private nextToolIndex = 0;
  private sawToolCalls = false;
  private firstChunk = true;
  private includeUsage: boolean;
  private tendrilPetals = new Map<number, { index: number; callId: string }>();

  constructor(chunkId: string, model: string, includeUsage = true) {
    this.chunkId = chunkId;
    this.model = model;
    this.created = Math.floor(Date.now() / 1000);
    this.includeUsage = includeUsage;
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

  private streamed(callId: string): boolean {
    for (const t of this.tendrilPetals.values()) if (t.callId === callId) return true;
    return false;
  }

  ingest(msg: Tone): string[] {
    const out: string[] = [];

    if (msg.chi === "tendril-reach") {
      this.sawToolCalls = true;
      this.seedRole(out);
      if (!this.streamed(msg.callId)) {
        const args = msg.args !== undefined
          ? (typeof msg.args === "string" ? msg.args : JSON.stringify(msg.args))
          : "{}";
        out.push(this.frame({
          tool_calls: [{
            index: this.nextToolIndex++,
            id: msg.callId,
            type: "function",
            function: { name: msg.name, arguments: args },
          }],
        }));
      }
      out.push(this.frame({}, "tool_calls"));
      out.push("data: [DONE]\n\n");
      return out;
    }

    if (msg.chi === "error") {
      const message = msg.message || "unknown error";
      out.push(this.frame({ content: `\n[error] ${message}\n` }));
      out.push(this.frame({}, "stop"));
      out.push("data: [DONE]\n\n");
      return out;
    }

    if (msg.chi === "wilt") {
      out.push(this.frame({}, this.sawToolCalls ? "tool_calls" : "stop"));
      const nectar = msg.nectar as Nectar | undefined;
      if (nectar && this.includeUsage) {
        const prompt = (nectar.input_tokens ?? 0) + (nectar.cache_read_input_tokens ?? 0) + (nectar.cache_creation_input_tokens ?? 0);
        const completion = nectar.output_tokens ?? 0;
        out.push(`data: ${JSON.stringify({
          id: this.chunkId,
          object: "chat.completion.chunk",
          created: this.created,
          model: this.model,
          choices: [],
          usage: { prompt_tokens: prompt, completion_tokens: completion, total_tokens: prompt + completion },
        })}\n\n`);
      }
      out.push("data: [DONE]\n\n");
      return out;
    }

    if (msg.chi !== "unfurl" || msg.closed) return out;

    if (msg.kind === PetalKind.text && msg.text) {
      this.seedRole(out);
      out.push(this.frame({ content: msg.text }));
      return out;
    }

    if (msg.kind === PetalKind.sepal && msg.text) {
      this.seedRole(out);
      out.push(this.frame({ reasoning_content: msg.text }));
      return out;
    }

    if (msg.kind === PetalKind.tendril) {
      let tendril = this.tendrilPetals.get(msg.petal);
      if (!tendril && msg.name) {
        tendril = { index: this.nextToolIndex++, callId: msg.callId ?? "" };
        this.tendrilPetals.set(msg.petal, tendril);
        this.seedRole(out);
        out.push(this.frame({
          tool_calls: [{ index: tendril.index, id: tendril.callId, type: "function", function: { name: normalizeToolName(msg.name), arguments: "" } }],
        }));
      }
      if (tendril && msg.sap) {
        out.push(this.frame({ tool_calls: [{ index: tendril.index, function: { arguments: msg.sap } }] }));
      }
    }
    return out;
  }
}
