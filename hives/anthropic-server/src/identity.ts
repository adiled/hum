// Bee identity — mirrors hives/common/src/identity.rs so a TS forager
// gets the same stable Hid the Rust hives do. humd dedupes a bee
// across reconnects by this hid; without it, every reconnect leaks a
// fresh manifest (ghost tools). The hid is `<prefix>_<hex>` where the
// hex is sha256(ed25519 public key) — identical to Hid::from_pubkey.
//
// The 32-byte ed25519 seed is persisted at
// $XDG_STATE_HOME/hum/bees/<kind>.key (else ~/.local/state/...), the
// same path + raw format the Rust side reads, so a hive can switch
// languages without changing identity.
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

// PKCS#8 DER prefix for an ed25519 private key; the 32-byte seed
// follows. Lets us reconstruct the keypair from a bare seed.
const PKCS8_ED25519_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

function keyPath(kind: string): string {
  const base = process.env.XDG_STATE_HOME
    ? join(process.env.XDG_STATE_HOME, "hum", "bees")
    : join(homedir(), ".local", "state", "hum", "bees");
  return join(base, `${kind}.key`);
}

/** Load (or mint + persist) the bee key and return its `<prefix>_<hex>` Hid. */
export function beeHid(kind: string, prefix: "fbee" | "wbee"): string {
  const path = keyPath(kind);
  let seed: Buffer;
  if (existsSync(path)) {
    seed = readFileSync(path);
    if (seed.length !== 32) throw new Error(`bee key ${path} is ${seed.length} bytes, expected 32`);
  } else {
    const der = generateKeyPairSync("ed25519").privateKey.export({ format: "der", type: "pkcs8" }) as Buffer;
    seed = Buffer.from(der.subarray(der.length - 32)); // last 32 bytes = raw seed
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, seed, { mode: 0o600 });
  }
  const priv = createPrivateKey({ key: Buffer.concat([PKCS8_ED25519_PREFIX, seed]), format: "der", type: "pkcs8" });
  const jwk = createPublicKey(priv).export({ format: "jwk" }) as { x: string };
  const pubRaw = Buffer.from(jwk.x, "base64url"); // 32-byte ed25519 pubkey
  return `${prefix}_` + createHash("sha256").update(pubRaw).digest("hex");
}
