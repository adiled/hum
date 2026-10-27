// The provider's bees, over the thrum client.
//
// One forager bee for each label, entered under this hive as that label
// when a call first needs it. A call prompts on its conversation's sigil
// (conversation.ts) and follows it by sid. When humd goes away mid-bloom,
// every open sigil hears an error and its stream ends; a call waits up to
// READY_MS for humd to admit the bee, then fails.
//
// A reach for one of the call's tools is held by callId until a later call
// answers it, for up to HELD_MS. A reach on a sigil whose turn has already
// gone back to the SDK cannot be put to it any more, so it is answered at
// once with an error asking the cell to call again; a held bloom that
// closes on its own is held no more. A held sigil whose reach was heard on
// a link that has since closed is attached again before it is answered.
//
// The link closes once nothing has been open for IDLE_MS, so an app that is
// done can exit, and opens again on the next call. `close()` cancels every
// bloom still open or held, and closes the link at once.

import { Bee, Role, type Hello, type Tone } from "../../../thrum-clients/ts/index.ts";
import pkg from "../package.json";
import { Sigils, type Answer } from "./conversation.ts";

export const HIVE_NAME = "vercel-ai";
const READY_MS = 10_000;
const IDLE_MS = 1_000;
const HELD_MS = 10 * 60_000;

const HELLO: Hello = {
  version: pkg.version,
  propensity: { statefulness: "convention-stateful", richness: "rich", wire: "ai-sdk/language-model-v3" },
  chis: ["hello", "prompt", "cancel", "cleanup", "attach", "tendril-result", "unfurl", "wilt", "error", "tendril-reach", "tendril-info"],
  source: "https://github.com/adiled/hum/tree/main/hives/vercel-ai",
};

export type { Tone };
type Follower = (tone: Tone) => void;

export class Forager {
  readonly label: string;
  readonly sigils = new Sigils((sid) => this.send({ chi: "cleanup", sid }));
  #bee: Bee | undefined;
  #started: Promise<void> = Promise.resolve();
  #up = false;
  #waiting: Array<() => void> = [];
  #following = new Map<string, Follower>();
  #held = new Map<string, { sid: string; at: number; bee: Bee | undefined }>();
  #idle: NodeJS.Timeout | undefined;

  constructor(label: string) {
    this.label = label;
  }

  #open(): void {
    const bee = new Bee({ hive: HIVE_NAME, as: this.label, role: Role.forager, hello: HELLO });
    this.#bee = bee;
    this.#up = false;
    bee.onNestled(() => {
      if (this.#bee !== bee) return;
      this.#up = true;
      for (const wake of this.#waiting.splice(0)) wake();
    });
    bee.onAstray(() => {
      if (this.#bee !== bee) return;
      this.#up = false;
      this.#lose("humd went away mid-bloom");
    });
    bee.onAny((t) => this.#stray(t));
    this.#started = bee.start();
    this.#started.catch(() => {});
    bee.closed().then(
      () => this.#gone(bee, "the link closed mid-bloom"),
      (e) => this.#gone(bee, e instanceof Error ? e.message : String(e)),
    );
  }

  #gone(bee: Bee, why: string): void {
    if (this.#bee !== bee) return;
    this.#bee = undefined;
    this.#up = false;
    this.#lose(why);
  }

  #lose(why: string): void {
    for (const [sid, fn] of [...this.#following]) fn({ chi: "error", sid, message: why });
  }

  #stray(t: Tone): void {
    const sid = t.sid;
    if (!sid || ![...this.#held.values()].some((h) => h.sid === sid)) return;
    if (t.chi === "tendril-reach") {
      this.send({
        chi: "tendril-result",
        sid,
        callId: t.callId,
        isError: true,
        output: "this call came after the turn had gone back to the caller, so no one will answer it; call it again",
      });
    }
    if (t.chi === "wilt" || t.chi === "error") this.release(sid);
  }

  #rest(): void {
    clearTimeout(this.#idle);
    if (this.#following.size > 0 || !this.#bee) return;
    this.#idle = setTimeout(() => void this.#shut(), IDLE_MS);
    this.#idle.unref();
  }

  async #shut(): Promise<void> {
    clearTimeout(this.#idle);
    const bee = this.#bee;
    this.#bee = undefined;
    this.#up = false;
    await bee?.close().catch(() => {});
  }

  async ready(): Promise<void> {
    clearTimeout(this.#idle);
    if (!this.#bee) this.#open();
    if (this.#up) return;
    const started = this.#started;
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("humd is away")), READY_MS);
      started.catch((e) => {
        clearTimeout(timer);
        reject(e);
      });
      this.#waiting.push(() => {
        clearTimeout(timer);
        resolve();
      });
    }).catch((e) => {
      this.#rest();
      throw e;
    });
  }

  follow(sid: string, fn: Follower): void {
    clearTimeout(this.#idle);
    this.#following.set(sid, fn);
    this.#bee?.onSid(sid, fn);
  }

  unfollow(sid: string): void {
    this.#following.delete(sid);
    this.#bee?.offSid(sid);
    this.#rest();
  }

  send(tone: Tone): void {
    this.#bee?.send(tone);
  }

  hold(sid: string, callId: string): void {
    this.#held.set(callId, { sid, at: Date.now(), bee: this.#bee });
  }

  heldSid(callIds: string[]): string | undefined {
    const now = Date.now();
    for (const [id, h] of this.#held) if (now - h.at > HELD_MS) this.#held.delete(id);
    const sids = new Set(callIds.map((id) => this.#held.get(id)?.sid));
    if (sids.size !== 1) return undefined;
    const [sid] = sids;
    return sid;
  }

  release(sid: string): void {
    for (const [id, h] of this.#held) if (h.sid === sid) this.#held.delete(id);
  }

  answer(sid: string, given: Answer[]): void {
    const heard = [...this.#held.values()].some((h) => h.sid === sid && h.bee === this.#bee);
    this.release(sid);
    if (!heard) this.send({ chi: "attach", sid });
    for (const a of given) {
      this.send({ chi: "tendril-result", sid, callId: a.callId, output: a.result, ...(a.isError ? { isError: true } : {}) });
    }
  }

  async close(): Promise<void> {
    const open = new Set([...this.#following.keys(), ...[...this.#held.values()].map((h) => h.sid)]);
    this.#held.clear();
    if (open.size > 0) {
      await this.ready().catch(() => {});
      for (const sid of open) this.send({ chi: "cancel", sid });
    }
    this.#lose("the provider closed");
    await this.#shut();
  }
}

const foragers = new Map<string, Forager>();

export function foragerOf(label = "main"): Forager {
  let f = foragers.get(label);
  if (!f) foragers.set(label, (f = new Forager(label)));
  return f;
}
