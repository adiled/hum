//! Primitives — sigil, rid, dusk, echo.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::chi::Chi;
use crate::envelope::{Envelope, Tone};

/// Deterministic identity for a (nest, session) pair.
///
/// The `nest` slot is the nest-kind namespace — "claude-cli",
/// "claude-repl", or any other future nest implementation. Survives
/// restarts, reconnects, forks. Derived, not assigned. Returns the
/// lowercase hex of the first 6 sha256 bytes (12 chars).
pub fn sigil(sid: &str, nest: &str) -> String {
    let mut h = Sha256::new();
    h.update(nest.as_bytes());
    h.update(b":");
    h.update(sid.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6])
}

/// Wall-clock milliseconds since the Unix epoch. Clamped to non-negative.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

static RID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Correlation id — monotonic counter joined with a base36 ms timestamp.
///
/// Format matches the TS legacy: `"{tsBase36}-{counterBase36}"`.
pub fn rid() -> String {
    let ts = now_ms().max(0) as u64;
    let n = RID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}", to_base36(ts), to_base36(n))
}

fn to_base36(mut n: u64) -> String {
    if n == 0 {
        return "0".into();
    }
    const A: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::with_capacity(13);
    while n > 0 {
        out.push(A[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 alphabet is ascii")
}

/// Absolute ms timestamp at which a tone with `dusk = dusk_in(ms)` expires.
pub fn dusk_in(ms: i64) -> i64 {
    now_ms() + ms
}

/// True if the envelope's `dusk` is in the past.
pub fn is_dusk(env: &Envelope) -> bool {
    env.dusk.is_some_and(|d| now_ms() > d)
}

/// Build an Echo tone referring to `tone`'s rid.
///
/// Per chi.ts the echo frame is `{ chi, rid, ok, error? }` — the rest of
/// the envelope is intentionally absent.
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
    fn sigil_matches_ts_shape() {
        // sha256("claude:abc")[..12] hex, computed once: humd and the TS daemon
        // MUST agree byte-for-byte or sigils desync.
        let s = sigil("abc", "claude");
        assert_eq!(s.len(), 12);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn rid_is_unique_and_monotonic_within_ms() {
        let a = rid();
        let b = rid();
        assert_ne!(a, b);
        assert!(a.contains('-') && b.contains('-'));
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
