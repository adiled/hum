// hum as a Vercel AI SDK v3 provider.
//
// The model prompts a humd daemon over thrum and translates the bloom's
// tones into a `LanguageModelV3` stream. The call's prompt is read as the
// blooms of a conversation (conversation.ts): its last user words are the
// prompt's content, the blooms before them its graft, and the call's
// settings say how the bloom grows: temperature, topP and topK prune,
// maxOutputTokens its season, stopSequences its frost, a seed, a JSON
// response format its trellis, and the tool choice how its tendrils reach.
//
// A reach for one of the call's tools is the SDK's to run. Once reaches
// stop coming the stream finishes with its tool calls, and the bloom waits
// for the next call, whose prompt ends with their results: those are the
// reach's tendril-result, on the bloom's own sigil. A call whose results
// answer no reach still held grows on from a graft of the whole
// conversation instead. A reach for a tool the call did not offer is
// answered at once as unserved.
//
// Configuration is intentionally tiny: `cwd` is where the bloom builds, a
// `hum://<humdid>/<path>` address or a path on this host, which becomes
// `hum:///<path>`; omitted, the bloom builds nowhere and is pure inference.
// `label` names the bee, `main` when omitted. Anything beyond them is left
// to the consumer or to humd.

import { resolve } from "node:path";
import type {
  LanguageModelV3,
  LanguageModelV3CallOptions,
  LanguageModelV3Content,
  LanguageModelV3StreamPart,
  LanguageModelV3StreamResult,
  LanguageModelV3GenerateResult,
  LanguageModelV3FinishReason,
  LanguageModelV3Usage,
} from "@ai-sdk/provider";

import type { Bloom } from "../../../thrum-clients/ts/index.ts";
import { fromPrompt, opening } from "./conversation.ts";
import { foragerOf, type Tone } from "./thrum.ts";
import { reachOf, toolsOf } from "./tools.ts";
import { emptyUsage, HumTranslator } from "./transform.ts";

const QUIET_MS = 150;

export interface HumProviderConfig {
  cwd?: string;
  label?: string;
}

export function buildsAt(cwd: string): string {
  return cwd.startsWith("hum://") ? cwd : `hum://${resolve(cwd)}`;
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
  const reach = reachOf(opts.toolChoice);
  if (reach) out.tendrils = { reach };
  return out;
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
    const forager = foragerOf(this.config.label);
    await forager.ready();
    const said = fromPrompt(opts.prompt);
    const given = said.answers;
    const held = given.length > 0 ? forager.heldSid(given.map((a) => a.callId)) : undefined;
    const continuing = held !== undefined && !forager.sigils.growing(held);
    const { graft, content } = opening(said);
    const sid = continuing ? held : forager.sigils.continuing(said.system, graft);
    const tools = toolsOf(opts.tools);
    const own = new Set(tools?.map((t) => t.name));
    const translator = new HumTranslator(own, new Set(given.map((a) => a.callId)));

    let controller!: ReadableStreamDefaultController<LanguageModelV3StreamPart>;
    let quiet: NodeJS.Timeout | undefined;
    let done = false;
    const after = (): Bloom[] => {
      const blooms = said.blooms.map((b) => ({ content: b.content, grew: [...(b.grew ?? [])] }));
      blooms[blooms.length - 1]?.grew.push(...translator.grew);
      return blooms;
    };
    const end = (parts: Iterable<LanguageModelV3StreamPart>, grown: boolean) => {
      if (done) return;
      done = true;
      clearTimeout(quiet);
      opts.abortSignal?.removeEventListener("abort", cancel);
      forager.unfollow(sid);
      forager.sigils.end(sid);
      if (grown) forager.sigils.remember(said.system, after(), sid);
      for (const part of parts) {
        try { controller.enqueue(part); } catch {}
      }
      try { controller.close(); } catch {}
    };
    const cancel = () => {
      if (done) return;
      forager.release(sid);
      forager.send({ chi: "cancel", sid });
      end(translator.flush(), false);
    };

    const stream = new ReadableStream<LanguageModelV3StreamPart>({
      start: (c) => {
        controller = c;
        controller.enqueue({ type: "stream-start", warnings: [] });
        if (opts.abortSignal?.aborted) return end(translator.flush(), false);
        opts.abortSignal?.addEventListener("abort", cancel);

        forager.sigils.begin(sid);
        forager.follow(sid, (t: Tone) => {
          if (t.chi === "tendril-reach") {
            if (!own.has(t.name)) {
              forager.send({ chi: "tendril-result", sid, callId: t.callId, isError: true, output: `no tool named ${t.name} was offered to this bloom` });
              return;
            }
            forager.hold(sid, t.callId);
            clearTimeout(quiet);
            quiet = setTimeout(() => end(translator.end({ unified: "tool-calls", raw: "tool_use" }), true), QUIET_MS);
          }
          if (t.chi === "wilt" || t.chi === "error") forager.release(sid);
          for (const part of translator.ingest(t)) {
            if (part.type === "finish") return end([part], part.finishReason.unified !== "error");
            try { controller.enqueue(part); } catch {}
          }
        });

        if (continuing) return forager.answer(sid, given);
        forager.send({
          chi: "prompt",
          sid,
          modelId: this.modelId,
          content,
          ...(graft.length > 0 ? { graft } : {}),
          ...bloomSettings(opts),
          ...(said.system ? { systemPrompt: said.system } : {}),
          ...(this.config.cwd ? { cwd: buildsAt(this.config.cwd) } : {}),
          ...(tools ? { tools } : {}),
        });
      },
      cancel,
    });

    return { stream };
  }

  async doGenerate(opts: LanguageModelV3CallOptions): Promise<LanguageModelV3GenerateResult> {
    const { stream } = await this.doStream(opts);
    const reader = stream.getReader();
    const content: LanguageModelV3Content[] = [];
    const open = new Map<string, { text: string }>();
    let finishReason: LanguageModelV3FinishReason = { unified: "other", raw: undefined };
    let usage: LanguageModelV3Usage = emptyUsage();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      switch (value.type) {
        case "text-start":
        case "reasoning-start": {
          const part = { type: value.type === "text-start" ? "text" as const : "reasoning" as const, text: "" };
          open.set(value.id, part);
          content.push(part);
          break;
        }
        case "text-delta":
        case "reasoning-delta": {
          const part = open.get(value.id);
          if (part) part.text += value.delta;
          break;
        }
        case "tool-call":
        case "tool-result":
          content.push(value);
          break;
        case "error":
          throw value.error;
        case "finish":
          finishReason = value.finishReason;
          usage = value.usage;
          break;
      }
    }
    return {
      content: content.filter((c) => !((c.type === "text" || c.type === "reasoning") && c.text === "")),
      usage,
      finishReason,
      warnings: [],
    };
  }
}
