
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::{Hid, Tone};

pub const GOSSIP_CHI: &str = "gossip-publish";

pub const GOSSIP_SEEN_CAP: usize = 1024;

pub const GOSSIP_TOPIC_BUF: usize = 256;

const MSG_ID_ORIGIN_CHARS: usize = 12;

pub struct GossipState {
    seen: Mutex<LruCache<String, ()>>,
    topics: Mutex<HashMap<String, broadcast::Sender<Value>>>,
}

impl GossipState {
    pub fn new() -> Arc<Self> {
        Self::with_cap(GOSSIP_SEEN_CAP)
    }

    pub fn with_cap(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(LruCache::new(NonZeroUsize::new(cap).expect("seen cap > 0"))),
            topics: Mutex::new(HashMap::new()),
        })
    }

    pub fn note_seen(&self, msg_id: &str) -> bool {
        let mut seen = self.seen.lock();
        if seen.contains(msg_id) {
            seen.get(msg_id);
            false
        } else {
            seen.put(msg_id.to_string(), ());
            true
        }
    }

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

    pub fn sender(&self, topic: &str) -> Option<broadcast::Sender<Value>> {
        self.topics.lock().get(topic).cloned()
    }
}

pub fn mint_msg_id(from: &Hid) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let hex = from.to_hex();
    let origin: String = hex.chars().take(MSG_ID_ORIGIN_CHARS).collect();
    let origin = origin.as_str();
    format!("{origin}-{:x}-{seq:x}", crate::now_ms())
}

pub fn gossip_tone_with_dusk(
    topic: &str,
    rid: &str,
    from: &Hid,
    payload: Value,
    msg_id: &str,
    dusk_ms: Option<i64>,
) -> Tone {
    let mut tone = serde_json::json!({
        "chi": GOSSIP_CHI,
        "rid": rid,
        "topic": topic,
        "payload": payload,
        "from": from.to_hex(),
        "msg_id": msg_id,
    });
    if let Some(dusk) = dusk_ms {
        tone.as_object_mut()
            .expect("gossip tone is an object")
            .insert("dusk".into(), serde_json::json!(crate::now_ms() + dusk));
    }
    tone
}

pub fn gossip_tone(topic: &str, rid: &str, from: &Hid, payload: Value, msg_id: &str) -> Tone {
    gossip_tone_with_dusk(topic, rid, from, payload, msg_id, None)
}

pub struct ParsedGossip<'a> {
    pub topic: &'a str,
    pub msg_id: &'a str,
    pub payload: &'a Value,
}

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
        assert_eq!(origin(&a).len(), MSG_ID_ORIGIN_CHARS);
    }

    #[test]
    fn seen_set_dedups_repeats() {
        let state = GossipState::new();
        assert!(state.note_seen("a"));
        assert!(!state.note_seen("a"));
        assert!(state.note_seen("b"));
        assert!(!state.note_seen("a"));
    }

    #[test]
    fn seen_set_evicts_at_its_cap() {
        let state = GossipState::with_cap(2);
        assert!(state.note_seen("a"));
        assert!(state.note_seen("b"));
        assert!(state.note_seen("c"), "c is new");
        assert!(state.note_seen("a"), "a was least recent, so evicted");
        assert!(!state.note_seen("c"), "c survived");
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
