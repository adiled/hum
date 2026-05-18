// hum — Vercel AI SDK provider.
//
// Usage:
//   import { createHum } from "@hum/vercel-ai";
//   const hum = createHum({ cwd: process.cwd() });
//   const result = await streamText({ model: hum("claude-sonnet-4-6"), prompt: "..." });

import { HumModel, type HumProviderConfig } from "./provider.ts";

export type { HumProviderConfig } from "./provider.ts";
export { HumModel } from "./provider.ts";

export function createHum(config: HumProviderConfig = {}) {
  const fn = (modelId: string) => new HumModel(modelId, config);
  fn.languageModel = (modelId: string) => new HumModel(modelId, config);
  return fn;
}
