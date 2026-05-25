// hum as a Vercel AI SDK v3 provider.
//
// The model talks to a humd daemon over thrum (NDJSON unix socket) and
// translates daemon tones into a `LanguageModelV3` stream.  Configuration
// is intentionally tiny — anything beyond `cwd` is left to the consumer
// or to humd's own defaults.

import { randomUUID } from "node:crypto";
import type {
  LanguageModelV3,
  LanguageModelV3CallOptions,
  LanguageModelV3StreamPart,
  LanguageModelV3StreamResult,
  LanguageModelV3GenerateResult,
  LanguageModelV3FinishReason,
  LanguageModelV3Usage,
  LanguageModelV3Prompt,
  LanguageModelV3FunctionTool,
} from "@ai-sdk/provider";

import { getThrum, type Tone } from "./thrum.ts";
import { HumTranslator } from "./transform.ts";

export interface HumProviderConfig {
  /** Working directory humd uses for fs-backed tools (JSONL transcripts,
   *  hum fs MCP). Omit for pure inference. */
  cwd?: string;
  /** Override the thrum socket path. Defaults to $XDG_RUNTIME_DIR/hum/hum.sock.thrum. */
  thrumPath?: string;
}

function emptyUsage(): LanguageModelV3Usage {
  return {
    inputTokens: { total: 0, noCache: 0, cacheRead: 0, cacheWrite: 0 },
    outputTokens: { total: 0, text: 0, reasoning: 0 },
  };
}

function lastUserText(prompt: LanguageModelV3Prompt): string {
  for (let i = prompt.length - 1; i >= 0; i--) {
    const m = prompt[i];
    if (m.role !== "user") continue;
    if (typeof m.content === "string") return m.content;
    if (Array.isArray(m.content)) {
      return m.content.filter((p: any) => p.type === "text").map((p: any) => p.text ?? "").join("\n");
    }
  }
  return "";
}

function systemFrom(prompt: LanguageModelV3Prompt): string {
  const out: string[] = [];
  for (const m of prompt) {
    if (m.role === "system" && typeof m.content === "string") out.push(m.content);
  }
  return out.join("\n\n");
}

function toolsFor(opts: LanguageModelV3CallOptions): Array<{ name: string; description?: string; parameters?: unknown }> | undefined {
  const tools = opts.tools as LanguageModelV3FunctionTool[] | undefined;
  if (!Array.isArray(tools) || tools.length === 0) return undefined;
  const out: Array<{ name: string; description?: string; parameters?: unknown }> = [];
  for (const t of tools) {
    if (!t || t.type !== "function" || !t.name) continue;
    out.push({
      name: t.name,
      ...(t.description ? { description: t.description } : {}),
      ...(t.inputSchema ? { parameters: t.inputSchema } : {}),
    });
  }
  return out.length > 0 ? out : undefined;
}

export class HumModel implements LanguageModelV3 {
  readonly specificationVersion = "v3" as const;
  readonly modelId: string;
  readonly provider = "hum";
  readonly supportedUrls: Record<string, RegExp[]> = {};

  constructor(modelId: string, private config: HumProviderConfig = {}) {
    this.modelId = modelId;
  }

  async doStream(opts: LanguageModelV3CallOptions): Promise<LanguageModelV3StreamResult> {
    const thrum = getThrum();
    await thrum.connect();

    const sid = `vai-${randomUUID()}`;
    const content = lastUserText(opts.prompt);
    const systemPrompt = systemFrom(opts.prompt);
    const tools = toolsFor(opts);

    const translator = new HumTranslator();

    const stream = new ReadableStream<LanguageModelV3StreamPart>({
      start: (controller) => {
        controller.enqueue({ type: "stream-start", warnings: [] });

        let closed = false;
        const close = () => {
          if (closed) return;
          closed = true;
          thrum.off(sid);
          for (const part of translator.flush()) {
            try { controller.enqueue(part); } catch { /* already closed */ }
          }
          try { controller.close(); } catch { /* ignore */ }
        };

        thrum.on(sid, (msg: Tone) => {
          for (const part of translator.ingest(msg)) {
            try { controller.enqueue(part); } catch { closed = true; }
            if (part.type === "finish" || part.type === "error") {
              closed = true;
              thrum.off(sid);
              try { controller.close(); } catch { /* ignore */ }
              return;
            }
          }
        });

        if (opts.abortSignal) {
          if (opts.abortSignal.aborted) {
            thrum.send({ chi: "cancel", sid, reason: "client-abort" });
            close();
            return;
          }
          opts.abortSignal.addEventListener("abort", () => {
            thrum.send({ chi: "cancel", sid, reason: "client-abort" });
            close();
          });
        }

        thrum.send({
          chi: "prompt",
          sid,
          modelId: this.modelId,
          content,
          ...(systemPrompt ? { systemPrompt } : {}),
          ...(this.config.cwd ? { cwd: this.config.cwd } : {}),
          ...(tools ? { tools } : {}),
        });
      },

      cancel: () => {
        thrum.off(sid);
        thrum.send({ chi: "cancel", sid, reason: "client-cancel" });
      },
    });

    return { stream };
  }

  async doGenerate(opts: LanguageModelV3CallOptions): Promise<LanguageModelV3GenerateResult> {
    const { stream } = await this.doStream(opts);
    const reader = stream.getReader();
    const text: string[] = [];
    let finishReason: LanguageModelV3FinishReason = { unified: "stop", raw: "stop" };
    let usage: LanguageModelV3Usage = emptyUsage();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value.type === "text-delta") text.push(value.delta);
      if (value.type === "finish") { finishReason = value.finishReason; usage = value.usage; }
    }
    return {
      content: text.length > 0 ? [{ type: "text", text: text.join("") }] : [],
      usage,
      finishReason,
      warnings: [],
    };
  }
}
