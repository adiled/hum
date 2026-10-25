// What every endpoint shares: the hive's config, its callers' bees, the
// sigils their conversations live in, and what each key has taken.
//
// A model a request names must be one the caller's breath says a nurse
// grows; a request that names none goes to any nurse with room. How a
// request asks its bloom to grow becomes the prompt's: `temperature`,
// `top_p` as `prune`, its token cap as `season`, `stop` as `frost`,
// `seed`, its JSON shape as `trellis`, its tool choice as `tendrils`, its
// reasoning effort as `sepal`. What only an OpenAI model understands,
// penalties and logit bias, rides under this hive's `ext` for its own
// nurse.

import type { PromptBody, Tone } from "../../../thrum-clients/ts/index.ts";
import { HIVE, type Config } from "./config.ts";
import type { Sigils } from "./conversation.ts";
import { HttpError } from "./http.ts";
import type { Requester, Requesters } from "./requester.ts";
import { reachOf } from "./tools.ts";
import type { Usage } from "./usage.ts";

export interface Ctx {
  config: Config;
  requesters: Requesters;
  sigils: Sigils;
  usage: Usage;
}

export interface Caller {
  key: string;
  rq: Requester;
}

export function modelFor(rq: Requester, model: unknown): string | undefined {
  if (model === undefined || model === null || model === "") return undefined;
  if (typeof model !== "string") throw new HttpError(400, "model is a string", null, "model");
  if (rq.reach.models.some((m) => m.id === model)) return model;
  if (rq.reach.scents.some((m) => m.id === model)) {
    throw new HttpError(400, `${model} distils scents, it grows no words; ask /v1/embeddings`, "model_not_supported", "model");
  }
  throw new HttpError(404, `The model '${model}' does not exist, or no nurse here grows it`, "model_not_found", "model");
}

export interface Asked {
  temperature?: number;
  top_p?: number;
  max?: number;
  stop?: string | string[] | null;
  seed?: number;
  format?: { type?: string; schema?: unknown; json_schema?: { schema?: unknown } } | null;
  tool_choice?: unknown;
  parallel_tool_calls?: boolean;
  effort?: string;
  sampling?: Record<string, unknown>;
}

export function growthOf(a: Asked): Partial<PromptBody> & { ext?: Record<string, Record<string, unknown>> } {
  const out: Partial<PromptBody> & { ext?: Record<string, Record<string, unknown>> } = {};
  if (typeof a.temperature === "number") out.temperature = a.temperature;
  if (typeof a.top_p === "number") out.prune = { p: a.top_p };
  if (typeof a.max === "number") out.season = a.max;
  if (typeof a.stop === "string") out.frost = [a.stop];
  else if (Array.isArray(a.stop) && a.stop.length > 0) out.frost = a.stop;
  if (typeof a.seed === "number") out.seed = a.seed;
  if (a.format && a.format.type && a.format.type !== "text") {
    out.trellis = a.format.type === "json_schema" ? (a.format.json_schema?.schema ?? a.format.schema ?? "json") : "json";
  }
  const reach = reachOf(a.tool_choice);
  if (reach !== undefined || typeof a.parallel_tool_calls === "boolean") {
    out.tendrils = {
      ...(reach !== undefined ? { reach } : {}),
      ...(typeof a.parallel_tool_calls === "boolean" ? { parallel: a.parallel_tool_calls } : {}),
    };
  }
  if (typeof a.effort === "string") out.sepal = { effort: a.effort };
  const sampling = Object.fromEntries(Object.entries(a.sampling ?? {}).filter(([, v]) => v !== undefined && v !== null));
  if (Object.keys(sampling).length > 0) out.ext = { [HIVE]: { sampling } };
  return out;
}

export function prompt(sid: string, model: string | undefined, rest: Partial<PromptBody> & { ext?: Record<string, Record<string, unknown>> }): Tone<"prompt"> {
  const tone: Record<string, unknown> = { chi: "prompt", sid, ...(model ? { modelId: model } : {}) };
  for (const [k, v] of Object.entries(rest)) {
    if (v === undefined) continue;
    if (Array.isArray(v) && v.length === 0 && k !== "content") continue;
    tone[k] = v;
  }
  return tone as unknown as Tone<"prompt">;
}
