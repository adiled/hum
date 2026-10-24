// Same key file, seed format and signed message as hum-identity, so a bee
// keeps one identity across the languages it is written in. Entering goes
// through the hum CLI, so there is one implementation of it.
import { createHash, createPrivateKey, createPublicKey, sign } from "node:crypto";
import { execFileSync } from "node:child_process";
import { accessSync, constants, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";

const PKCS8_ED25519_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
const SOCKET_DOMAIN = "hum-bee-socket-v1";
const BEE_ID = /^bee_[0-9a-f]{64}$/;

export interface BeeProof {
  hid: string;
  from: string;
  pubkey: string;
  signedAt: number;
  signature: string;
}

function humBin(): string {
  const dirs = (process.env.PATH ?? "").split(delimiter).filter(Boolean);
  for (const dir of [...dirs, join(homedir(), ".local", "bin")]) {
    const candidate = join(dir, "hum");
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {}
  }
  throw new Error("hum is not on PATH or in ~/.local/bin; a bee cannot enter without it");
}

const entered = new Map<string, string>();

export function enter(hive: string, label: string): string {
  const memo = `${hive}\0${label}`;
  const known = entered.get(memo);
  if (known) return known;
  const id = execFileSync(humBin(), ["bee", "enter", "--hive", hive, "--as", label], { encoding: "utf8" }).trim();
  if (!BEE_ID.test(id)) throw new Error(`hum bee enter answered ${JSON.stringify(id)}, not a bee id`);
  entered.set(memo, id);
  return id;
}

export function signedAs(id: string): BeeProof {
  if (!BEE_ID.test(id)) throw new Error(`${JSON.stringify(id)} is not a bee id`);
  const base = process.env.XDG_STATE_HOME
    ? join(process.env.XDG_STATE_HOME, "hum", "bees")
    : join(homedir(), ".local", "state", "hum", "bees");
  const seed = readFileSync(join(base, `${id}.key`));
  if (seed.length !== 32) throw new Error(`bee key for ${id} is ${seed.length} bytes, expected 32`);
  const priv = createPrivateKey({ key: Buffer.concat([PKCS8_ED25519_PREFIX, seed]), format: "der", type: "pkcs8" });
  const jwk = createPublicKey(priv).export({ format: "jwk" }) as { x: string };
  const pubRaw = Buffer.from(jwk.x, "base64url");
  const hid = "bee_" + createHash("sha256").update(pubRaw).digest("hex");
  if (hid !== id) throw new Error(`the key filed as ${id} is not that bee's key`);
  const signedAt = Date.now();
  const signature = sign(null, Buffer.from(`${SOCKET_DOMAIN}:${hid}:${signedAt}`), priv).toString("hex");
  return { hid, from: hid, pubkey: pubRaw.toString("hex"), signedAt, signature };
}
