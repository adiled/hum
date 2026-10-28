// A caller, as a forager bee.
//
// Each key is one requester, a forager bee of this hive entered under the
// key's label (keys.ts). A requester is ready once humd admits it and its
// breath says what it can reach, the `models`, `scents` and `tools` it
// keeps as humd breathes again. It follows each sigil it is growing a bloom
// in, one follower a sid, and a sid it follows hears an error if humd goes
// away mid-bloom.
//
// A reach for one of its caller's tools is held by callId until the caller
// answers it, for ten minutes at most: the nest gives up on a reach a
// little after the longest `bash` may run, ten minutes, and an answer held
// past that would reach a call no one waits on any more. An answer that
// comes once its hold is gone grows the bloom on from its graft instead. A
// hold let go unanswered, as it lapses or as the link closes, cancels its
// bloom, unless a prompt has since opened another on its sid; the cancel's
// wilt is never taken for a later bloom's. A link that ends on its own
// leaves its cancels to the key's next link. A reach for a sid whose bloom
// has already gone back to the caller cannot be put to the caller any
// more, so it is answered at once with an error asking the cell to call
// again. A reach the requester sends itself, a scent, waits for its result
// by callId. A requester's link closes once it has been idle ten minutes,
// following no bloom and holding no call, and opens again on its next
// request.

import { randomUUID } from "node:crypto";
import { Bee, Chi, Role, type Hello, type Model, type Tone } from "../../../thrum-clients/ts/index.ts";
import pkg from "../package.json";
import { HIVE } from "./config.ts";
import { HttpError } from "./http.ts";

export const SOURCE = "https://github.com/adiled/hum/tree/main/hives/openai-server";
export const VERSION = pkg.version;

export const HELLO: Hello = {
  version: VERSION,
  propensity: { statefulness: "stateful", richness: "rich", wire: "openai/api" },
  chis: ["hello", "prompt", "cancel", "cleanup", "tendril-reach", "tendril-result", "unfurl", "wilt", "error", "tendril-info"],
  source: SOURCE,
};

const IDLE_MS = 10 * 60_000;
const READY_MS = 10_000;
const HELD_MS = 10 * 60_000;

export interface Reach {
  models: Model[];
  scents: Model[];
  tools: unknown[];
}

type Follower = (tone: Tone) => void;

export class Requester {
  readonly label: string;
  readonly bee: Bee;
  lastUsed = Date.now();
  reach: Reach = { models: [], scents: [], tools: [] };
  readonly #started: Promise<void>;
  #up = false;
  #breathed = false;
  #waiting: Array<() => void> = [];
  #following = new Map<string, Follower>();
  #asked = new Map<string, (tone: Tone<"tendril-result">) => void>();
  #held = new Map<string, { sid: string; at: number }>();
  #rested = new Set<string>();
  #stilled = new Set<string>();

  constructor(label: string, hum?: string) {
    this.label = label;
    this.bee = new Bee({ hive: HIVE, as: label, role: Role.forager, hello: HELLO, hum });
    this.bee.on(Chi.breath, (b) => {
      this.reach = { models: b.models ?? [], scents: b.scents ?? [], tools: b.tools ?? [] };
      this.#breathed = true;
      this.#wake();
    });
    this.bee.on(Chi.tendrilResult, (t) => {
      const asked = this.#asked.get(t.callId);
      if (asked) asked(t);
    });
    this.bee.onAny((t) => this.#stray(t));
    this.bee.onNestled(() => {
      this.#up = true;
      this.#wake();
    });
    this.bee.onAstray(() => {
      this.#up = false;
      this.#breathed = false;
      for (const [sid, fn] of [...this.#following]) fn({ chi: "error", sid, code: "humd-away", message: "humd went away mid-bloom" });
    });
    this.#started = this.bee.start();
    this.#started.catch(() => {});
  }

  #wake(): void {
    if (!this.#up || !this.#breathed) return;
    for (const wake of this.#waiting.splice(0)) wake();
  }

  #stray(t: Tone): void {
    if (t.sid && (t.chi === "wilt" || t.chi === "error")) this.#stilled.delete(t.sid);
    if (t.chi !== "tendril-reach" || !t.sid || !this.#rested.has(t.sid)) return;
    this.bee.send({
      chi: "tendril-result",
      sid: t.sid,
      callId: t.callId,
      isError: true,
      output: "this call came after the turn had gone back to the caller, so no one will answer it; call it again",
    });
  }

  async ready(ms = READY_MS): Promise<void> {
    if (this.#up && this.#breathed) return;
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new HttpError(503, "humd did not admit this caller's bee in time; is humd running?", "humd_away")), ms);
      this.#started.catch((e) => {
        clearTimeout(timer);
        reject(new HttpError(503, `this caller's bee could not join humd: ${e instanceof Error ? e.message : e}`, "humd_refused"));
      });
      this.#waiting.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  follow(sid: string, fn: Follower): void {
    this.lastUsed = Date.now();
    this.#rested.delete(sid);
    this.#following.set(sid, fn);
    this.bee.onSid(sid, (t) => {
      if (t.chi === "wilt" && t.reason === "cancel" && this.#stilled.delete(sid)) return;
      fn(t);
    });
  }

  unfollow(sid: string, rest = false): void {
    this.#following.delete(sid);
    this.bee.offSid(sid);
    if (rest) this.#rested.add(sid);
  }

  following(sid: string): boolean {
    return this.#following.has(sid);
  }

  send(tone: Tone): void {
    this.lastUsed = Date.now();
    if (tone.chi === "prompt" && tone.sid) {
      for (const [id, h] of this.#held) if (h.sid === tone.sid) this.#held.delete(id);
    }
    this.bee.send(tone);
  }

  cancel(sid: string): void {
    this.#stilled.add(sid);
    this.bee.send({ chi: "cancel", sid });
  }

  hold(sid: string, callId: string): void {
    this.#held.set(callId, { sid, at: Date.now() });
  }

  #live(now: number): Array<{ sid: string; at: number }> {
    return [...this.#held.values()].filter((h) => now - h.at <= HELD_MS);
  }

  lapse(now = Date.now()): void {
    const lapsed = new Set<string>();
    for (const [id, h] of this.#held) {
      if (now - h.at <= HELD_MS) continue;
      this.#held.delete(id);
      lapsed.add(h.sid);
    }
    const live = new Set(this.#live(now).map((h) => h.sid));
    for (const sid of lapsed) if (!live.has(sid) && !this.#following.has(sid)) this.cancel(sid);
  }

  letGo(): string[] {
    const sids = [...new Set([...this.#held.values()].map((h) => h.sid))];
    this.#held.clear();
    return sids;
  }

  heldSid(callIds: string[]): string | undefined {
    const now = Date.now();
    const sids = new Set(callIds.map((id) => {
      const h = this.#held.get(id);
      return h && now - h.at <= HELD_MS ? h.sid : undefined;
    }));
    if (sids.size !== 1) return undefined;
    const [sid] = sids;
    return sid;
  }

  release(callIds: string[]): void {
    for (const id of callIds) this.#held.delete(id);
  }

  scent(args: { model: string; input: string[]; dimensions?: number }, ms: number): Promise<Tone<"tendril-result">> {
    const callId = `scent-${randomUUID()}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#asked.delete(callId);
        reject(new HttpError(504, `no scent came back within ${ms} ms`));
      }, ms);
      this.#asked.set(callId, (t) => {
        clearTimeout(timer);
        this.#asked.delete(callId);
        resolve(t);
      });
      this.send({ chi: "tendril-reach", callId, name: "scent", args });
    });
  }

  idle(now = Date.now()): boolean {
    return this.#following.size === 0 && this.#asked.size === 0 && this.#live(now).length === 0 && now - this.lastUsed > IDLE_MS;
  }

  close(): void {
    for (const sid of this.letGo()) this.cancel(sid);
    void this.bee.close();
  }
}

export class Requesters {
  readonly #all = new Map<string, Requester>();
  readonly #owed = new Map<string, string[]>();
  readonly #hum: string | undefined;

  constructor(hum?: string) {
    this.#hum = hum;
    setInterval(() => {
      const now = Date.now();
      for (const [label, rq] of this.#all) {
        rq.lapse(now);
        if (rq.idle(now)) {
          rq.close();
          this.#all.delete(label);
        }
      }
    }, 60_000).unref();
  }

  peek(label: string): Requester | undefined {
    return this.#all.get(label);
  }

  async get(label: string): Promise<Requester> {
    let rq = this.#all.get(label);
    if (!rq) {
      const born = new Requester(label, this.#hum);
      this.#all.set(label, born);
      born.bee.closed().catch((e) => {
        console.error(`[openai-server] ${label}: ${e}`);
        if (this.#all.get(label) !== born) return;
        this.#all.delete(label);
        const owed = born.letGo();
        if (owed.length > 0) this.#owed.set(label, [...(this.#owed.get(label) ?? []), ...owed]);
      });
      rq = born;
    }
    rq.lastUsed = Date.now();
    await rq.ready();
    const owed = this.#owed.get(label);
    this.#owed.delete(label);
    for (const sid of owed ?? []) rq.cancel(sid);
    return rq;
  }
}
