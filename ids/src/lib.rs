//! hum-native ID — 256-bit identifier, Crockford-base32 encoded.
//!
//! Layout: `[ ts (6 bytes, BE) ][ random (26 bytes) ] = 32 bytes`
//! Text:   52 chars Crockford base32 (260 bits → 4-bit zero pad on LSB end).
//!
//! Properties:
//! - lexicographically sortable by mint time at millisecond precision
//! - 208 bits of cryptographically secure randomness
//! - alphabet omits I, L, O, U (Crockford); uppercase only

use rand::RngCore;
use std::time::{SystemTime, UNIX_EPOCH};

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ID_LEN: usize = 52;

/// Mint a fresh ID: 48-bit BE ms timestamp + 208 random bits, Crockford-base32 encoded.
pub fn mint_id() -> String {
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_millis() as u64;

    let mut buf = [0u8; 32];
    // 48-bit BE timestamp at offset 0..6
    buf[0] = ((ts_ms >> 40) & 0xff) as u8;
    buf[1] = ((ts_ms >> 32) & 0xff) as u8;
    buf[2] = ((ts_ms >> 24) & 0xff) as u8;
    buf[3] = ((ts_ms >> 16) & 0xff) as u8;
    buf[4] = ((ts_ms >> 8) & 0xff) as u8;
    buf[5] = (ts_ms & 0xff) as u8;
    rand::thread_rng().fill_bytes(&mut buf[6..]);

    encode(&buf)
}

/// Decode the 48-bit timestamp prefix back to ms since epoch.
pub fn timestamp_of(id: &str) -> Option<u64> {
    if !is_valid_id(id) {
        return None;
    }
    // The first 48 bits live in the top 48 bits of a 260-bit number, which
    // means they occupy the first ⌈48 / 5⌉ = 10 chars, with the 10th char's
    // bottom 2 bits belonging to byte 6 (random). Pull the top 50 bits and
    // shift off the trailing 2.
    let mut v: u64 = 0;
    for &c in id.as_bytes()[..10].iter() {
        let d = decode_char(c)?;
        v = (v << 5) | (d as u64);
    }
    // v now holds the top 50 bits of the 260-bit value. Strip the bottom 2.
    Some(v >> 2)
}

/// 52 chars in the Crockford alphabet (0-9, A-Z minus I, L, O, U).
pub fn is_valid_id(s: &str) -> bool {
    if s.len() != ID_LEN {
        return false;
    }
    s.bytes().all(|b| decode_char(b).is_some())
}

fn decode_char(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'H' => Some(c - b'A' + 10),
        b'J' | b'K' => Some(c - b'J' + 18),
        b'M' | b'N' => Some(c - b'M' + 20),
        b'P'..=b'T' => Some(c - b'P' + 22),
        b'V'..=b'Z' => Some(c - b'V' + 27),
        _ => None,
    }
}

fn encode(buf: &[u8; 32]) -> String {
    // 256 input bits → 260 output bits; we treat the buffer as a big-endian
    // integer shifted left 4, then peel 5-bit chunks from the top.
    // Use a small big-int rep: 33 bytes after the 4-bit left shift.
    let mut shifted = [0u8; 33];
    for i in 0..32 {
        shifted[i] |= buf[i] >> 4;
        shifted[i + 1] = (buf[i] & 0x0f) << 4;
    }
    // `shifted` is a 264-bit big-endian buffer holding our 260-bit payload
    // right-aligned (top 4 bits are zero pad). Char 0 takes the top 5 bits
    // of the payload, which begin 4 bits into `shifted` (skipping the pad).
    let mut out = String::with_capacity(ID_LEN);
    for i in 0..ID_LEN {
        let bit = 4 + i * 5;
        let byte = bit / 8;
        let off = bit % 8;
        // grab 5 bits starting at (byte, off), MSB-first
        let hi = shifted[byte] as u16;
        let lo = *shifted.get(byte + 1).unwrap_or(&0) as u16;
        let window = (hi << 8) | lo; // 16 bits, big-endian
        let idx = ((window >> (11 - off)) & 0x1f) as usize;
        out.push(ALPHABET[idx] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_known_timestamp() {
        // Pick a fixed ms value and verify timestamp_of recovers it from a
        // freshly built ID with that prefix.
        let ts_ms: u64 = 1_700_000_000_000;
        let mut buf = [0u8; 32];
        buf[0] = ((ts_ms >> 40) & 0xff) as u8;
        buf[1] = ((ts_ms >> 32) & 0xff) as u8;
        buf[2] = ((ts_ms >> 24) & 0xff) as u8;
        buf[3] = ((ts_ms >> 16) & 0xff) as u8;
        buf[4] = ((ts_ms >> 8) & 0xff) as u8;
        buf[5] = (ts_ms & 0xff) as u8;
        // fill random portion with something non-zero to ensure prefix decode
        // doesn't accidentally rely on zeros
        for (i, b) in buf[6..].iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(13);
        }
        let id = encode(&buf);
        assert_eq!(id.len(), ID_LEN);
        assert!(is_valid_id(&id));
        assert_eq!(timestamp_of(&id), Some(ts_ms));
    }

    #[test]
    fn mint_is_valid_and_recent() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let id = mint_id();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(is_valid_id(&id));
        let ts = timestamp_of(&id).expect("timestamp decodes");
        assert!(ts >= before && ts <= after, "ts={ts} before={before} after={after}");
    }

    #[test]
    fn ts_parity_vectors() {
        // Reference vectors produced by lib/id.ts encodeBase32() in Node.
        // Keeps the Rust encoder bit-identical to the TS implementation.
        let zero = [0u8; 32];
        assert_eq!(encode(&zero), "0".repeat(52));

        let mut ts_only = [0u8; 32];
        let ts_ms: u64 = 1_700_000_000_000;
        ts_only[0] = ((ts_ms >> 40) & 0xff) as u8;
        ts_only[1] = ((ts_ms >> 32) & 0xff) as u8;
        ts_only[2] = ((ts_ms >> 24) & 0xff) as u8;
        ts_only[3] = ((ts_ms >> 16) & 0xff) as u8;
        ts_only[4] = ((ts_ms >> 8) & 0xff) as u8;
        ts_only[5] = (ts_ms & 0xff) as u8;
        assert_eq!(
            encode(&ts_only),
            "065WZSB800000000000000000000000000000000000000000000"
        );

        let ones = [0xffu8; 32];
        assert_eq!(
            encode(&ones),
            "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZG"
        );
    }

    #[test]
    fn rejects_bad_chars_and_lengths() {
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("ABC"));
        // 52 chars but contains 'I' (excluded from Crockford alphabet)
        let bad: String = std::iter::repeat('I').take(52).collect();
        assert!(!is_valid_id(&bad));
        // 51 chars, all valid alphabet
        let short: String = std::iter::repeat('0').take(51).collect();
        assert!(!is_valid_id(&short));
    }
}
