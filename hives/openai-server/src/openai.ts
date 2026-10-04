import type { ThrumClient, Tone } from "./thrum.ts";

const API_BASE = process.env.OPENAI_API_BASE
  ?? "https://api.openai.com/v1";
const API_KEY = process.env.OPENAI_API_KEY
  ?? process.env.OPENAI_SERVER_API_KEY
  ?? "";

interface ToolDef {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
}

function functionTools(tools: ToolDef[]): Array<Record<string, unknown>> {
  return tools.map((t) => ({
    type: "function",
    function: {
      name: t.name,
      ...(t.description ? { description: t.description } : {}),
      parameters: t.inputSchema ?? {},
    },
  }));
}

function toolMessage(name: string, callId: string, output: string): Record<string, unknown> {
  return { role: "tool", tool_call_id: callId, content: `[tool ${name}] ${output}` };
}

interface Session {
  sid: string;
  model: string;
  messages: Array<Record<string, unknown>>;
  tools?: ToolDef[];
  pending: Map<string, { name: string; args: string }>;
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

  async handlePrompt(msg: Tone): Promise<void> {
    const sid = (msg.sid as string) ?? "";
    if (!sid) return;

    const sess = this.sessionFor(sid);
    sess.model = (msg.modelId as string) ?? sess.model;
    const tools = msg.tools as ToolDef[] | undefined;
    if (Array.isArray(tools) && tools.length > 0) sess.tools = tools;

    const systemPrompt = msg.systemPrompt as string | undefined;
    if (systemPrompt) sess.messages.push({ role: "system", content: systemPrompt });
    const content = (msg.content as string) ?? (msg.text as string) ?? "";
    if (content) sess.messages.push({ role: "user", content });

    await this.streamTurn(sess, sid);
  }

  async handleToolResult(msg: Tone): Promise<void> {
    const sid = (msg.sid as string) ?? "";
    const callId = (msg.callId as string) ?? "";
    const sess = this.sessions.get(sid);
    if (!sess || !callId) return;

    const call = sess.pending.get(callId);
    if (!call) return;
    sess.pending.delete(callId);
    const output = (msg.output as string) ?? (msg.result as string) ?? "";

    sess.messages.push(toolMessage(call.name, callId, output));
    await this.streamTurn(sess, sid);
  }

  private async streamTurn(sess: Session, sid: string): Promise<void> {
    sess.finished = false;
    const gen = ++sess.generation;

    const body: Record<string, unknown> = {
      model: sess.model,
      messages: sess.messages,
      stream: true,
    };
    if (sess.tools && sess.tools.length > 0) {
      body.tools = functionTools(sess.tools);
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

    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    let textBlock = 0;
    let reasoningBlock = 0;
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

    if (toolArgs.size > 0) {
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

    this.chunk(sid, "content_block_stop", { blockIdx: textBlock });
    this.finish(sid, "stop", inputTokens, outputTokens);
  }

  private sessionFor(sid: string): Session {
    const existing = this.sessions.get(sid);
    if (existing) return existing;
    if (this.sessions.size >= OpenAIWorker.MAX_SESSIONS) {
      const oldest = this.sessions.keys().next().value as string | undefined;
      if (oldest !== undefined) this.sessions.delete(oldest);
    }
    const fresh: Session = {
      sid,
      model: "",
      messages: [],
      pending: new Map(),
      generation: 0,
      finished: false,
    };
    this.sessions.set(sid, fresh);
    return fresh;
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
