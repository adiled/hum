// POST /v1/embeddings: the scents of texts.
//
// An embedding is a text's scent, which an attendant distils by the model
// it names (WIRE.md, `scent`). The caller's bee reaches for it with no
// bloom between, and the scents come back in the order of the input. A
// model must be one the caller's breath lists among its scents. Scents
// come back as numbers, or as base64 of their float32s when asked;
// token inputs are not distilled here.

import type { IncomingMessage, ServerResponse } from "node:http";
import type { Caller, Ctx } from "./hive.ts";
import { bad, HttpError, readJson, sendJson } from "./http.ts";

interface Body {
  model?: string;
  input?: unknown;
  dimensions?: number;
  encoding_format?: "float" | "base64";
}

function inputsOf(input: unknown): string[] {
  if (typeof input === "string") return [input];
  if (Array.isArray(input) && input.length > 0 && input.every((s) => typeof s === "string")) return input as string[];
  throw bad("input is a text, or a list of texts; token inputs are not distilled here", "input");
}

function base64(scent: number[]): string {
  const buf = Buffer.alloc(scent.length * 4);
  scent.forEach((v, i) => buf.writeFloatLE(v, i * 4));
  return buf.toString("base64");
}

export async function embeddings(ctx: Ctx, who: Caller, req: IncomingMessage, res: ServerResponse): Promise<void> {
  const body = await readJson<Body>(req);
  const { rq } = who;
  const model = body.model;
  if (typeof model !== "string" || model.length === 0) throw bad("model is required", "model");
  if (!rq.reach.scents.some((m) => m.id === model)) {
    if (rq.reach.models.some((m) => m.id === model)) {
      throw new HttpError(400, `${model} grows words; no attendant here distils scents by it`, "model_not_supported", "model");
    }
    throw new HttpError(404, `The model '${model}' does not exist, or no attendant here distils scents by it`, "model_not_found", "model");
  }
  const input = inputsOf(body.input);
  const grasped = await rq.scent({ model, input, ...(typeof body.dimensions === "number" ? { dimensions: body.dimensions } : {}) }, ctx.config.timeoutMs);
  if (grasped.isError) throw new HttpError(502, grasped.output || "the scent could not be distilled", "scent_failed");
  const result = (grasped.result ?? {}) as { scents?: number[][]; nectar?: { input_tokens?: number } };
  const scents = Array.isArray(result.scents) ? result.scents : [];
  if (scents.length !== input.length) throw new HttpError(502, `asked ${input.length} scents and ${scents.length} came back`, "scent_failed");
  const tokens = result.nectar?.input_tokens ?? 0;
  ctx.usage.track(who.key, tokens, 0);
  sendJson(res, 200, {
    object: "list",
    data: scents.map((scent, index) => ({ object: "embedding", index, embedding: body.encoding_format === "base64" ? base64(scent) : scent })),
    model,
    usage: { prompt_tokens: tokens, total_tokens: tokens },
  });
}
