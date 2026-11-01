//! Primitives — sigil, rid, dusk, echo.
//!
//! A sigil is the deterministic identity of a session, the same on every
//! humd. It survives restarts, reconnects and forks. It is derived, not
//! assigned: the lowercase hex of the first 6 sha256 bytes.
//!
//! The echo frame is `{ chi, rid, ok, error? }` — the rest of the
//! envelope is intentionally absent.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::chi::Chi;
use crate::envelope::{Envelope, Tone};

pub fn sigil(sid: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"sigil:");
    h.update(sid.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6])
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn rid() -> String { hum_identity::HumId::mint().to_string() }

pub fn dusk_in(ms: i64) -> i64 {
    now_ms() + ms
}

pub fn is_dusk(env: &Envelope) -> bool {
    env.dusk.is_some_and(|d| now_ms() > d)
}

pub fn echo_for(tone: &Tone, ok: bool, error: Option<String>) -> Tone {
    let envelope = Envelope::new(Chi::Echo, tone.envelope.rid.clone());
    let mut body = Map::new();
    body.insert("ok".into(), Value::Bool(ok));
    if let Some(e) = error {
        body.insert("error".into(), json!(e));
    }
    Tone::with_body(envelope, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigil_is_stable() {
        let s = sigil("abc");
        assert_eq!(s.len(), 12);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn rid_is_unique_and_canonical() {
        let a = rid();
        let b = rid();
        assert_ne!(a, b);
        assert!(hum_identity::HumId::parse(&a).is_ok());
        assert!(hum_identity::HumId::parse(&b).is_ok());
    }

    #[test]
    fn dusk_round_trip() {
        let env = Envelope { dusk: Some(now_ms() - 1000), ..Envelope::new(Chi::Hello, "r") };
        assert!(is_dusk(&env));
        let env2 = Envelope { dusk: Some(now_ms() + 60_000), ..Envelope::new(Chi::Hello, "r") };
        assert!(!is_dusk(&env2));
    }

    #[test]
    fn echo_shape() {
        let src = Tone::new(Envelope::new(Chi::Prompt, "rid-1"));
        let e = echo_for(&src, true, None);
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["chi"], "echo");
        assert_eq!(v["rid"], "rid-1");
        assert_eq!(v["ok"], true);
        assert!(v.get("error").is_none());
    }
}
