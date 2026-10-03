// Worker half of the openai-server hive — the inverse direction.
//
// The forager half (index.ts) puts an OpenAI-shaped HTTP surface in
// FRONT of humd: OpenAI wire in → chi:"prompt" out to hum's own
// workers. This module is the missing mirror: it accepts chi:"prompt"
// tones ROUTED TO THIS BEE as an advertised model, calls OpenAI's real
// API upstream, and streams the reply back as chi:"chunk"/"finish".
//
// So one process, one hive kind, one thrum connection serves BOTH
// directions — the hive is a hybrid bee: it both speaks OpenAI wire to
// hum's clients AND consumes OpenAI's API for hum's daemons. That's
// not exotic; VOCABULARY.md treats a hive as a kind slot and hybrid
// bees are legal. humd routes by modelId against the worker's
// advertised models, so if a caller asks for a model this hive
// advertises, humd forwards the prompt right back here — everything
// gets hummed.
//
// OpenAI's API is stateless per call — every request carries full
// history. So the worker is StatelessPerCall: each prompt is a fresh
// self-contained request; no pooling, sid continuity is irrelevant.

import type { ThrumClient, Tone } from "./thrum.ts";

// Upstream OpenAI-compatible API. OPENAI_API_BASE lets the worker
// point at any OpenAI-compatible server (runuz-hive principle:
// hardcode nothing). Default is the official API.
const API_BASE = process.env.OPENAI_API_BASE
  ?? "https://api.openai.com/v1";
const API_KEY = process.env.OPENAI_API_KEY
  ?? process.env.OPENAI_SERVER_API_KEY
  ?? "";

export class OpenAIWorker {
  private thrum: ThrumClient;

  constructor(thrum: ThrumClient) {
    this.thrum = thrum;
  }

  // Dispatch a chi:"prompt" tone routed to this bee.
  async handlePrompt(msg: Tone): Promise<void> {
    const sid = (msg.sid as string) ?? "";
    if (!sid) return;
    const model = (msg.modelId as string) ?? "";
    const content = (msg.content as string) ?? (msg.text as string) ?? "";
    const systemPrompt = msg.systemPrompt as string | undefined;
    const tools = msg.tools as Array<{ name: string; description?: string; inputSchema?: Record<string, unknown> }> | undefined;

    // OpenAI chat-completions body — the exact shape the API expects.
    const messages: Array<{ role: string; content: string }> = [];
    if (systemPrompt) messages.push({ role: "system", content: systemPrompt });
    messages.push({ role: "user", content });

    const body: Record<string, unknown> = {
      model,
      messages,
      stream: true,
    };
    if (Array.isArray(tools) && tools.length > 0) {
      // OpenAI function-call shape from thrum tool defs.
      body.tools = tools.map((t) => ({
        type: "function",
        function: {
          name: t.name,
          ...(t.description ? { description: t.description } : {}),
          parameters: t.inputSchema ?? {},
        },
      }));
    }

    try {
      const resp = await fetch(`${API_BASE}/chat/completions`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(API_KEY ? { "Authorization": `Bearer ${API_KEY}` } : {}),
        },
        body: JSON.stringify(body),
      });

      if (!resp.ok || !resp.body) {
        const detail = await resp.text().catch(() => "");
        this.error(sid, "upstream_error", `OpenAI API ${resp.status}: ${detail.slice(0, 500)}`);
        return;
      }

      // Stream SSE lines, fold deltas into chunk tones exactly like the
      // Rust WireListener's canonical surface. Each content block (text,
      // reasoning, tool-call) gets its own blockIdx; deltas within a block
      // carry that index, and a content_block_stop closes each block.
      const reader = resp.body.getReader();
      const decoder = new TextDecoder();
      let buf = "";
      let textBlock = 0;           // index of the text block
      let reasoningBlock = 0;      // index of the reasoning block
      let toolBlocks = 0;          // count of tool-call blocks opened
      // tool_calls arrive keyed by their own `index` (0..n-1); accumulate
      // each one's partial-arguments JSON across chunks.
      const toolArgs = new Map<number, { name: string; args: string }>();
      let inputTokens = 0;
      let outputTokens = 0;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let nl: number;
        while ((nl = buf.indexOf("\n")) >= 0) {
          const line = buf.slice(0, nl);
          buf = buf.slice(nl + 1);
          if (!line.startsWith("data:")) continue;
          const data = line.slice(5).trim();
          if (data === "[DONE]") continue;
          let ev: any;
          try { ev = JSON.parse(data); } catch { continue; }
          const choice = ev.choices?.[0];
          const delta = choice?.delta;
          if (delta?.content) {
            this.chunk(sid, "text_delta", { blockIdx: textBlock, delta: delta.content });
          }
          if (delta?.reasoning_content) {
            this.chunk(sid, "reasoning_delta", { blockIdx: reasoningBlock, delta: delta.reasoning_content });
          }
          if (Array.isArray(delta?.tool_calls)) {
            for (const tc of delta.tool_calls) {
              const idx = tc.index ?? toolBlocks;
              const slot = toolArgs.get(idx) ?? { name: "", args: "" };
              if (tc.function?.name) slot.name = tc.function.name;
              if (tc.function?.arguments) slot.args += tc.function.arguments;
              toolArgs.set(idx, slot);
              // A blockIdx per tool-call block, offset past text+reasoning.
              this.chunk(sid, "tool_input_delta", { blockIdx: idx, partialJson: slot.args });
            }
          }
          if (ev.usage) {
            inputTokens = ev.usage.prompt_tokens ?? 0;
            outputTokens = ev.usage.completion_tokens ?? 0;
          }
        }
      }
      buf += decoder.decode();
      this.chunk(sid, "content_block_stop", { blockIdx: textBlock });
      this.finish(sid, "stop", inputTokens, outputTokens);
    } catch (e) {
      this.error(sid, "upstream_error", (e as Error).message ?? "upstream failed");
    }
  }

  private chunk(sid: string, chunkType: string, payload: Record<string, unknown>): void {
    this.thrum.send({
      chi: "chunk",
      sid,
      chunkType,
      ...payload,
    } as Tone);
  }

  private finish(sid: string, reason: string, inputTokens: number, outputTokens: number): void {
    this.thrum.send({
      chi: "finish",
      sid,
      finishReason: reason,
      usage: { input_tokens: inputTokens, output_tokens: outputTokens, total_tokens: inputTokens + outputTokens },
    } as Tone);
  }

  private error(sid: string, code: string, message: string): void {
    this.thrum.send({
      chi: "error",
      sid,
      code,
      subtype: "error",
      message,
    } as Tone);
  }
}
