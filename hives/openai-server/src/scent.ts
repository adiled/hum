// The scent attendant: scents distilled by the API upstream.
//
// Its one bee is entered as an attendant, `scent`, and names in its hello
// the upstream models that distil scents (upstream.ts), so humd brings it
// every `scent` reach for one of them. A reach's texts go to `/embeddings`
// as they are, and the scents come back in their order, with the tokens
// they took as nectar. Any other reach is refused.

import { Bee, Chi, Role, type Hello, type Tone } from "../../../thrum-clients/ts/index.ts";
import { HIVE } from "./config.ts";
import { SOURCE, VERSION } from "./requester.ts";
import { post } from "./upstream.ts";

export const HELLO: Hello = {
  version: VERSION,
  models: [],
  propensity: { statefulness: "stateless", richness: "lean", wire: "openai/embeddings" },
  chis: ["hello", "tendril-reach", "tendril-result"],
  source: SOURCE,
};

export class Scent {
  readonly bee: Bee;
  #models: string[] = [];

  constructor(hum?: string) {
    this.bee = new Bee({ hive: HIVE, as: "scent", role: Role.attendant, hum, hello: HELLO });
    this.bee.on(Chi.tendrilReach, (t) => void this.#reached(t));
  }

  serve(models: string[]): void {
    this.#models = models;
    this.bee.hello({ models });
  }

  #answer(t: Tone<"tendril-reach">, answer: { result?: unknown; output?: string; isError?: boolean }): void {
    this.bee.send({ chi: "tendril-result", callId: t.callId, ...(t.sid ? { sid: t.sid } : {}), ...answer });
  }

  async #reached(t: Tone<"tendril-reach">): Promise<void> {
    const args = (t.args ?? {}) as { model?: unknown; input?: unknown; dimensions?: unknown };
    if (t.name !== "scent" || typeof args.model !== "string" || !Array.isArray(args.input)) {
      return this.#answer(t, { isError: true, output: "this attendant distils scents, and nothing else: name scent, with a model and input" });
    }
    if (!this.#models.includes(args.model)) {
      return this.#answer(t, { isError: true, output: `this attendant does not distil scents by ${args.model}` });
    }
    try {
      const resp = await post("/embeddings", {
        model: args.model,
        input: args.input,
        encoding_format: "float",
        ...(typeof args.dimensions === "number" ? { dimensions: args.dimensions } : {}),
      });
      if (!resp.ok) {
        return this.#answer(t, { isError: true, output: `the API upstream answered ${resp.status}: ${(await resp.text()).slice(0, 500)}` });
      }
      const body = (await resp.json()) as { data?: Array<{ index?: number; embedding?: number[] }>; usage?: { prompt_tokens?: number } };
      const scents = [...(body.data ?? [])].sort((a, b) => (a.index ?? 0) - (b.index ?? 0)).map((d) => d.embedding ?? []);
      this.#answer(t, { result: { scents, nectar: { input_tokens: body.usage?.prompt_tokens ?? 0 } } });
    } catch (e) {
      this.#answer(t, { isError: true, output: `the API upstream did not answer: ${(e as Error).message}` });
    }
  }
}
