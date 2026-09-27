//! `gossip` — ensemble-wide pub-sub fan-out above the Transport seam.
//!
//! Each `PeerConnection` becomes a gossip neighbor. A `publish(topic,
//! payload)` mints one `chi:"gossip-publish"` tone and ships it to every
//! installed peer; their `install()` drainers see the chi, check the
//! `msg_id` against a bounded LRU "seen" set, dispatch to local
//! subscribers, and re-fan to every OTHER installed peer. Duplicates
//! short-circuit at the seen-set check, so the same `msg_id` never
//! crosses the same node twice.
//!
//! Sits ABOVE the unicast `route()` path — gossip is mesh-wide
//! announcements (hum relocated, humd overloaded, drone alerts);
//! `route()` stays the way to send to ONE specific humd. They share the
//! same `PeerConnection.send` wire but are semantically distinct.
//!
//! ## Why homegrown vs `libp2p-gossipsub`
//!
//! `libp2p-gossipsub` is a `NetworkBehaviour` glued to `libp2p::Swarm`.
//! Using it would mean either (a) bringing the full Swarm + libp2p
//! transport stack alongside our `Transport` trait — two parallel wire
//! abstractions — or (b) writing a `Transport` shim that pretends to be
//! a libp2p transport carrying our `PeerConnection`s. Both are heavier
//! than the v0 goal: light-touch fan-out with dedup, no peer scoring,
//! no eager/lazy push split, no IHAVE/IWANT gossip.
//!
//! What we DON'T get (and don't need yet): mesh maintenance heuristics,
//! peer scoring, message validation hooks, lazy pull gossip, graft/prune
//! topology messages. When the mesh grows past low-hundreds of peers per
//! topic, swapping in `libp2p-gossipsub` becomes worthwhile — the chi
//! and the `Ensemble::publish` / `subscribe_topic` surface stay the same.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::{Hid, Tone};

/// Wire-level chi string for gossip-publish tones. Mirrors
/// `thrum_core::Chi::GossipPublish` — kept as a literal here so the
/// ensemble crate doesn't pull the chi enum just to read one variant.
pub const GOSSIP_CHI: &str = "gossip-publish";

/// Bound on the per-Ensemble seen-set. ~1k entries keeps memory tiny
/// (each entry is a 32-char hex string) while comfortably covering any
/// realistic fan-out window across the mesh.
pub const GOSSIP_SEEN_CAP: usize = 1024;

/// Bound on per-topic broadcast::Sender capacity. Subscribers that fall
/// behind by more than this see `RecvError::Lagged` and resync from a
/// future message — same semantic the main inbox uses.
pub const GOSSIP_TOPIC_BUF: usize = 256;

/// State shared between `Ensemble::publish` / `subscribe_topic` and the
/// `install()` drainer task. Sits behind one `Arc` per ensemble so the
/// drainer can mutate the seen-set + dispatch to topic senders without
/// reaching back through `Ensemble`.
pub struct GossipState {
    /// LRU of recently-seen `msg_id` strings. Bounded — oldest evicted
    /// when capacity is hit.
    seen: Mutex<LruCache<String, ()>>,
    /// One `broadcast::Sender` per subscribed topic. Created lazily on
    /// the first `subscribe_topic(topic)` call and reused across
    /// subscribers. Senders persist for the ensemble's lifetime — the
    /// memory cost is one sender + receiver count per active topic.
    topics: Mutex<HashMap<String, broadcast::Sender<Value>>>,
}

impl GossipState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(LruCache::new(
                NonZeroUsize::new(GOSSIP_SEEN_CAP).expect("seen cap > 0"),
            )),
            topics: Mutex::new(HashMap::new()),
        })
    }

    /// Returns true if `msg_id` was not previously in the seen-set
    /// (i.e. caller should process + re-fan). Inserts on every call so
    /// the second observation of the same id returns false.
    pub fn note_seen(&self, msg_id: &str) -> bool {
        let mut seen = self.seen.lock();
        if seen.contains(msg_id) {
            // Touch for LRU recency, then signal duplicate.
            seen.get(msg_id);
            false
        } else {
            seen.put(msg_id.to_string(), ());
            true
        }
    }

    /// Subscribe to a topic, lazily creating the broadcast channel if
    /// this is the first subscriber for it.
    pub fn subscribe(&self, topic: &str) -> broadcast::Receiver<Value> {
        let mut topics = self.topics.lock();
        topics
            .entry(topic.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(GOSSIP_TOPIC_BUF);
                tx
            })
            .subscribe()
    }

    /// Lookup the sender for a topic — `None` if no subscribers have
    /// asked for it yet. Drainer uses this to decide whether to
    /// dispatch the payload locally (it still re-fans either way).
    pub fn sender(&self, topic: &str) -> Option<broadcast::Sender<Value>> {
        self.topics.lock().get(topic).cloned()
    }
}

/// Mint the wire `msg_id` for one publish: `{origin6}-{ms:x}-{seq:x}`.
///
/// The originator assigns this, and it is deliberately not derived from
/// the payload. Content addressing cannot tell "the network delivered
/// this twice" from "we published this twice on purpose", and a repeat
/// is ordinary for the tones gossip exists to carry — a heartbeat, a
/// standing overload alert, a retry after a resync. Hashing the payload
/// swallows those as duplicates; only the sender can know which it meant.
///
/// A per-process counter makes every publish distinct whatever the clock
/// or the content does, and the origin prefix keeps two humds from
/// minting the same id. Receivers dedup on this and never mint it
/// themselves.
pub fn mint_msg_id(from: &Hid) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let origin = &from.to_hex()[..12];
    format!("{origin}-{:x}-{seq:x}", crate::now_ms())
}

/// Build a `chi:"gossip-publish"` tone with the given fields. Kept here
/// so `Ensemble::publish` and the install drainer's re-fan path agree
/// on the wire shape.
pub fn gossip_tone(topic: &str, rid: &str, from: &Hid, payload: Value, msg_id: &str) -> Tone {
    serde_json::json!({
        "chi": GOSSIP_CHI,
        "rid": rid,
        "topic": topic,
        "payload": payload,
        "from": from.to_hex(),
        "msg_id": msg_id,
    })
}

/// Parsed view of an incoming gossip tone. Drainer pulls these fields
/// to decide whether to dispatch + re-fan; callers don't construct it.
pub struct ParsedGossip<'a> {
    pub topic: &'a str,
    pub msg_id: &'a str,
    pub payload: &'a Value,
}

/// Pull the gossip fields out of a tone. Returns `None` if any required
/// field is missing or the wrong type — the drainer treats that as
/// "not a gossip tone, fan into the regular inbox like everything else."
pub fn parse_gossip(tone: &Tone) -> Option<ParsedGossip<'_>> {
    let topic = tone.get("topic")?.as_str()?;
    let msg_id = tone.get("msg_id")?.as_str()?;
    let payload = tone.get("payload")?;
    Some(ParsedGossip { topic, msg_id, payload })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn msg_id_is_unique_per_publish() {
        let from = Hid::random_humd();
        let a = mint_msg_id(&from);
        let b = mint_msg_id(&from);
        assert_ne!(a, b, "two publishes must not share an id");
    }

    #[test]
    fn msg_id_carries_its_origin() {
        let a = mint_msg_id(&Hid::random_humd());
        let b = mint_msg_id(&Hid::random_humd());
        let origin = |id: &str| id.split('-').next().unwrap().to_string();
        assert_ne!(origin(&a), origin(&b), "two humds must not mint alike");
        assert_eq!(origin(&a).len(), 12);
    }

    #[test]
    fn seen_set_dedups_within_capacity() {
        let state = GossipState::new();
        assert!(state.note_seen("a"));
        assert!(!state.note_seen("a"));
        assert!(state.note_seen("b"));
        assert!(!state.note_seen("a"));
    }

    #[test]
    fn parse_gossip_pulls_fields() {
        let from = Hid::random_humd();
        let id = mint_msg_id(&from);
        let t = gossip_tone("topic", "r", &from, json!(1), &id);
        let p = parse_gossip(&t).unwrap();
        assert_eq!(p.topic, "topic");
        assert_eq!(p.msg_id, id);
        assert_eq!(p.payload, &json!(1));
    }
}
