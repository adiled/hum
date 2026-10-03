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
// Tool loop: when OpenAI returns tool_calls, the worker emits
// chi:"tool-call" tones. humd routes them to the owning forager, which
// returns chi:"tool-result" keyed by callId. The worker feeds each
// result back into the OpenAI conversation and continues streaming —
// the model sees the tool output and proceeds. Same loop the Rust
// workers run.
//
// OpenAI's API is stateless per call — every request carries full
// history. So the worker is StatelessPerCall: each prompt is a fresh
// self-contained request; we keep the messages array per-sid so tool
// results can continue the conversation.

import type { ThrumClient, Tone } from "./thrum.ts";

// Upstream OpenAI-compatible API. OPENAI_API_BASE lets the worker
// point at any OpenAI-compatible server (runuz-hive principle:
// hardcode nothing). Default is the official API.
const API_BASE = process.env.OPENAI_API_BASE
  ?? "https://api.openai.com/v1";
const API_KEY = process.env.OPENAI_API_KEY
  ?? process.env.OPENAI_SERVER_API_KEY
  ?? "";

interface Session {
  sid: string;
  model: string;
  messages: Array<Record<string, unknown>>; // OpenAI message shape (role, content, tool_calls, tool_call_id)
  tools?: Array<{ name: string; description?: string; inputSchema?: Record<string, unknown> }>;
  // callId → accumulated tool-call info, awaiting chi:"tool-result"
  pending: Map<string, { name: string; args: string }>;
  // active stream generation token — cancels an in-flight upstream
  // stream so a tool-result can't race a stale response.
  generation: number;
  finished: boolean;
}

export class OpenAIWorker {
  private thrum: ThrumClient;
  private sessions = new Map<string, Session>();
  private static readonly MAX_SESSIONS = 32;

  constructor(thrum: ThrumClient) {
    this.thrum = thrum;
  }

  // Dispatch a chi:"prompt" tone routed to this bee.
  async handlePrompt(msg: Tone): Promise<void> {
    const sid = (msg.sid as string) ?? "";
    if (!sid) return;

    // Reuse the per-sid conversation so follow-up turns + tool results
    // continue the thread (statefulness is per-call, but the session
    // object is what lets tool-results resume).
    let sess = this.sessions.get(sid);
    if (!sess) {
      if (this.sessions.size >= OpenAIWorker.MAX_SESSIONS) {
        // Evict oldest (Map iterates insertion order).
        const oldest = this.sessions.keys().next().value as string;
        if (oldest) this.sessions.delete(oldest);
      }
      sess = {
        sid,
        model: (msg.modelId as string) ?? "",
        messages: [],
        pending: new Map(),
        generation: 0,
        finished: false,
      };
      this.sessions.set(sid, sess);
    }
    sess.model = (msg.modelId as string) ?? sess.model;
    const tools = msg.tools as Array<{ name: string; description?: string; inputSchema?: Record<string, unknown> }> | undefined;
    if (Array.isArray(tools) && tools.length > 0) sess.tools = tools;

    const systemPrompt = msg.systemPrompt as string | undefined;
    if (systemPrompt) sess.messages.push({ role: "system", content: systemPrompt });
    const content = (msg.content as string) ?? (msg.text as string) ?? "";
    if (content) sess.messages.push({ role: "user", content });

    await this.streamTurn(sess, sid);
  }

  // Handle chi:"tool-result" — feed the output back to the model and
  // continue the conversation.
  async handleToolResult(msg: Tone): Promise<void> {
    const sid = (msg.sid as string) ?? "";
    const callId = (msg.callId as string) ?? "";
    const sess = this.sessions.get(sid);
    if (!sess || !callId) return;

    const call = sess.pending.get(callId);
    if (!call) return;
    sess.pending.delete(callId);

    // The model's tool call becomes a tool message; the result is the
    // tool output (OpenAI wire: role "tool", tool_call_id, content).
    sess.messages.push({
      role: "tool",
      tool_call_id: callId,
      content: `[tool ${call.name}] ${(msg.output as string) ?? (msg.result as string) ?? ""}`,
    } as any);

    // Resume the stream with the tool output in the conversation.
    await this.streamTurn(sess, sid);
  }

  private async streamTurn(sess: Session, sid: string): Promise<void> {
    sess.finished = false;
    const gen = ++sess.generation;

    // OpenAI chat-completions body — the exact shape the API expects.
    const body: Record<string, unknown> = {
      model: sess.model,
      messages: sess.messages,
      stream: true,
    };
    if (sess.tools && sess.tools.length > 0) {
      // OpenAI function-call shape from thrum tool defs.
      body.tools = sess.tools.map((t) => ({
        type: "function",
        function: {
          name: t.name,
          ...(t.description ? { description: t.description } : {}),
          parameters: t.inputSchema ?? {},
        },
      }));
    }

    let resp: Response;
    try {
      resp = await fetch(`${API_BASE}/chat/completions`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(API_KEY ? { "Authorization": `Bearer ${API_KEY}` } : {}),
        },
        body: JSON.stringify(body),
      });
    } catch (e) {
      if (gen !== sess.generation) return; // superseded by a newer turn
      this.error(sid, "upstream_error", (e as Error).message ?? "upstream failed");
      return;
    }

    if (!resp.ok || !resp.body) {
      if (gen !== sess.generation) return;
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
    let textBlock = 0;
    let reasoningBlock = 0;
    // tool_calls arrive keyed by their own `index` (0..n-1); accumulate
    // each one's partial-arguments JSON across chunks.
    const toolArgs = new Map<number, { name: string; args: string; callId: string }>();
    let inputTokens = 0;
    let outputTokens = 0;

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        if (gen !== sess.generation) { reader.cancel(); return; }
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
              const idx = tc.index ?? 0;
              const slot = toolArgs.get(idx) ?? { name: "", args: "", callId: tc.id ?? "" };
              if (tc.function?.name) slot.name = tc.function.name;
              if (tc.function?.arguments) slot.args += tc.function.arguments;
              if (tc.id) slot.callId = tc.id;
              toolArgs.set(idx, slot);
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
    } catch (e) {
      if (gen !== sess.generation) return;
      this.error(sid, "upstream_error", (e as Error).message ?? "upstream failed");
      return;
    }

    if (gen !== sess.generation) return;

    // If the model emitted tool calls, don't finish — emit chi:"tool-call"
    // tones so humd routes them to foragers, and wait for tool-results.
    if (toolArgs.size > 0) {
      // Record the assistant message with its tool_calls array — OpenAI
      // wire requires the continuation's tool messages to reference the
      // call ids from this message.
      const calls: Array<{ id: string; type: string; function: { name: string; arguments: string } }> = [];
      for (const [, tc] of toolArgs) {
        const callId = tc.callId || `call_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`;
        calls.push({ id: callId, type: "function", function: { name: tc.name, arguments: tc.args } });
        sess.pending.set(callId, { name: tc.name, args: tc.args });
        this.thrum.send({
          chi: "tool-call",
          sid,
          callId,
          toolName: tc.name,
          args: tc.args,
        } as Tone);
      }
      sess.messages.push({
        role: "assistant",
        content: null,
        tool_calls: calls,
      } as any);
      return;
    }

    // Plain text response — close the block and finish.
    this.chunk(sid, "content_block_stop", { blockIdx: textBlock });
    this.finish(sid, "stop", inputTokens, outputTokens);
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

  // chi:"cancel" — bump generation so any in-flight upstream stream
  // for this sid is superseded and stops emitting.
  cancel(msg: Tone): void {
    const sid = (msg.sid as string) ?? "";
    const sess = this.sessions.get(sid);
    if (!sess) return;
    sess.generation++;
    sess.finished = true;
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
