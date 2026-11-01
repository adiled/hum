// The hive's one bee, a forager entered as `main`, that every caller
// prompts through.
//
// It is ready once humd admits it. It follows each sigil it is growing a
// bloom in, one follower a sid, and a sid it follows hears an error if
// humd goes away mid-bloom. A reach for one of a caller's tools is held by
// callId until the caller's tool_result comes, for thirty minutes at most.
// A reach for a sid whose bloom has already gone back to the caller cannot
// be put to the caller any more, so it is answered at once with an error
// asking the cell to call again.

import { Bee, Role, type Hello, type Tone } from "../../../thrum-clients/ts/index.ts";
import pkg from "../package.json";
import { HttpError } from "./http.ts";

export const HIVE = "anthropic-server";

const HELD_MS = 30 * 60_000;

type Follower = (tone: Tone) => void;

export class Forager {
  readonly hello: Hello = {
    version: pkg.version,
    propensity: { statefulness: "convention-stateful", richness: "medium", wire: "anthropic/messages" },
    chis: ["hello", "prompt", "cancel", "cleanup", "unfurl", "wilt", "error", "tendril-reach", "tendril-result"],
    source: "https://github.com/adiled/hum/tree/main/hives/anthropic-server",
  };
  readonly bee = new Bee({ hive: HIVE, as: "main", role: Role.forager, hello: this.hello });
  #up = false;
  #following = new Map<string, Follower>();
  #held = new Map<string, { sid: string; at: number }>();
  #rested = new Set<string>();

  constructor() {
    this.bee.onNestled(() => {
      this.#up = true;
    });
    this.bee.onAstray(() => {
      this.#up = false;
      for (const [sid, fn] of [...this.#following]) fn({ chi: "error", sid, code: "humd-away", message: "humd went away mid-bloom" });
    });
    this.bee.onAny((t) => this.#stray(t));
  }

  #stray(t: Tone): void {
    if (t.chi !== "tendril-reach" || !t.sid || !this.#rested.has(t.sid)) return;
    this.bee.send({
      chi: "tendril-result",
      sid: t.sid,
      callId: t.callId,
      isError: true,
      output: "this call came after the turn had gone back to the caller, so no one will answer it; call it again",
    });
  }

  ready(): void {
    if (!this.#up) throw new HttpError(503, "humd is away, or has not admitted this server's bee yet; try again shortly");
  }

  follow(sid: string, fn: Follower): void {
    this.#rested.delete(sid);
    this.#following.set(sid, fn);
    this.bee.onSid(sid, fn);
  }

  unfollow(sid: string, rest = false): void {
    this.#following.delete(sid);
    this.bee.offSid(sid);
    if (rest) this.#rested.add(sid);
  }

  forget(sid: string): void {
    this.#rested.delete(sid);
    this.bee.send({ chi: "cleanup", sid });
  }

  send(tone: Tone): void {
    this.bee.send(tone);
  }

  hold(sid: string, callId: string): void {
    this.#held.set(callId, { sid, at: Date.now() });
  }

  heldSid(callIds: string[]): string | undefined {
    const now = Date.now();
    for (const [id, h] of this.#held) if (now - h.at > HELD_MS) this.#held.delete(id);
    const sids = new Set(callIds.map((id) => this.#held.get(id)?.sid));
    if (sids.size !== 1) return undefined;
    const [sid] = sids;
    return sid;
  }

  release(callIds: string[]): void {
    for (const id of callIds) this.#held.delete(id);
  }
}
