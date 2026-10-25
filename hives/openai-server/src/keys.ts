// Who may call, and which bee each caller is.
//
// A key is a bearer token with a name. Keys live in secrets/keys.json in
// the hive's state, mode 0600 in a 0700 directory, as the sha256 of each
// token, so the file never holds one; `key add` prints a new token once.
// The config's apiKey, if any, is a key named `default`. The file is read
// again whenever it changes, so a key added or removed counts at once.
//
// Each key is one forager bee of this hive, entered as `key-<name>`, so
// humd hears every caller apart and `hum bee <id> exit` shuts one out.
// With no keys at all, anyone may call, as the one bee `anonymous`.

import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { chmodSync, mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { stateDir } from "./config.ts";

export const ANONYMOUS = "anonymous";

const NAME_RE = /^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$/;

interface Stored {
  sha256: string;
  created: string;
}

export function keysPath(): string {
  return join(stateDir(), "secrets", "keys.json");
}

function sha(token: string): Buffer {
  return createHash("sha256").update(token).digest();
}

function readStored(): Record<string, Stored> {
  try {
    const parsed = JSON.parse(readFileSync(keysPath(), "utf8")) as { keys?: Record<string, Stored> };
    return parsed.keys ?? {};
  } catch {
    return {};
  }
}

function writeStored(keys: Record<string, Stored>): void {
  const dir = join(stateDir(), "secrets");
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  chmodSync(dir, 0o700);
  const tmp = `${keysPath()}.tmp`;
  writeFileSync(tmp, JSON.stringify({ keys }, null, 2) + "\n", { mode: 0o600 });
  renameSync(tmp, keysPath());
}

export function labelOf(name: string): string {
  return name === ANONYMOUS ? ANONYMOUS : `key-${name}`;
}

export class Keys {
  readonly #configured: string;
  #seen = -1;
  #hashes = new Map<string, Buffer>();

  constructor(configured: string) {
    this.#configured = configured;
  }

  #refresh(): void {
    let mtime = 0;
    try {
      mtime = statSync(keysPath()).mtimeMs;
    } catch {}
    if (mtime === this.#seen) return;
    this.#seen = mtime;
    const hashes = new Map<string, Buffer>();
    for (const [name, k] of Object.entries(readStored())) {
      if (typeof k?.sha256 === "string" && k.sha256.length === 64) hashes.set(name, Buffer.from(k.sha256, "hex"));
    }
    if (this.#configured) hashes.set("default", sha(this.#configured));
    this.#hashes = hashes;
  }

  open(): boolean {
    this.#refresh();
    return this.#hashes.size === 0;
  }

  nameOf(bearer: string): string | undefined {
    this.#refresh();
    if (this.#hashes.size === 0) return ANONYMOUS;
    if (!bearer) return undefined;
    const said = sha(bearer);
    for (const [name, hash] of this.#hashes) {
      if (timingSafeEqual(said, hash)) return name;
    }
    return undefined;
  }
}

export function addKey(name: string): string {
  if (!NAME_RE.test(name) || name === ANONYMOUS) throw new Error(`"${name}" is not a key name: letters, digits, - _ and ., not "${ANONYMOUS}"`);
  const keys = readStored();
  if (keys[name]) throw new Error(`a key named ${name} already exists; remove it first`);
  const token = `sk-hum-${randomBytes(32).toString("base64url")}`;
  keys[name] = { sha256: sha(token).toString("hex"), created: new Date().toISOString() };
  writeStored(keys);
  return token;
}

export function listKeys(): Array<{ name: string; created: string }> {
  return Object.entries(readStored()).map(([name, k]) => ({ name, created: k.created }));
}

export function removeKey(name: string): boolean {
  const keys = readStored();
  if (!keys[name]) return false;
  delete keys[name];
  writeStored(keys);
  return true;
}
