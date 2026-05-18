//! `kad` — homegrown Kademlia DHT for Hid → HumdAddr discovery.
//!
//! T4 lookup layer above the [`crate::Transport`] seam, parallel to the
//! gossip fan-out. Goal is the same one Kademlia solves in the paper:
//! given a [`Hid`] you don't already have a connection to, find a
//! [`HumdAddr`] for it by iteratively asking peers progressively closer
//! to the target in XOR space. peers.json gives us the static T1/T2
//! seed list; this gives us the dynamic T4 dial-the-stranger surface.
//!
//! ## Why homegrown vs `libp2p-kad`
//!
//! `libp2p-kad` is a `NetworkBehaviour` glued to `libp2p::Swarm` with
//! its own RPC framing, its own peer-id space, its own iterative-lookup
//! state machine, its own k-bucket layout. Plugging it in means either
//! standing the full Swarm runtime up alongside our `Transport` trait
//! or writing a libp2p-transport shim that smuggles our
//! [`PeerConnection`]s through. Same architectural trade we made in
//! [`crate::gossip`] — two parallel wire abstractions, or a shim that
//! pretends.
//!
//! Instead: ~500 LoC, XOR distance over the 32-byte Hid, a 256-slot
//! routing table (one [`KBucket`] per leading-zero distance bucket), an
//! iterative FIND_NODE that fans out α=3 queries in parallel and
//! terminates when no nearer peer is returned. Two new chi values
//! (`kad-find-node`, `kad-find-node-resp`) carry queries over peer
//! `send()` — exactly the same wire path gossip uses.
//!
//! ## What we DO get
//! - O(log n) lookup in expectation across the mesh.
//! - Bounded-memory routing table (K=20 per bucket × 256 buckets max).
//! - Convergence to closer peers as the lookup progresses.
//!
//! ## What we DON'T get yet
//! - No ping-based liveness eviction — LRU is "last inserted wins."
//! - No periodic bucket refresh, no random-id self-lookups.
//! - No replication / `STORE` — this is pure node lookup (FIND_NODE).
//! - No security: a hostile peer can return arbitrary HumdAddrs and
//!   we trust them. Real DHTs layer S/Kademlia or Coral on top.
//!
//! When the mesh grows past ~thousands of nodes, or we need value
//! storage, swapping in `libp2p-kad` becomes worthwhile. The chi
//! shapes and the [`crate::Ensemble::kad_find`] surface stay stable.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rand::RngCore;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::{HumdAddr, Hid, Tone};

/// Kademlia replication / bucket-size parameter (paper default).
pub const KAD_K: usize = 20;

/// Parallelism for iterative lookups (paper default).
pub const KAD_ALPHA: usize = 3;

/// Hard cap on FIND_NODE rounds in a single `kad_find` call. Bounded so
/// a pathological mesh can't spin forever; lookups normally terminate
/// in O(log n) rounds well below this.
pub const KAD_MAX_ROUNDS: usize = 16;

/// Wire-level chi for FIND_NODE query. Mirrors [`thrum_core::Chi::KadFindNode`].
pub const KAD_FIND_NODE_CHI: &str = "kad-find-node";

/// Wire-level chi for FIND_NODE response. Mirrors
/// [`thrum_core::Chi::KadFindNodeResp`].
pub const KAD_FIND_NODE_RESP_CHI: &str = "kad-find-node-resp";

// ── Distance ───────────────────────────────────────────────────────────────

/// XOR distance helpers. Distance is `a ^ b` byte-wise; ordering is
/// lexicographic on the resulting 32 bytes (so leading-zero-count is the
/// natural bucket index).
pub struct XorDistance;

impl XorDistance {
    /// Byte-wise XOR of two HumdIds. The lower the value (treated as a
    /// 256-bit big-endian number), the closer the two ids.
    pub fn distance(a: &Hid, b: &Hid) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = a.as_bytes()[i] ^ b.as_bytes()[i];
        }
        out
    }

    /// Index of the bucket that holds `peer` relative to `me` — the
    /// count of leading zero bits in `distance(me, peer)`. Range
    /// `0..=255`. By convention `me == peer` returns `256` (no bucket);
    /// callers filter `me` out before bucket placement.
    pub fn bucket_index(me: &Hid, peer: &Hid) -> usize {
        let d = Self::distance(me, peer);
        for (i, byte) in d.iter().enumerate() {
            if *byte != 0 {
                return i * 8 + byte.leading_zeros() as usize;
            }
        }
        256
    }
}

// ── K-bucket ───────────────────────────────────────────────────────────────

/// A single Kademlia k-bucket: up to K [`HumdAddr`] entries ordered by
/// recency. New entries push to the back; on overflow we evict from the
/// front (LRU). Real Kademlia would ping the LRU first and only evict
/// on no-response — that's a follow-up.
#[derive(Debug, Default, Clone)]
pub struct KBucket {
    entries: VecDeque<HumdAddr>,
}

impl KBucket {
    pub fn new() -> Self {
        Self { entries: VecDeque::with_capacity(KAD_K) }
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn iter(&self) -> impl Iterator<Item = &HumdAddr> { self.entries.iter() }

    /// Insert (or refresh) `addr`. If the bucket already holds an entry
    /// with the same id, that entry is moved to the back (most-recent
    /// slot) and the hints are merged from the new entry — keeping the
    /// most recently advertised contact info. If the bucket is full,
    /// the oldest entry is evicted.
    pub fn insert(&mut self, addr: HumdAddr) {
        if let Some(pos) = self.entries.iter().position(|e| e.id == addr.id) {
            // Refresh recency: pull existing out, merge hints, push back.
            let mut existing = self.entries.remove(pos).expect("position valid");
            for hint in &addr.hints {
                if !existing.hints.iter().any(|h| h == hint) {
                    existing.hints.push(hint.clone());
                }
            }
            self.entries.push_back(existing);
            return;
        }
        if self.entries.len() >= KAD_K {
            self.entries.pop_front();
        }
        self.entries.push_back(addr);
    }
}

// ── Routing table ──────────────────────────────────────────────────────────

/// 256 k-buckets keyed by leading-zero count of XOR distance to `me`.
/// Bucket `i` holds peers whose distance to `me` has exactly `i`
/// leading zero bits — i.e. peers in a shell `2^(255-i) ≤ d < 2^(256-i)`.
#[derive(Debug)]
pub struct RoutingTable {
    me: Hid,
    buckets: Vec<KBucket>,
}

impl RoutingTable {
    pub fn new(me: Hid) -> Self {
        Self {
            me,
            buckets: (0..256).map(|_| KBucket::new()).collect(),
        }
    }

    pub fn me(&self) -> Hid { self.me }

    /// Insert `addr` into the appropriate bucket. Inserting the table's
    /// own id is a no-op (returns false). Returns true on insert/refresh.
    pub fn insert(&mut self, addr: HumdAddr) -> bool {
        if addr.id == self.me {
            return false;
        }
        let idx = XorDistance::bucket_index(&self.me, &addr.id);
        if idx >= 256 {
            return false;
        }
        self.buckets[idx].insert(addr);
        true
    }

    /// Return up to `count` peers sorted by XOR-distance to `target`
    /// (closest first). Walks every bucket — cheap because the table is
    /// bounded at K × 256 = 5120 entries worst case.
    pub fn closest_to(&self, target: &Hid, count: usize) -> Vec<HumdAddr> {
        let mut all: Vec<HumdAddr> = self
            .buckets
            .iter()
            .flat_map(|b| b.iter().cloned())
            .collect();
        all.sort_by(|a, b| {
            XorDistance::distance(target, &a.id).cmp(&XorDistance::distance(target, &b.id))
        });
        all.truncate(count);
        all
    }

    /// Total number of peers currently in the table.
    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Direct lookup: does the table hold a HumdAddr for `id`?
    pub fn get(&self, id: &Hid) -> Option<HumdAddr> {
        let idx = XorDistance::bucket_index(&self.me, id);
        if idx >= 256 {
            return None;
        }
        self.buckets[idx]
            .iter()
            .find(|a| &a.id == id)
            .cloned()
    }
}

// ── State shared with the install drainer ──────────────────────────────────

/// Routing table + pending in-flight FIND_NODE queries. One `Arc<KadState>`
/// per [`crate::Ensemble`]; cloned into every drainer task so kad chis
/// can notify outstanding lookups without reaching back through `Ensemble`.
pub struct KadState {
    pub table: Mutex<RoutingTable>,
    /// Pending queries keyed by `query_id`. Each FIND_NODE we send
    /// registers a oneshot here; the matching resp populates it.
    pending: Mutex<HashMap<String, oneshot::Sender<Vec<HumdAddr>>>>,
}

impl KadState {
    pub fn new(me: Hid) -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(RoutingTable::new(me)),
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// Stash a HumdAddr into the routing table. Used at install-time
    /// (bootstrap a peer we just connected to) and when FIND_NODE resps
    /// advertise new peers.
    pub fn note_peer(&self, addr: HumdAddr) {
        self.table.lock().insert(addr);
    }

    /// Register a pending query; the returned oneshot fires when the
    /// matching `kad-find-node-resp` arrives (or when the caller drops
    /// the receiver — timeout path).
    pub fn register_query(&self, query_id: String) -> oneshot::Receiver<Vec<HumdAddr>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(query_id, tx);
        rx
    }

    /// Drop a pending query (timeout / lookup gave up). Best-effort; if
    /// the resp races in after this, the oneshot's `send` returns Err
    /// and we silently drop.
    pub fn cancel_query(&self, query_id: &str) {
        self.pending.lock().remove(query_id);
    }

    /// Deliver a FIND_NODE response payload to a pending query if one
    /// exists. Returns true if a waiter was notified.
    pub fn deliver_response(&self, query_id: &str, closest: Vec<HumdAddr>) -> bool {
        let tx = self.pending.lock().remove(query_id);
        if let Some(tx) = tx {
            let _ = tx.send(closest);
            true
        } else {
            false
        }
    }

    pub fn closest_to(&self, target: &Hid, count: usize) -> Vec<HumdAddr> {
        self.table.lock().closest_to(target, count)
    }

    pub fn get(&self, id: &Hid) -> Option<HumdAddr> {
        self.table.lock().get(id)
    }
}

// ── Wire shapes ────────────────────────────────────────────────────────────

/// Mint a fresh random query_id as 32 hex chars (16 random bytes).
/// Collision probability is negligible across a single lookup window.
pub fn mint_query_id() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Build a `chi:"kad-find-node"` tone.
pub fn find_node_tone(rid: &str, query_id: &str, target: &Hid, from: &Hid) -> Tone {
    serde_json::json!({
        "chi": KAD_FIND_NODE_CHI,
        "rid": rid,
        "query_id": query_id,
        "target": target.to_hex(),
        "from": from.to_hex(),
    })
}

/// Build a `chi:"kad-find-node-resp"` tone carrying up to K HumdAddrs.
pub fn find_node_resp_tone(
    rid: &str,
    query_id: &str,
    from: &Hid,
    closest: &[HumdAddr],
) -> Tone {
    serde_json::json!({
        "chi": KAD_FIND_NODE_RESP_CHI,
        "rid": rid,
        "query_id": query_id,
        "from": from.to_hex(),
        "closest": closest,
    })
}

/// Parsed FIND_NODE query. Drainer pulls these to respond.
pub struct ParsedFindNode {
    pub query_id: String,
    pub target: Hid,
    pub from: Hid,
    pub rid: String,
}

/// Parsed FIND_NODE response. Drainer pulls these to deliver to waiters.
pub struct ParsedFindNodeResp {
    pub query_id: String,
    pub from: Hid,
    pub closest: Vec<HumdAddr>,
}

fn parse_humd_id(v: &Value) -> Option<Hid> {
    Hid::from_hex(v.as_str()?).ok()
}

/// Pull fields out of a `chi:"kad-find-node"` tone.
pub fn parse_find_node(tone: &Tone) -> Option<ParsedFindNode> {
    let query_id = tone.get("query_id")?.as_str()?.to_string();
    let target = parse_humd_id(tone.get("target")?)?;
    let from = parse_humd_id(tone.get("from")?)?;
    let rid = tone
        .get("rid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(ParsedFindNode { query_id, target, from, rid })
}

/// Pull fields out of a `chi:"kad-find-node-resp"` tone.
pub fn parse_find_node_resp(tone: &Tone) -> Option<ParsedFindNodeResp> {
    let query_id = tone.get("query_id")?.as_str()?.to_string();
    let from = parse_humd_id(tone.get("from")?)?;
    let closest_json = tone.get("closest")?.as_array()?;
    let closest: Vec<HumdAddr> = closest_json
        .iter()
        .filter_map(|v| serde_json::from_value::<HumdAddr>(v.clone()).ok())
        .collect();
    Some(ParsedFindNodeResp { query_id, from, closest })
}

// ── Lookup driver ──────────────────────────────────────────────────────────

/// Result of one `kad_find` call.
#[derive(Debug, Clone)]
pub enum KadFindOutcome {
    /// Target was found — either already in our routing table or
    /// advertised by a peer's FIND_NODE response.
    Found(HumdAddr),
    /// Lookup converged (no closer peer in α latest rounds) without
    /// hitting the exact target.
    Exhausted,
    /// Wall-clock timeout fired before the lookup converged.
    TimedOut,
}

impl KadFindOutcome {
    pub fn into_option(self) -> Option<HumdAddr> {
        match self {
            KadFindOutcome::Found(addr) => Some(addr),
            _ => None,
        }
    }
}

/// Internal: track the lookup's working set across rounds.
pub(crate) struct LookupShortlist {
    /// Candidates known so far, sorted by distance to target (closest first).
    /// Bounded by K to mirror a Kademlia "shortlist."
    shortlist: Vec<HumdAddr>,
    /// Set of HumdIds we've already queried in this lookup — never re-query.
    queried: std::collections::HashSet<Hid>,
    target: Hid,
}

impl LookupShortlist {
    pub fn new(target: Hid, seed: Vec<HumdAddr>) -> Self {
        let mut s = Self {
            shortlist: Vec::new(),
            queried: std::collections::HashSet::new(),
            target,
        };
        for addr in seed {
            s.insert(addr);
        }
        s
    }

    pub fn insert(&mut self, addr: HumdAddr) {
        if self.shortlist.iter().any(|a| a.id == addr.id) {
            return;
        }
        self.shortlist.push(addr);
        let target = self.target;
        self.shortlist.sort_by(|a, b| {
            XorDistance::distance(&target, &a.id).cmp(&XorDistance::distance(&target, &b.id))
        });
        self.shortlist.truncate(KAD_K);
    }

    /// Up to α not-yet-queried entries from the closest end of the shortlist.
    pub fn next_unqueried(&self, alpha: usize) -> Vec<HumdAddr> {
        self.shortlist
            .iter()
            .filter(|a| !self.queried.contains(&a.id))
            .take(alpha)
            .cloned()
            .collect()
    }

    pub fn mark_queried(&mut self, id: Hid) {
        self.queried.insert(id);
    }

    /// Closest distance seen so far (or all-ones sentinel if empty).
    pub fn closest_distance(&self) -> [u8; 32] {
        self.shortlist
            .first()
            .map(|a| XorDistance::distance(&self.target, &a.id))
            .unwrap_or([0xff; 32])
    }

    pub fn closest(&self) -> Option<&HumdAddr> {
        self.shortlist.first()
    }
}

/// Spin one FIND_NODE query against a single peer connection. Sends the
/// tone, awaits the matching `kad-find-node-resp` (registered on
/// `kad`), and returns the advertised list — or empty on timeout.
pub(crate) async fn query_peer(
    kad: &Arc<KadState>,
    conn: &Arc<dyn crate::PeerConnection>,
    me: &Hid,
    target: &Hid,
    per_query_timeout: Duration,
) -> Vec<HumdAddr> {
    let query_id = mint_query_id();
    let rid = format!("kad-find-{}", &query_id[..8]);
    let rx = kad.register_query(query_id.clone());
    let tone = find_node_tone(&rid, &query_id, target, me);
    if conn.send(tone).await.is_err() {
        kad.cancel_query(&query_id);
        return Vec::new();
    }
    match tokio::time::timeout(per_query_timeout, rx).await {
        Ok(Ok(closest)) => closest,
        _ => {
            kad.cancel_query(&query_id);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_distance_is_symmetric() {
        let a = Hid::random_humd();
        let b = Hid::random_humd();
        assert_eq!(XorDistance::distance(&a, &b), XorDistance::distance(&b, &a));
        assert_eq!(XorDistance::distance(&a, &a), [0u8; 32]);
    }

    #[test]
    fn bucket_index_zero_for_self() {
        let me = Hid::random_humd();
        assert_eq!(XorDistance::bucket_index(&me, &me), 256);
    }

    #[test]
    fn bucket_index_high_for_close_ids() {
        // Flip the very last bit — distance = 0x00..01 → 255 leading zeros.
        let mut bytes = [0u8; 32];
        bytes[31] = 0x01;
        let me = Hid::from([0u8; 32]);
        let other = Hid::from(bytes);
        assert_eq!(XorDistance::bucket_index(&me, &other), 255);
    }

    #[test]
    fn bucket_index_zero_for_inverted_top_bit() {
        // Flip the top bit — distance = 0x80..00 → 0 leading zeros.
        let mut bytes = [0u8; 32];
        bytes[0] = 0x80;
        let me = Hid::from([0u8; 32]);
        let other = Hid::from(bytes);
        assert_eq!(XorDistance::bucket_index(&me, &other), 0);
    }

    #[test]
    fn kbucket_lru_evicts_oldest_when_full() {
        let mut b = KBucket::new();
        let ids: Vec<Hid> = (0..KAD_K + 1).map(|_| Hid::random_humd()).collect();
        for id in &ids {
            b.insert(HumdAddr::new(*id));
        }
        assert_eq!(b.len(), KAD_K);
        // Oldest (ids[0]) should have been evicted; newest (last) retained.
        assert!(b.iter().all(|a| a.id != ids[0]));
        assert!(b.iter().any(|a| a.id == *ids.last().unwrap()));
    }

    #[test]
    fn kbucket_refresh_moves_to_back() {
        let mut b = KBucket::new();
        let a_id = Hid::random_humd();
        let b_id = Hid::random_humd();
        b.insert(HumdAddr::new(a_id));
        b.insert(HumdAddr::new(b_id));
        // Re-insert a — should move to back (most-recent slot).
        b.insert(HumdAddr::new(a_id));
        let order: Vec<Hid> = b.iter().map(|x| x.id).collect();
        assert_eq!(order, vec![b_id, a_id]);
    }

    #[test]
    fn routing_table_closest_orders_by_distance() {
        let me = Hid::from([0u8; 32]);
        let mut t = RoutingTable::new(me);
        // Three peers at varying distances.
        let mut near = [0u8; 32];
        near[31] = 0x01;
        let mut mid = [0u8; 32];
        mid[15] = 0x01;
        let mut far = [0u8; 32];
        far[0] = 0x80;

        t.insert(HumdAddr::new(Hid::from(far)));
        t.insert(HumdAddr::new(Hid::from(near)));
        t.insert(HumdAddr::new(Hid::from(mid)));

        let closest = t.closest_to(&me, 3);
        assert_eq!(closest.len(), 3);
        assert_eq!(closest[0].id, Hid::from(near));
        assert_eq!(closest[1].id, Hid::from(mid));
        assert_eq!(closest[2].id, Hid::from(far));
    }

    #[test]
    fn routing_table_drops_self_inserts() {
        let me = Hid::random_humd();
        let mut t = RoutingTable::new(me);
        assert!(!t.insert(HumdAddr::new(me)));
        assert!(t.is_empty());
    }

    #[test]
    fn parse_find_node_round_trip() {
        let me = Hid::random_humd();
        let target = Hid::random_humd();
        let q = mint_query_id();
        let tone = find_node_tone("rid-1", &q, &target, &me);
        let parsed = parse_find_node(&tone).expect("parse");
        assert_eq!(parsed.query_id, q);
        assert_eq!(parsed.target, target);
        assert_eq!(parsed.from, me);
    }

    #[test]
    fn parse_find_node_resp_round_trip() {
        let me = Hid::random_humd();
        let peer = HumdAddr::new(Hid::random_humd()).with_hint("tcp:1.2.3.4:9000");
        let tone = find_node_resp_tone("rid-1", "qid-1", &me, &[peer.clone()]);
        let parsed = parse_find_node_resp(&tone).expect("parse");
        assert_eq!(parsed.query_id, "qid-1");
        assert_eq!(parsed.from, me);
        assert_eq!(parsed.closest.len(), 1);
        assert_eq!(parsed.closest[0].id, peer.id);
        assert_eq!(parsed.closest[0].hints, peer.hints);
    }
}
