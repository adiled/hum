// hum as a Vercel AI SDK v3 provider.
//
// The model prompts a humd daemon over thrum and translates the bloom's
// tones into a `LanguageModelV3` stream. The last user message becomes
// the prompt's petals, its text and its files as pollen, and the call's
// settings say how the bloom grows: temperature, topP and topK prune,
// maxOutputTokens its season, stopSequences its frost, a seed, a JSON
// response format its trellis, and the tool choice how its tendrils
// reach. Configuration is intentionally tiny: `cwd` is the working
// directory humd uses for fs-backed tools, omitted for pure inference,
// and anything beyond it is left to the consumer or to humd's defaults.

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

import { PetalKind, type Petal } from "../../../thrum-clients/ts/index.ts";
import { getThrum, type Tone } from "./thrum.ts";
import { HumTranslator } from "./transform.ts";

export interface HumProviderConfig {
  cwd?: string;
}

function emptyUsage(): LanguageModelV3Usage {
  return {
    inputTokens: { total: 0, noCache: 0, cacheRead: 0, cacheWrite: 0 },
    outputTokens: { total: 0, text: 0, reasoning: 0 },
  };
}

function base64Of(data: Uint8Array): string {
  return Buffer.from(data).toString("base64");
}

function lastUserPetals(prompt: LanguageModelV3Prompt): Petal[] {
  for (let i = prompt.length - 1; i >= 0; i--) {
    const m = prompt[i];
    if (m.role !== "user") continue;
    if (typeof m.content === "string") return [{ kind: PetalKind.text, text: m.content }];
    const petals: Petal[] = [];
    for (const part of m.content as any[]) {
      if (part.type === "text") petals.push({ kind: PetalKind.text, text: part.text ?? "" });
      if (part.type !== "file") continue;
      const mediaType: string = part.mediaType ?? "application/octet-stream";
      const kind = mediaType.startsWith("image/") ? "image" : mediaType.startsWith("audio/") ? "audio" : "file";
      const data = part.data;
      const grain = data instanceof URL
        ? { kind, mediaType, url: data.toString() }
        : { kind, mediaType, data: typeof data === "string" ? data : base64Of(data) };
      petals.push({ kind: PetalKind.pollen, grain });
    }
    return petals;
  }
  return [{ kind: PetalKind.text, text: "" }];
}

function bloomSettings(opts: LanguageModelV3CallOptions): Partial<Tone<"prompt">> {
  const out: Partial<Tone<"prompt">> = {};
  if (typeof opts.temperature === "number") out.temperature = opts.temperature;
  if (typeof opts.topP === "number" || typeof opts.topK === "number") {
    out.prune = {
      ...(typeof opts.topP === "number" ? { p: opts.topP } : {}),
      ...(typeof opts.topK === "number" ? { k: opts.topK } : {}),
    };
  }
  if (typeof opts.maxOutputTokens === "number") out.season = opts.maxOutputTokens;
  if (Array.isArray(opts.stopSequences) && opts.stopSequences.length > 0) out.frost = opts.stopSequences;
  if (typeof opts.seed === "number") out.seed = opts.seed;
  if (opts.responseFormat?.type === "json") out.trellis = opts.responseFormat.schema ?? "json";
  const choice = opts.toolChoice;
  if (choice) out.tendrils = { reach: choice.type === "tool" ? choice.toolName : choice.type };
  return out;
}

function systemFrom(prompt: LanguageModelV3Prompt): string {
  const out: string[] = [];
  for (const m of prompt) {
    if (m.role === "system" && typeof m.content === "string") out.push(m.content);
  }
  return out.join("\n\n");
}

function toolsFor(opts: LanguageModelV3CallOptions): Array<{ name: string; description?: string; inputSchema?: unknown }> | undefined {
  const tools = opts.tools as LanguageModelV3FunctionTool[] | undefined;
  if (!Array.isArray(tools) || tools.length === 0) return undefined;
  const out: Array<{ name: string; description?: string; inputSchema?: unknown }> = [];
  for (const t of tools) {
    if (!t || t.type !== "function" || !t.name) continue;
    out.push({
      name: t.name,
      ...(t.description ? { description: t.description } : {}),
      ...(t.inputSchema ? { inputSchema: t.inputSchema } : {}),
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
    const content = lastUserPetals(opts.prompt);
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
            try { controller.enqueue(part); } catch {}
          }
          try { controller.close(); } catch {}
        };

        thrum.on(sid, (msg: Tone) => {
          for (const part of translator.ingest(msg)) {
            try { controller.enqueue(part); } catch { closed = true; }
            if (part.type === "finish" || part.type === "error") {
              closed = true;
              thrum.off(sid);
              try { controller.close(); } catch {}
              return;
            }
          }
        });

        if (opts.abortSignal) {
          if (opts.abortSignal.aborted) {
            thrum.send({ chi: "cancel", sid });
            close();
            return;
          }
          opts.abortSignal.addEventListener("abort", () => {
            thrum.send({ chi: "cancel", sid });
            close();
          });
        }

        thrum.send({
          chi: "prompt",
          sid,
          modelId: this.modelId,
          content,
          ...bloomSettings(opts),
          ...(systemPrompt ? { systemPrompt } : {}),
          ...(this.config.cwd ? { cwd: this.config.cwd } : {}),
          ...(tools ? { tools } : {}),
        });
      },

      cancel: () => {
        thrum.off(sid);
        thrum.send({ chi: "cancel", sid });
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
