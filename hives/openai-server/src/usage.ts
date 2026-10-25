// What each key has taken, kept in usage.json in the hive's state.
//
// Every request a key makes counts, with the tokens its bloom took in and
// gave out. The count is written whole, through a temporary file, every
// 30 seconds it changed and when the process ends, a stop signal among
// the ways it can.

import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { stateDir } from "./config.ts";

export interface Taken {
  requests: number;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export class Usage {
  readonly #path = join(stateDir(), "usage.json");
  readonly #taken: Record<string, Taken>;
  #dirty = false;

  constructor() {
    let taken: Record<string, Taken> = {};
    try {
      taken = JSON.parse(readFileSync(this.#path, "utf8"));
    } catch {}
    this.#taken = taken && typeof taken === "object" ? taken : {};
    setInterval(() => this.flush(), 30_000).unref();
    process.once("exit", () => this.flush());
    for (const signal of ["SIGTERM", "SIGINT"] as const) {
      process.once(signal, () => {
        this.flush();
        process.exit(0);
      });
    }
  }

  track(key: string, prompt: number, completion: number): void {
    const t = this.#taken[key] ?? { requests: 0, promptTokens: 0, completionTokens: 0, totalTokens: 0 };
    t.requests += 1;
    t.promptTokens += prompt;
    t.completionTokens += completion;
    t.totalTokens += prompt + completion;
    this.#taken[key] = t;
    this.#dirty = true;
  }

  of(key: string): Taken {
    return this.#taken[key] ?? { requests: 0, promptTokens: 0, completionTokens: 0, totalTokens: 0 };
  }

  flush(): void {
    if (!this.#dirty) return;
    try {
      mkdirSync(stateDir(), { recursive: true });
      writeFileSync(`${this.#path}.tmp`, JSON.stringify(this.#taken, null, 2));
      renameSync(`${this.#path}.tmp`, this.#path);
      this.#dirty = false;
    } catch {}
  }
}
