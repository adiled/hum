
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};

use lru::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::Tone;

pub const DELIVERY_SEEN_CAP: usize = 4096;

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

fn mid_key(mid: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(mid.as_bytes());
    h.finalize().into()
}

fn mid_prefix(mid: &str) -> String {
    mid.chars().take(12).collect()
}

pub fn mid_of(tone: &Tone) -> Option<&str> {
    tone.get("mid").and_then(|v| v.as_str())
}

pub fn dispatchable(tone: &Tone, delivery: &DeliveryState, expired_dusk: &AtomicU64) -> bool {
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
        assert!(dispatchable(&tone, &d, &expired));
        assert!(dispatchable(&tone, &d, &expired), "no mid means no claim to enforce");
        assert_eq!(d.len(), 0);
    }

    #[test]
    fn a_response_echoing_a_request_rid_is_not_a_duplicate() {
        let d = state();
        let expired = AtomicU64::new(0);
        let request = json!({"chi": "prompt", "rid": "r-1", "mid": "m-req"});
        let response = json!({"chi": "chunk", "rid": "r-1", "mid": "m-resp"});
        assert!(dispatchable(&request, &d, &expired));
        assert!(dispatchable(&response, &d, &expired), "same rid, different mid");
    }

    #[test]
    fn a_retransmitted_mid_is_suppressed() {
        let d = state();
        let expired = AtomicU64::new(0);
        let tone = json!({"chi": "chunk", "rid": "r-1", "mid": "m-1"});
        assert!(dispatchable(&tone, &d, &expired));
        assert!(!dispatchable(&tone, &d, &expired));
        assert_eq!(expired.load(Ordering::SeqCst), 0, "not a dusk drop");
    }

    #[test]
    fn a_past_dusk_is_dropped_and_counted() {
        let d = state();
        let expired = AtomicU64::new(0);
        let tone = json!({"chi": "chunk", "rid": "r-1", "mid": "m-1", "dusk": crate::now_ms() - 1});
        assert!(!dispatchable(&tone, &d, &expired));
        assert_eq!(expired.load(Ordering::SeqCst), 1);
        assert_eq!(d.len(), 0, "a dead tone must not occupy seen capacity");
    }

    #[test]
    fn a_future_dusk_is_delivered() {
        let d = state();
        let expired = AtomicU64::new(0);
        let tone = json!({"chi": "chunk", "rid": "r-1", "mid": "m-1", "dusk": crate::now_ms() + 60_000});
        assert!(dispatchable(&tone, &d, &expired));
        assert_eq!(expired.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_enormous_mid_costs_a_fixed_32_bytes() {
        let d = state();
        let huge = "x".repeat(4 * 1024 * 1024);
        assert!(d.note_mid(&huge));
        assert!(!d.note_mid(&huge), "still dedups exactly");
        assert_eq!(d.len(), 1);
        assert_eq!(mid_key(&huge).len(), 32);
        assert_ne!(mid_key(&huge), mid_key(&"y".repeat(4 * 1024 * 1024)));
    }

    #[test]
    fn mid_prefix_is_char_safe() {
        assert_eq!(mid_prefix("short"), "short");
        assert_eq!(mid_prefix("é".repeat(50).as_str()), "é".repeat(12));
    }

    #[test]
    fn the_seen_set_evicts_at_its_cap() {
        let d = DeliveryState::with_cap(2);
        assert!(d.note_mid("a"));
        assert!(d.note_mid("b"));
        assert!(!d.note_mid("a"), "still remembered at cap");
        assert!(d.note_mid("c"), "c is new");
        assert_eq!(d.len(), 2, "cap holds");
        assert!(!d.note_mid("a"), "a survived");
        assert!(d.note_mid("b"), "b was the least recent, so it went");
    }

    #[test]
    fn a_duplicate_past_the_cap_is_admitted_again() {
        let d = DeliveryState::with_cap(1);
        assert!(d.note_mid("old"));
        assert!(d.note_mid("new"));
        assert!(d.note_mid("old"), "forgotten — cap is 1");
    }

    #[test]
    fn a_dead_mid_does_not_evict_a_live_one() {
        let d = state();
        let expired = AtomicU64::new(0);
        let live = json!({"chi": "chunk", "rid": "r", "mid": "live"});
        assert!(dispatchable(&live, &d, &expired));
        let dead = json!({"chi": "chunk", "rid": "r", "mid": "dead", "dusk": crate::now_ms() - 1});
        assert!(!dispatchable(&dead, &d, &expired));
        assert_eq!(d.len(), 1);
        assert!(!dispatchable(&live, &d, &expired), "the live mid is still remembered");
    }
}
