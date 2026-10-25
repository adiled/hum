// The hive's bees, over the thrum client.
//
// The main bee is the worker that serves OpenAI models. Each caller of
// the HTTP surface prompts as a forager of its own, a requester, entered
// under a label derived from who is asking, so humd hears every caller
// apart. A requester follows each sigil it opened by sid. When humd goes
// away mid-bloom, every open sigil hears an error and its HTTP response
// ends, and a new request waits briefly for humd to return before it is
// turned away. A requester is ready once humd hears it, and waits at
// most the time it is given for that.

import { Bee, type Tone } from "../../../thrum-clients/ts/index.ts";
import pkg from "../package.json";

export const HIVE_NAME = "openai-server";
export const BEE_VERSION = pkg.version;
export const SOURCE = "https://github.com/adiled/hum/tree/main/hives/openai-server";

export type { Tone };
type SidHandler = (msg: Tone) => void;

export class Requester {
  readonly bee: Bee;
  lastUsed = Date.now();
  readonly started: Promise<void>;
  #open = new Map<string, SidHandler>();
  #up = false;
  #waiting: Array<() => void> = [];

  constructor(label: string) {
    this.bee = new Bee({ hive: HIVE_NAME, as: label, hello: { roles: ["forager"], version: BEE_VERSION, source: SOURCE } });
    this.bee.onNestled(() => {
      this.#up = true;
      for (const wake of this.#waiting.splice(0)) wake();
    });
    this.bee.onAstray(() => {
      this.#up = false;
      for (const [sid, fn] of [...this.#open]) fn({ chi: "error", sid, message: "humd went away mid-bloom" });
    });
    this.started = this.bee.start();
    this.started.catch(() => {});
  }

  async ready(ms: number): Promise<void> {
    await this.started;
    if (this.#up) return;
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("humd is away")), ms);
      this.#waiting.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  on(sid: string, fn: SidHandler): void {
    this.#open.set(sid, fn);
    this.bee.onSid(sid, fn);
  }

  off(sid: string): void {
    this.#open.delete(sid);
    this.bee.offSid(sid);
  }

  send(tone: Tone): void {
    this.lastUsed = Date.now();
    this.bee.send(tone);
  }

  busy(): boolean {
    return this.#open.size > 0;
  }

  close(): void {
    void this.bee.close();
  }
}
