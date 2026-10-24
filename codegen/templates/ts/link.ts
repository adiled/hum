// The client end of `hum bee link`, for a bee written in TS.
//
// A Bee runs `hum bee link` and speaks to it over two pipes, one JSON
// object per line. The link is hum's own client, in Rust: it enters the
// bee, signs the hello, reads humd's verdict, reconnects when humd
// restarts, and checks every tone against the thrum. So this file knows
// no protocol, and a TS bee behaves exactly as a bee in any language.
//
//   const bee = new Bee({ hive: "openai-server", as: "main", hello: { roles: ["worker"], models } });
//   bee.on(Chi.prompt, (tone) => { bee.send({ chi: Chi.chunk, sid: tone.sid, chunkType: "text_delta", delta }) });
//   bee.onLost(() => stopTurns());
//   await bee.start();   // admitted, or Refused, or LinkError
//   await bee.closed();  // how the link ended; Refused or LinkError unless the bee closed it
//
// A tone goes to the handler for its sid if there is one, else to the
// handler for its chi, else to onAny: an asker follows each session it
// opened by sid, a worker takes prompts by chi. Handlers run in the
// order tones arrive. An async handler is not awaited, so a long turn
// never holds up the tone after it (a cancel).
// A tone sent while humd is away is dropped by the link, which says so.

import { spawn, type ChildProcess } from "node:child_process";
import { accessSync, constants as fsConstants } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { createInterface } from "node:readline";
import type { Envelope, HelloBody, ToneView, ToneViews } from "./protocol.ts";

// @constants

export type ClientOwned = (typeof CLIENT_OWNED)[number];

/** The bee's own hello fields. The client owns the rest. */
export type Hello = Omit<HelloBody, ClientOwned>;

/** A change to the hello: a field set to null is dropped. */
export type HelloPatch = { [K in keyof Hello]?: Hello[K] | null };

/** A tone the bee hears: its chi's view, and whatever else it carries. */
export type Incoming<C extends keyof ToneViews = keyof ToneViews> = ToneView<C> & Record<string, unknown>;

/** A tone the bee sends. The client gives it a rid when it has none. */
export type Outgoing = {
  [C in keyof ToneViews]: { chi: C; rid?: string } & Partial<Omit<Envelope, "chi" | "rid">> & ToneViews[C];
}[keyof ToneViews];

export interface Advisory {
  code: AdvisoryCodeT | (string & {});
  message: string;
  from: "client" | "humd";
  fields?: Record<string, unknown>;
}

export interface BeeOptions {
  hive: string;
  as: string;
  hello?: Hello;
  /** The `hum` to run. Found by humCli() when not given. */
  hum?: string;
}

export class Refused extends Error {
  readonly reason: string;
  constructor(reason: string) {
    super(`humd refused the bee: ${reason}`);
    this.name = "Refused";
    this.reason = reason;
  }
}

export class LinkError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "LinkError";
  }
}

/** Where `hum` is: $HUM_CLI, else `hum` on PATH, else ~/.local/bin/hum. */
export function humCli(): string {
  const explicit = process.env[HUM_CLI_ENV];
  if (explicit) return explicit;
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    if (dir && runnable(join(dir, HUM_CLI_NAME))) return join(dir, HUM_CLI_NAME);
  }
  return join(homedir(), HUM_CLI_DEFAULT);
}

function runnable(path: string): boolean {
  try {
    accessSync(path, fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
}

type Handler<T> = (arg: T) => unknown;

export class Bee {
  readonly hive: string;
  readonly as: string;
  /** The bee's id, once the link has entered it. */
  id = "";

  #opts: BeeOptions;
  #child: ChildProcess | undefined;
  #bySid = new Map<string, Handler<Incoming>>();
  #byChi = new Map<string, Handler<any>>();
  #any: Handler<Incoming> | undefined;
  #nestled: Handler<void>[] = [];
  #lost: Handler<void>[] = [];
  #advisory: Handler<Advisory>[] = [];
  #starting: { resolve: () => void; reject: (e: Error) => void } | undefined;
  #done: Promise<void>;
  #finish: (e?: Error) => void = () => {};
  #closing = false;
  #refusal: string | undefined;
  #failure: Error | undefined;

  constructor(opts: BeeOptions) {
    this.hive = opts.hive;
    this.as = opts.as;
    this.#opts = opts;
    this.#done = new Promise<void>((resolve, reject) => {
      this.#finish = (e) => (e ? reject(e) : resolve());
    });
    this.#done.catch(() => {});
  }

  /** Handle every tone of one chi; a later handler replaces an earlier. */
  on<C extends keyof ToneViews>(chi: C, fn: Handler<Incoming<C>>): this {
    this.#byChi.set(chi, fn);
    return this;
  }

  /** Handle every tone of one session, before any chi handler. */
  onSid(sid: string, fn: Handler<Incoming>): this {
    this.#bySid.set(sid, fn);
    return this;
  }

  /** Stop following a session. */
  offSid(sid: string): this {
    this.#bySid.delete(sid);
    return this;
  }

  /** Handle every tone no sid or chi handler takes. */
  onAny(fn: Handler<Incoming>): this {
    this.#any = fn;
    return this;
  }

  /** humd admitted the bee: at start, and again after every reconnect. */
  onNestled(fn: Handler<void>): this {
    this.#nestled.push(fn);
    return this;
  }

  /** humd went away. The link is dialing again; stop what humd was waiting on. */
  onLost(fn: Handler<void>): this {
    this.#lost.push(fn);
    return this;
  }

  /** Something the bee should fix. The link has already logged it. */
  onAdvisory(fn: Handler<Advisory>): this {
    this.#advisory.push(fn);
    return this;
  }

  /** Run the link; resolves once humd has admitted the bee. */
  start(): Promise<void> {
    if (this.#child) return Promise.reject(new LinkError("this bee has already started"));
    const hum = this.#opts.hum ?? humCli();
    const child = spawn(hum, ["bee", "link", "--hive", this.hive, "--as", this.as], {
      stdio: ["pipe", "pipe", "inherit"],
    });
    this.#child = child;
    const started = new Promise<void>((resolve, reject) => {
      this.#starting = { resolve, reject };
    });
    child.on("error", (e) => this.#end(new LinkError(`could not run ${hum}: ${e.message}`)));
    child.stdin?.on("error", () => {});
    if (child.stdout) {
      createInterface({ input: child.stdout, crlfDelay: Infinity }).on("line", (line) => this.#receive(line));
    }
    child.on("close", (code, signal) => this.#exited(code, signal));
    this.#write({ hello: this.#opts.hello ?? {} });
    return started;
  }

  send(tone: Outgoing): void {
    this.#write(tone);
  }

  hello(patch: HelloPatch): void {
    this.#write({ hello: patch });
  }

  /** Close the link: what was sent goes out, then the link exits. */
  close(): Promise<void> {
    this.#closing = true;
    if (this.#child) this.#child.stdin?.end();
    else this.#finish();
    return this.#done;
  }

  /** Settles when the link has exited: resolves after close(), rejects otherwise. */
  closed(): Promise<void> {
    return this.#done;
  }

  #write(msg: unknown): void {
    const stdin = this.#child?.stdin;
    if (!stdin || stdin.destroyed || !stdin.writable) return;
    stdin.write(JSON.stringify(msg) + "\n");
  }

  #receive(line: string): void {
    let msg: any;
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (typeof msg?.chi === "string") {
      const sid = typeof msg.sid === "string" ? msg.sid : undefined;
      const fn = (sid !== undefined ? this.#bySid.get(sid) : undefined) ?? this.#byChi.get(msg.chi) ?? this.#any;
      if (fn) call(fn, msg);
      return;
    }
    switch (msg?.link) {
      case "ready":
        if (msg.linkVersion !== LINK_VERSION) {
          this.#failure = new LinkError(
            `hum speaks link version ${msg.linkVersion} and this client ${LINK_VERSION}; rebuild the hive against the installed hum`,
          );
          this.#child?.kill("SIGKILL");
          return;
        }
        this.id = String(msg.bee ?? "");
        return;
      case "nestled":
        this.#starting?.resolve();
        this.#starting = undefined;
        for (const fn of this.#nestled) call(fn, undefined);
        return;
      case "lost":
        for (const fn of this.#lost) call(fn, undefined);
        return;
      case "advisory": {
        const { link: _, ...advisory } = msg;
        for (const fn of this.#advisory) call(fn, advisory as Advisory);
        return;
      }
      case "refused":
        this.#refusal = String(msg.error ?? "refused");
        return;
    }
  }

  #exited(code: number | null, signal: NodeJS.Signals | null): void {
    if (code === LINK_EXIT.refused) return this.#end(new Refused(this.#refusal ?? "refused"));
    if (this.#failure) return this.#end(this.#failure);
    if (code === LINK_EXIT.closed && this.#closing) return this.#end();
    const how = signal ? `was killed by ${signal}` : `exited ${code}`;
    this.#end(new LinkError(`the link ${how}${code === LINK_EXIT.usage ? ", a usage error" : ""}`));
  }

  #end(err?: Error): void {
    if (this.#starting) {
      this.#starting.reject(err ?? new LinkError("the link closed before humd admitted the bee"));
      this.#starting = undefined;
    }
    this.#finish(err);
  }
}

function call<T>(fn: Handler<T>, arg: T): void {
  try {
    const r = fn(arg) as { then?: unknown } | undefined;
    if (r && typeof r.then === "function") {
      (r as Promise<unknown>).then(undefined, (e: unknown) => console.error("thrum: a handler failed:", e));
    }
  } catch (e) {
    console.error("thrum: a handler failed:", e);
  }
}
