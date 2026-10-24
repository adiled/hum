// Same key file, seed format and signed message as hum-identity, so a bee
// keeps one identity across the languages it is written in.
import { createHash, createPrivateKey, createPublicKey, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const PKCS8_ED25519_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
const SOCKET_DOMAIN = "hum-bee-socket-v1";

export interface BeeProof {
  hid: string;
  from: string;
  pubkey: string;
  signedAt: number;
  signature: string;
}

export function signedAs(): BeeProof {
  const id = process.env.HUM_BEE?.trim() ?? "";
  if (!/^bee_[0-9a-f]{64}$/.test(id)) {
    throw new Error(
      `HUM_BEE ${JSON.stringify(id)} names no bee: its hive enters one with \`hum bee enter --hive <hive>\` and passes the id in HUM_BEE`,
    );
  }
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
