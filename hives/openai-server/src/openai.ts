import { PetalKind, type Bee, type Petal, type PetalKindT, type Tone } from "../../../thrum-clients/ts/index.ts";

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

interface Sigil {
  sid: string;
  model: string;
  messages: Array<Record<string, unknown>>;
  tools?: ToolDef[];
  pending: Map<string, { name: string; args: string }>;
  generation: number;
  wilted: boolean;
}

export class OpenAINurse {
  private thrum: Bee;
  private sigils = new Map<string, Sigil>();
  private static readonly MAX_SIGILS = 32;
  private discovered: string[] = [];

  constructor(thrum: Bee) {
    this.thrum = thrum;
  }

  async discover(): Promise<void> {
    let ids: string[] = [];
    try {
      const resp = await fetch(`${API_BASE}/models`, {
        headers: API_KEY ? { "Authorization": `Bearer ${API_KEY}` } : {},
      });
      if (resp.ok) {
        const body = await resp.json() as { data?: Array<{ id?: unknown }> };
        if (Array.isArray(body.data)) {
          ids = body.data
            .map(d => typeof d?.id === "string" ? d.id : "")
            .filter(s => s.length > 0);
        }
      }
    } catch {}
    this.discovered = ids;
  }

  models(): string[] {
    return this.discovered.slice();
  }

  async handlePrompt(msg: Tone<"prompt">): Promise<void> {
    const sid = msg.sid ?? "";
    if (!sid) return;

    const modelId = msg.modelId ?? "";
    if (!this.discovered.includes(modelId)) {
      this.error(sid, "unknown_model", `nurse does not serve model '${modelId}'`);
      return;
    }

    const sigil = this.sigilFor(sid);
    sigil.model = modelId;
    const tools = msg.tools as ToolDef[] | undefined;
    if (Array.isArray(tools) && tools.length > 0) sigil.tools = tools;

    const systemPrompt = msg.systemPrompt;
    if (systemPrompt) sigil.messages.push({ role: "system", content: systemPrompt });
    const content = petalText(msg.content) || msg.text || "";
    if (content) sigil.messages.push({ role: "user", content });

    await this.grow(sigil, sid);
  }

  async handleTendrilResult(msg: Tone<"tendril-result">): Promise<void> {
    const sid = msg.sid ?? "";
    const callId = msg.callId;
    const sigil = this.sigils.get(sid);
    if (!sigil || !callId) return;

    const call = sigil.pending.get(callId);
    if (!call) return;
    sigil.pending.delete(callId);
    const grasped = msg.output ?? msg.result ?? "";
    const output = typeof grasped === "string" ? grasped : JSON.stringify(grasped);

    sigil.messages.push(toolMessage(call.name, callId, output));
    await this.grow(sigil, sid);
  }

  private async grow(sigil: Sigil, sid: string): Promise<void> {
    sigil.wilted = false;
    const gen = ++sigil.generation;

    const body: Record<string, unknown> = {
      model: sigil.model,
      messages: sigil.messages,
      stream: true,
      stream_options: { include_usage: true },
    };
    if (sigil.tools && sigil.tools.length > 0) {
      body.tools = functionTools(sigil.tools);
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
      if (gen !== sigil.generation) return;
      this.error(sid, "upstream_error", (e as Error).message ?? "upstream failed");
      return;
    }

    if (!resp.ok || !resp.body) {
      if (gen !== sigil.generation) return;
      const detail = await resp.text().catch(() => "");
      this.error(sid, "upstream_error", `OpenAI API ${resp.status}: ${detail.slice(0, 500)}`);
      return;
    }

    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    const petals = new Map<string, { petal: number; kind: PetalKindT }>();
    const petalOf = (key: string, kind: PetalKindT): number => {
      const known = petals.get(key);
      if (known) return known.petal;
      const petal = petals.size;
      petals.set(key, { petal, kind });
      return petal;
    };
    const tendrils = new Map<number, { name: string; args: string; callId: string }>();
    let inputTokens = 0;
    let outputTokens = 0;

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        if (gen !== sigil.generation) { reader.cancel(); return; }
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
          const delta = ev.choices?.[0]?.delta;
          if (delta?.reasoning_content) {
            const petal = petalOf("sepal", PetalKind.sepal);
            this.thrum.send({ chi: "unfurl", sid, petal, kind: PetalKind.sepal, text: delta.reasoning_content });
          }
          if (delta?.content) {
            const petal = petalOf("text", PetalKind.text);
            this.thrum.send({ chi: "unfurl", sid, petal, kind: PetalKind.text, text: delta.content });
          }
          if (Array.isArray(delta?.tool_calls)) {
            for (const tc of delta.tool_calls) {
              const idx = tc.index ?? 0;
              const tendril = tendrils.get(idx) ?? { name: "", args: "", callId: tc.id ?? "" };
              if (tc.function?.name) tendril.name = tc.function.name;
              if (tc.function?.arguments) tendril.args += tc.function.arguments;
              if (tc.id) tendril.callId = tc.id;
              tendrils.set(idx, tendril);
              const petal = petalOf(`tendril:${idx}`, PetalKind.tendril);
              this.thrum.send({
                chi: "unfurl",
                sid,
                petal,
                kind: PetalKind.tendril,
                ...(tc.id ? { callId: tc.id } : {}),
                ...(tc.function?.name ? { name: tc.function.name } : {}),
                ...(tc.function?.arguments ? { sap: tc.function.arguments } : {}),
              });
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
      if (gen !== sigil.generation) return;
      this.error(sid, "upstream_error", (e as Error).message ?? "upstream failed");
      return;
    }

    if (gen !== sigil.generation) return;

    for (const { petal, kind } of petals.values()) {
      this.thrum.send({ chi: "unfurl", sid, petal, kind, closed: true });
    }

    if (tendrils.size > 0) {
      const calls: Array<{ id: string; type: string; function: { name: string; arguments: string } }> = [];
      for (const [, tendril] of tendrils) {
        const callId = tendril.callId || `call_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`;
        calls.push({ id: callId, type: "function", function: { name: tendril.name, arguments: tendril.args } });
        sigil.pending.set(callId, { name: tendril.name, args: tendril.args });
        this.thrum.send({ chi: "tendril-reach", sid, callId, name: tendril.name, args: argsOf(tendril.args) });
      }
      sigil.messages.push({
        role: "assistant",
        content: null,
        tool_calls: calls,
      } as any);
      return;
    }

    this.wilt(sid, "stop", inputTokens, outputTokens);
  }

  private sigilFor(sid: string): Sigil {
    const existing = this.sigils.get(sid);
    if (existing) return existing;
    if (this.sigils.size >= OpenAINurse.MAX_SIGILS) {
      const oldest = this.sigils.keys().next().value as string | undefined;
      if (oldest !== undefined) this.sigils.delete(oldest);
    }
    const fresh: Sigil = {
      sid,
      model: "",
      messages: [],
      pending: new Map(),
      generation: 0,
      wilted: false,
    };
    this.sigils.set(sid, fresh);
    return fresh;
  }

  private wilt(sid: string, reason: string, inputTokens: number, outputTokens: number): void {
    this.thrum.send({
      chi: "wilt",
      sid,
      reason,
      nectar: { input_tokens: inputTokens, output_tokens: outputTokens, total_tokens: inputTokens + outputTokens },
    });
  }

  cancel(msg: Tone<"cancel">): void {
    const sid = msg.sid ?? "";
    const sigil = this.sigils.get(sid);
    if (!sigil) return;
    sigil.generation++;
    sigil.wilted = true;
  }

  private error(sid: string, code: string, message: string): void {
    this.thrum.send({
      chi: "error",
      sid,
      code,
      subtype: "error",
      message,
    });
  }
}

function argsOf(raw: string): unknown {
  try { return JSON.parse(raw); } catch { return raw; }
}

function petalText(content: Petal[] | undefined): string {
  if (!Array.isArray(content)) return "";
  return content
    .filter((p) => p.kind === PetalKind.text && typeof p.text === "string")
    .map((p) => p.text as string)
    .join("\n");
}
