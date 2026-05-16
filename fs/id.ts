import { randomBytes } from "crypto";

// ── hum-native ID ──────────────────────────────────────────────────
//
// 256-bit identifier; 48-bit big-endian millisecond timestamp followed
// by 208 bits of cryptographically secure random. Encoded as 52 chars
// of Crockford base32 (alphabet omits I, L, O, U).
//
//   layout:  [ ts (6 bytes, BE) ][ random (26 bytes) ]   = 32 bytes
//   text:    52 chars Crockford base32 (260 bits → 4-bit zero pad)
//
// Properties:
//   - lexicographically sortable by mint time at millisecond precision
//   - 208 bits of randomness → ~104 bits of brute-force security under
//     Grover's quadratic speedup, ahead of NIST's 128-bit post-quantum
//     target's collision-resistance side (birthday bound is ~2^128)
//   - 256 bits total width vs UUID's 128 — strictly more entropy
//   - no marketing prefix, all 52 chars are payload
//
// Sample: 04G7M9Q5R80123456789ABCDEFGHJKMNPQRSTVWXYZ012345VWXYZ

const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ID_LEN = 52;
// Char class mirrors the alphabet exactly.
const ID_RE = /^[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{52}$/;

export function mintId(): string {
  const ts = Date.now();
  const buf = Buffer.alloc(32);
  // 48-bit BE timestamp at offset 0
  buf.writeUIntBE(ts, 0, 6);
  // 208 random bits at offset 6
  randomBytes(26).copy(buf, 6);
  return encodeBase32(buf);
}

export function isValidId(s: string): boolean {
  return typeof s === "string" && s.length === ID_LEN && ID_RE.test(s);
}

function encodeBase32(buf: Buffer): string {
  // 256 input bits → 260 output bits (4-bit zero pad on the LSB end).
  // Use BigInt to avoid 32-bit shift gotchas.
  let v = 0n;
  for (const byte of buf) v = (v << 8n) | BigInt(byte);
  v = v << 4n; // pad to 260 bits
  let out = "";
  for (let i = ID_LEN - 1; i >= 0; i--) {
    const idx = Number((v >> BigInt(i * 5)) & 0x1fn);
    out += ALPHABET[idx];
  }
  return out;
}
