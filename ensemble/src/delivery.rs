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

use crate::Tone;

/// Bound on the per-ensemble `mid` seen-set. Sizing is a memory/idle-
/// time trade: the set only has to remember an id for as long as a
/// duplicate of it could still be in flight, which `dusk` bounds from
/// above. A tone with no `dusk` can be redelivered arbitrarily late by
/// a re-fan, so it is bounded only by this cap.
pub const DELIVERY_SEEN_CAP: usize = 4096;

/// Ids of tones this ensemble has already dispatched.
pub struct DeliveryState {
    seen: Mutex<LruCache<String, ()>>,
}

impl DeliveryState {
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            seen: Mutex::new(LruCache::new(
                NonZeroUsize::new(DELIVERY_SEEN_CAP).expect("seen cap > 0"),
            )),
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
        let mut seen = self.seen.lock();
        if seen.contains(mid) {
            seen.get(mid);
            false
        } else {
            seen.put(mid.to_string(), ());
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
                mid,
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
