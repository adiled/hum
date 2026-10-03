//! delivery — when a tone arrives, and whether it arrives at all.
//!
//! Two independent rules, both applied per hop at the receiving edge:
//!
//! - **lifetime** (`dusk`): a tone past its own deadline is dead. It is
//!   not delivered and not re-fanned.
//! - **at-most-once** (`mid`): a tone whose id this ensemble has already
//!   dispatched is a duplicate. It is not delivered again.
//!
//! Both are optional per tone. A tone with neither is delivered as it
//! arrives, which is the default and what the plumbing tones rely on.
//!
//! `mid` is the originator's, minted once per logical message, and is
//! never `rid`. `rid` is correlation: a request and the response that
//! echoes its rid share one, so deduping on rid would drop every
//! response as a duplicate of its request. A response therefore carries
//! its *own* `mid` and the request's `rid` — the two are not redundant,
//! they answer different questions. See WIRE.md.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::Tone;

/// Bound on the per-ensemble `mid` seen-set, in entries. Sizing is a
/// memory/idle-time trade: the set only has to remember an id for as
/// long as a duplicate of it could still be in flight, which `dusk`
/// bounds from above. A tone with no `dusk` can be redelivered
/// arbitrarily late by a re-fan, so it is bounded by this cap instead.
pub const DELIVERY_SEEN_CAP: usize = 4096;

/// Ids of tones this ensemble has already dispatched, held as digests.
pub struct DeliveryState {
    seen: Mutex<LruCache<[u8; 32], ()>>,
}

impl DeliveryState {
    pub fn new() -> std::sync::Arc<Self> {
        Self::with_cap(DELIVERY_SEEN_CAP)
    }

    pub fn with_cap(cap: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            seen: Mutex::new(LruCache::new(NonZeroUsize::new(cap).expect("seen cap > 0"))),
        })
    }

    /// True if `mid` is new — i.e. the caller should dispatch it. Inserts
    /// on every call, so a second observation returns false.
    ///
    /// Separate from the gossip seen-set on purpose: a `mid` says "this
    /// node already delivered this tone", while a gossip `msg_id` also
    /// governs whether the tone is re-fanned. Sharing one set would
    /// couple two unrelated decisions and let one evict the other's
    /// entries early.
    pub fn note_mid(&self, mid: &str) -> bool {
        let key = mid_key(mid);
        let mut seen = self.seen.lock();
        if seen.contains(&key) {
            seen.get(&key);
            false
        } else {
            seen.put(key, ());
            true
        }
    }

    pub fn len(&self) -> usize {
        self.seen.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// True if `tone` is dead on arrival: it carries a `dusk` that has
/// already passed, so it must not be dispatched or re-fanned. Counts
/// the drop. A tone with no `dusk` never expires — the field is
/// optional in the envelope.
///
/// Each node applies the deadline it was handed, at its own edge, once.
fn drop_if_dusk(tone: &Tone, expired: &AtomicU64) -> bool {
    let past = tone
        .get("dusk")
        .and_then(|v| v.as_i64())
        .is_some_and(|dusk| crate::now_ms() > dusk);
    if past {
        expired.fetch_add(1, Ordering::SeqCst);
        tracing::debug!(
            target: "ensemble",
            chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or(""),
            "tone.dusk: dropped on arrival"
        );
    }
    past
}

/// Reduce a `mid` to a fixed 32-byte key before it goes in the set.
///
/// A `mid` is attacker-controlled and unbounded: the TCP transport reads
/// NDJSON with `BufReader::lines()`, which has no length cap, so a peer
/// can put a megabyte in one field. Capping the *entry count* therefore
/// does not bound the memory — 4096 entries of unbounded string is not
/// a bound. Digesting keeps dedup exact while making the footprint
/// `cap * 32` bytes no matter what arrives.
fn mid_key(mid: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(mid.as_bytes());
    h.finalize().into()
}

/// Enough of a `mid` to correlate a log line with a sender, without
/// copying an attacker-sized string into the log.
fn mid_prefix(mid: &str) -> String {
    mid.chars().take(12).collect()
}

/// The `mid` on a tone, if it carries one. Absent or non-string means
/// "no at-most-once claim", not "malformed" — the tone is delivered.
pub fn mid_of(tone: &Tone) -> Option<&str> {
    tone.get("mid").and_then(|v| v.as_str())
}

/// Apply both rules in order. Returns true if the tone must not be
/// dispatched or re-fanned.
///
/// Dusk is checked first so a dead tone never occupies seen-set
/// capacity, and so an id that is already known to be expired cannot
/// evict a live one.
pub fn admit(tone: &Tone, delivery: &DeliveryState, expired_dusk: &AtomicU64) -> bool {
    if drop_if_dusk(tone, expired_dusk) {
        return false;
    }
    match mid_of(tone) {
        Some(mid) if !delivery.note_mid(mid) => {
            tracing::debug!(
                target: "ensemble",
                mid = %mid_prefix(mid),
                chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or(""),
                "tone.mid: duplicate suppressed"
            );
            false
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn state() -> Arc<DeliveryState> {
        DeliveryState::new()
    }

    #[test]
    fn a_mid_is_admitted_once() {
        let d = state();
        assert!(d.note_mid("m1"));
        assert!(!d.note_mid("m1"));
        assert!(d.note_mid("m2"));
    }

    #[test]
    fn tones_without_a_mid_are_always_admitted() {
        let d = state();
        let expired = AtomicU64::new(0);
        let tone = json!({"chi": "prompt", "rid": "r-1"});
        assert!(admit(&tone, &d, &expired));
        assert!(admit(&tone, &d, &expired), "no mid means no claim to enforce");
        assert_eq!(d.len(), 0);
    }

    /// A request and its response share an rid but not a mid. Deduping
    /// on the rid would eat the response; deduping on the mid must not.
    #[test]
    fn a_response_echoing_a_request_rid_is_not_a_duplicate() {
        let d = state();
        let expired = AtomicU64::new(0);
        let request = json!({"chi": "prompt", "rid": "r-1", "mid": "m-req"});
        let response = json!({"chi": "chunk", "rid": "r-1", "mid": "m-resp"});
        assert!(admit(&request, &d, &expired));
        assert!(admit(&response, &d, &expired), "same rid, different mid");
    }

    #[test]
    fn a_retransmitted_mid_is_suppressed() {
        let d = state();
        let expired = AtomicU64::new(0);
        let tone = json!({"chi": "chunk", "rid": "r-1", "mid": "m-1"});
        assert!(admit(&tone, &d, &expired));
        assert!(!admit(&tone, &d, &expired));
        assert_eq!(expired.load(Ordering::SeqCst), 0, "not a dusk drop");
    }

    #[test]
    fn a_past_dusk_is_dropped_and_counted() {
        let d = state();
        let expired = AtomicU64::new(0);
        let tone = json!({"chi": "chunk", "rid": "r-1", "mid": "m-1", "dusk": crate::now_ms() - 1});
        assert!(!admit(&tone, &d, &expired));
        assert_eq!(expired.load(Ordering::SeqCst), 1);
        assert_eq!(d.len(), 0, "a dead tone must not occupy seen capacity");
    }

    #[test]
    fn a_future_dusk_is_delivered() {
        let d = state();
        let expired = AtomicU64::new(0);
        let tone = json!({"chi": "chunk", "rid": "r-1", "mid": "m-1", "dusk": crate::now_ms() + 60_000});
        assert!(admit(&tone, &d, &expired));
        assert_eq!(expired.load(Ordering::SeqCst), 0);
    }

    /// The set is bounded in entries AND in bytes. A `mid` is
    /// attacker-controlled and the transport has no frame cap, so the
    /// entry cap alone bounds nothing.
    #[test]
    fn an_enormous_mid_costs_a_fixed_32_bytes() {
        let d = state();
        let huge = "x".repeat(4 * 1024 * 1024);
        assert!(d.note_mid(&huge));
        assert!(!d.note_mid(&huge), "still dedups exactly");
        assert_eq!(d.len(), 1);
        // The key is the digest, not the string.
        assert_eq!(mid_key(&huge).len(), 32);
        assert_ne!(mid_key(&huge), mid_key(&"y".repeat(4 * 1024 * 1024)));
    }

    /// A multibyte `mid` must not panic the prefix used in logs.
    #[test]
    fn mid_prefix_is_char_safe() {
        assert_eq!(mid_prefix("short"), "short");
        assert_eq!(mid_prefix("é".repeat(50).as_str()), "é".repeat(12));
    }

    /// Eviction must actually happen at the cap — the whole reason the
    /// cap exists. Filling past it should evict the least recently seen
    /// and admit the newcomer.
    #[test]
    fn the_seen_set_evicts_at_its_cap() {
        let d = DeliveryState::with_cap(2);
        assert!(d.note_mid("a"));
        assert!(d.note_mid("b"));
        // Observing "a" is also its LRU touch, so "b" is now the victim.
        assert!(!d.note_mid("a"), "still remembered at cap");
        assert!(d.note_mid("c"), "c is new");
        assert_eq!(d.len(), 2, "cap holds");
        assert!(!d.note_mid("a"), "a survived");
        assert!(d.note_mid("b"), "b was the least recent, so it went");
    }

    /// A duplicate is only suppressed while the set can still remember
    /// it. Past the cap a very late retransmit is admitted again — the
    /// documented consequence of bounding memory, not a silent
    /// guarantee. `dusk` is what closes this window in practice.
    #[test]
    fn a_duplicate_past_the_cap_is_admitted_again() {
        let d = DeliveryState::with_cap(1);
        assert!(d.note_mid("old"));
        assert!(d.note_mid("new"));
        assert!(d.note_mid("old"), "forgotten — cap is 1");
    }

    /// Dusk is checked first, so a mid already known to be dead cannot
    /// displace a live one from the bounded set.
    #[test]
    fn a_dead_mid_does_not_evict_a_live_one() {
        let d = state();
        let expired = AtomicU64::new(0);
        let live = json!({"chi": "chunk", "rid": "r", "mid": "live"});
        assert!(admit(&live, &d, &expired));
        let dead = json!({"chi": "chunk", "rid": "r", "mid": "dead", "dusk": crate::now_ms() - 1});
        assert!(!admit(&dead, &d, &expired));
        assert_eq!(d.len(), 1);
        assert!(!admit(&live, &d, &expired), "the live mid is still remembered");
    }
}
