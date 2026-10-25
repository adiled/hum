// The provider's bee, over the thrum client.
//
// One bee for the process, a forager, started on first use. A generation
// prompts under a sigil of its own and follows it by sid. When humd goes
// away mid-bloom, every open sigil hears an error and its stream ends,
// and a new generation waits briefly for humd to return before it fails.
// The bee connects once humd hears it.

import { Bee, type Tone } from "../../../thrum-clients/ts/index.ts";
import pkg from "../package.json";

export const HIVE_NAME = "vercel-ai";
const READY_MS = 10_000;

export type { Tone };
export type SidHandler = (msg: Tone) => void;

class ProviderBee {
  #bee = new Bee({
    hive: HIVE_NAME,
    as: "main",
    hello: { roles: ["forager"], version: pkg.version },
  });
  #started: Promise<void> | undefined;
  #open = new Map<string, SidHandler>();
  #up = false;
  #waiting: Array<() => void> = [];

  constructor() {
    this.#bee.onNestled(() => {
      this.#up = true;
      for (const wake of this.#waiting.splice(0)) wake();
    });
    this.#bee.onAstray(() => {
      this.#up = false;
      for (const [sid, fn] of [...this.#open]) fn({ chi: "error", sid, message: "humd went away mid-bloom" });
    });
  }

  async connect(): Promise<void> {
    await (this.#started ??= this.#bee.start());
    if (this.#up) return;
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("humd is away")), READY_MS);
      this.#waiting.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  on(sid: string, fn: SidHandler): void {
    this.#open.set(sid, fn);
    this.#bee.onSid(sid, fn);
  }

  off(sid: string): void {
    this.#open.delete(sid);
    this.#bee.offSid(sid);
  }

  send(tone: Tone): void {
    this.#bee.send(tone);
  }
}

let shared: ProviderBee | undefined;

export function getThrum(): ProviderBee {
  return (shared ??= new ProviderBee());
}
