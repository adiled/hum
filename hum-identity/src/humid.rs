//! HumId — the one canonical identifier hum mints.
//!
//! 256-bit, Crockford-base32 encoded (52 chars). Foreign formats are
//! projections via deterministic transform (`to_uuid_v5`).
//!
//! Three flavors, indistinguishable at the type level:
//! - `HumId::mint()` — ts-prefixed random (sessions, requests, calls)
//! - `HumId::from_hash(b)` — pure hash encoding (identity-derived)
//! - `HumId::from_foreign(s)` — deterministic hash of a foreign id
//!   (e.g. OC plugin's uuid → stable HumId without a bridge map)
//!
//! Layout: `[ ts (6 bytes, BE) ][ random (26 bytes) ] = 32 bytes`
//! Text:   52 chars Crockford base32 (260 bits → 4-bit zero pad on LSB end).
//!
//! Properties:
//! - lexicographically sortable by mint time at millisecond precision
//! - 208 bits of cryptographically secure randomness
//! - alphabet omits I, L, O, U (Crockford); uppercase only

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const NS_CLAUDE_SESSION: Uuid = Uuid::from_bytes([
    0x68, 0x75, 0x6d, 0x2d, 0x73, 0x65, 0x73, 0x73, 0x69, 0x6f, 0x6e, 0x2d, 0x6e, 0x73, 0x00, 0x01,
]);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HumId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    BadFormat,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadFormat => write!(f, "not a canonical HumId (52-char Crockford-base32)"),
        }
    }
}

impl std::error::Error for IdError {}

impl HumId {
    pub fn mint() -> Self {
        Self(mint_id())
    }

    pub fn from_hash(bytes: [u8; 32]) -> Self {
        Self(encode(&bytes))
    }

    /// Deterministic projection of a foreign string into HumId space.
    pub fn from_foreign(s: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"hum-foreign-id:");
        hasher.update(s.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        Self(encode(&digest))
    }

    pub fn parse(s: &str) -> Result<Self, IdError> {
        if is_valid_id(s) {
            Ok(Self(s.to_string()))
        } else {
            Err(IdError::BadFormat)
        }
    }

    pub fn timestamp(&self) -> Option<u64> {
        timestamp_of(&self.0)
    }

    /// Deterministic UUIDv5 projection, for boundaries that require
    /// UUID shape.
    pub fn to_uuid_v5(&self, namespace: Uuid) -> Uuid {
        Uuid::new_v5(&namespace, self.0.as_bytes())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for HumId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for HumId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for HumId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl From<HumId> for String {
    fn from(h: HumId) -> String {
        h.0
    }
}

impl Serialize for HumId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HumId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ID_LEN: usize = 52;

/// The 48-bit timestamp occupies the top 48 bits of the 260-bit value,
/// so it fills the first ⌈48 / 5⌉ = 10 characters. The tenth character
/// carries two bits of byte 6, which is random.
const TIMESTAMP_CHARS: usize = 10;

/// The tenth character's bottom two bits belong to the random portion,
/// so they are shifted back off.
const TRAILING_PAD_BITS: u32 = 2;

/// 256 input bits become 260 output bits: the buffer is treated as a
/// big-endian integer shifted left by 4, and 5-bit characters are
/// peeled from the top of that. The payload therefore starts
/// [`PAYLOAD_OFFSET_BITS`] into the shifted buffer.
const PAYLOAD_OFFSET_BITS: usize = 4;

pub fn mint_id() -> String {
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_millis() as u64;

    let mut buf = [0u8; 32];
    buf[0] = ((ts_ms >> 40) & 0xff) as u8;
    buf[1] = ((ts_ms >> 32) & 0xff) as u8;
    buf[2] = ((ts_ms >> 24) & 0xff) as u8;
    buf[3] = ((ts_ms >> 16) & 0xff) as u8;
    buf[4] = ((ts_ms >> 8) & 0xff) as u8;
    buf[5] = (ts_ms & 0xff) as u8;
    rand::thread_rng().fill_bytes(&mut buf[6..]);

    encode(&buf)
}

pub fn timestamp_of(id: &str) -> Option<u64> {
    if !is_valid_id(id) {
        return None;
    }
    let mut v: u64 = 0;
    for &c in id.as_bytes()[..TIMESTAMP_CHARS].iter() {
        let d = decode_char(c)?;
        v = (v << 5) | (d as u64);
    }
    Some(v >> TRAILING_PAD_BITS)
}

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
    let mut shifted = [0u8; 33];
    for i in 0..32 {
        shifted[i] |= buf[i] >> 4;
        shifted[i + 1] = (buf[i] & 0x0f) << 4;
    }
    let mut out = String::with_capacity(ID_LEN);
    for i in 0..ID_LEN {
        let bit = PAYLOAD_OFFSET_BITS + i * 5;
        let byte = bit / 8;
        let off = bit % 8;
        let hi = shifted[byte] as u16;
        let lo = *shifted.get(byte + 1).unwrap_or(&0) as u16;
        let window = (hi << 8) | lo;
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
        let ts_ms: u64 = 1_700_000_000_000;
        let mut buf = [0u8; 32];
        buf[0] = ((ts_ms >> 40) & 0xff) as u8;
        buf[1] = ((ts_ms >> 32) & 0xff) as u8;
        buf[2] = ((ts_ms >> 24) & 0xff) as u8;
        buf[3] = ((ts_ms >> 16) & 0xff) as u8;
        buf[4] = ((ts_ms >> 8) & 0xff) as u8;
        buf[5] = (ts_ms & 0xff) as u8;
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
        assert!(
            ts >= before && ts <= after,
            "ts={ts} before={before} after={after}"
        );
    }

    #[test]
    fn ts_parity_vectors() {
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
    fn hum_id_roundtrip_mint() {
        let h = HumId::mint();
        let s = h.as_str().to_string();
        assert_eq!(HumId::parse(&s).unwrap(), h);
        assert!(h.timestamp().is_some());
    }

    #[test]
    fn hum_id_from_foreign_is_deterministic() {
        let a = HumId::from_foreign("oc-session-abc");
        let b = HumId::from_foreign("oc-session-abc");
        let c = HumId::from_foreign("oc-session-xyz");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hum_id_to_uuid_v5_deterministic() {
        let h = HumId::parse(&mint_id()).unwrap();
        let u1 = h.to_uuid_v5(NS_CLAUDE_SESSION);
        let u2 = h.to_uuid_v5(NS_CLAUDE_SESSION);
        assert_eq!(u1, u2);
        assert_eq!(u1.get_version_num(), 5);
    }

    #[test]
    fn hum_id_serde_roundtrips() {
        let h = HumId::mint();
        let j = serde_json::to_string(&h).unwrap();
        let back: HumId = serde_json::from_str(&j).unwrap();
        assert_eq!(back, h);
        let bad: Result<HumId, _> = serde_json::from_str("\"not-a-real-id\"");
        assert!(bad.is_err());
    }

    #[test]
    fn rejects_bad_chars_and_lengths() {
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("ABC"));
        let bad: String = std::iter::repeat('I').take(52).collect();
        assert!(!is_valid_id(&bad));
        let short: String = std::iter::repeat('0').take(51).collect();
        assert!(!is_valid_id(&short));
    }
}
