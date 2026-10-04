use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use parking_lot::RwLock;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;

pub mod handshake;
pub use handshake::{
    EnsembleHello, HelloParse, Hid, HidParseError, HidPrefix, HumdAddr, HumdKey, PeerCapabilities,
    PeerConnection, Transport, hello_tone, hello_tone_unsigned, parse_hello, parse_hello_caps,
};

pub mod link;
pub use link::{
    InMemoryEndpoint, LinkCounters, LinkFaults, Noise, PARTITION_BUFFER_CAP, Script, Verdict,
};

pub mod tcp;
pub use tcp::{TcpEndpoint, TcpListener, TcpTransport};

pub mod tls;
pub use tls::{
    PinnedFingerprintVerifier, TLS_FP_HINT, TLS_HINT, TlsTcpEndpoint, TlsTcpListener,
    TlsTcpTransport, cert_fingerprint, client_config_pinned,
};

pub mod iroh;
pub use iroh::{IROH_ALPN, IROH_IP_HINT, IrohEndpoint, IrohTransport, dialable_addr};

pub mod delivery;
pub mod framing;
pub mod opening;
pub mod send;
pub use delivery::{DELIVERY_SEEN_CAP, DeliveryState};
pub use framing::MAX_FRAME_BYTES;
pub use opening::Gate;
pub use send::{SEND_TIMEOUT, SendError, SendStats, fanout, send_bounded};

pub mod gossip;
pub use gossip::{
    GOSSIP_CHI, GOSSIP_SEEN_CAP, GossipState, gossip_tone, gossip_tone_with_dusk, mint_msg_id,
};

pub mod liveness;
pub use liveness::{
    Lease, Liveness, LivenessSignal, PING_CHI, PONG_CHI, ping_tone, pong_tone, probe_seq,
};

pub mod kad;
pub use kad::{
    KAD_ALPHA, KAD_FIND_NODE_CHI, KAD_FIND_NODE_RESP_CHI, KAD_K, KAD_MAX_ROUNDS, KBucket,
    KadFindOutcome, KadState, RoutingTable, XorDistance, find_node_resp_tone, find_node_tone,
    mint_query_id, parse_find_node, parse_find_node_resp,
};

pub mod hives;
pub mod uri;
pub use hives::{ANNOUNCE_TOPIC, BindAddr, HiveAnnounce, HiveManifest, Propensity, ToolEntry};
pub use uri::{AliasResolver, HostRef, HumUri, UriParseError};

pub mod headroom;

#[cfg(feature = "onchain")]
pub mod onchain;

const HANDSHAKE_DOMAIN: &str = "hum-ensemble-handshake-v1";

const HANDSHAKE_SKEW_MS: i64 = 60_000;

pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub type Tone = serde_json::Value;

struct Peer {
    conn: Arc<dyn PeerConnection>,
    learned_caps: Option<PeerCapabilities>,
    lease: Lease,
}

pub struct Ensemble {
    me: Hid,
    peers: Arc<RwLock<HashMap<Hid, Peer>>>,
    inbox: Inbox,
    gossip: Arc<GossipState>,
    kad: Arc<KadState>,
    strict_auth: bool,
    expired_dusk: Arc<AtomicU64>,
    delivery: Arc<DeliveryState>,
    send_stats: Arc<SendStats>,
    unverified: Arc<AtomicU64>,
}

#[derive(Clone)]
pub struct Inbox {
    tx: broadcast::Sender<Tone>,
    subscribers: Arc<AtomicUsize>,
    dropped: Arc<AtomicU64>,
}

impl Inbox {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            tx,
            subscribers: Arc::new(AtomicUsize::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn publish(&self, tone: Tone) -> bool {
        if self.tx.send(tone).is_err() {
            let total = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                target: "ensemble",
                total,
                "inbox.publish.dropped: no local subscriber attached",
            );
            return false;
        }
        true
    }

    pub fn subscribe(&self) -> InboxSub {
        self.subscribers.fetch_add(1, Ordering::SeqCst);
        InboxSub {
            rx: self.tx.subscribe(),
            subscribers: self.subscribers.clone(),
        }
    }

    pub fn has_subscribers(&self) -> bool {
        self.subscribers.load(Ordering::SeqCst) > 0
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

pub struct InboxSub {
    rx: broadcast::Receiver<Tone>,
    subscribers: Arc<AtomicUsize>,
}

impl Deref for InboxSub {
    type Target = broadcast::Receiver<Tone>;
    fn deref(&self) -> &Self::Target {
        &self.rx
    }
}

impl std::ops::DerefMut for InboxSub {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rx
    }
}

impl Drop for InboxSub {
    fn drop(&mut self) {
        self.subscribers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for InboxSub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxSub").finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("no peer with id {0}")]
    UnknownPeer(Hid),
    #[error("tone has no `to` humd_id")]
    Untargeted,
    #[error("send failed: {0}")]
    SendFailed(anyhow::Error),
    #[error("peer stalled: no write completed within the send deadline")]
    PeerStalled,
}

impl Ensemble {
    pub fn new(me: Hid) -> Self {
        Self {
            me,
            peers: Arc::new(RwLock::new(HashMap::new())),
            inbox: Inbox::new(),
            gossip: GossipState::new(),
            kad: KadState::new(me),
            strict_auth: true,
            expired_dusk: Arc::new(AtomicU64::new(0)),
            delivery: DeliveryState::new(),
            send_stats: SendStats::new(),
            unverified: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_strict_auth(me: Hid, strict: bool) -> Self {
        let mut e = Self::new(me);
        e.strict_auth = strict;
        e
    }

    pub fn me(&self) -> Hid {
        self.me
    }

    pub fn strict_auth(&self) -> bool {
        self.strict_auth
    }

    pub fn install(
        &self,
        conn: Arc<dyn PeerConnection>,
        my_caps: PeerCapabilities,
        my_key: &HumdKey,
    ) {
        self.admit(conn, hello_tone(&self.me, my_key, &my_caps), true);
    }

    pub fn add_peer(&self, conn: Arc<dyn PeerConnection>) {
        self.install_unsigned(conn, PeerCapabilities::default());
    }

    pub fn add_peer_with_caps(&self, conn: Arc<dyn PeerConnection>, caps: PeerCapabilities) {
        self.install_unsigned(conn, caps);
    }

    pub fn install_unsigned(&self, conn: Arc<dyn PeerConnection>, my_caps: PeerCapabilities) {
        self.admit(conn, hello_tone_unsigned(&self.me, &my_caps), false);
    }

    fn admit(&self, conn: Arc<dyn PeerConnection>, hello: Tone, rekey: bool) {
        let id = conn.peer().id;
        conn.arm_opening();

        let stats = self.send_stats.clone();
        let hello_conn = conn.clone();
        tokio::spawn(async move {
            if send::send_opening_bounded(&hello_conn, hello, &stats, send::SEND_TIMEOUT)
                .await
                .is_err()
            {
                hello_conn.close();
            }
        });

        let rx = conn.take_receiver();
        self.peers.write().insert(
            id,
            Peer {
                conn: conn.clone(),
                learned_caps: None,
                lease: Lease::new(),
            },
        );
        self.kad.note_peer(conn.peer().clone());

        let Some(rx) = rx else { return };
        let drain = Drain {
            me: self.me,
            peers: self.peers.clone(),
            inbox: self.inbox.clone(),
            gossip: self.gossip.clone(),
            kad: self.kad.clone(),
            delivery: self.delivery.clone(),
            expired_dusk: self.expired_dusk.clone(),
            send_stats: self.send_stats.clone(),
            unverified: self.unverified.clone(),
            strict: self.strict_auth,
            rekey,
        };
        tokio::spawn(drain.run(rx, conn));
    }

    pub async fn probe_all(&self, seq: u64) {
        let peers: Vec<(Hid, Arc<dyn PeerConnection>)> = self
            .peers
            .read()
            .iter()
            .map(|(id, p)| (*id, p.conn.clone()))
            .collect();
        let sends = peers
            .into_iter()
            .map(|(id, conn)| (conn, ping_tone(&self.me, &id, seq)))
            .collect();
        for e in fanout(sends, &self.send_stats).await {
            tracing::debug!(target: "ensemble.liveness", error = %e, "probe.failed");
        }
    }

    pub async fn probe_one(&self, id: &Hid, seq: u64) {
        let conn = self.peers.read().get(id).map(|p| p.conn.clone());
        if let Some(conn) = conn {
            let _ = send_bounded(&conn, ping_tone(&self.me, id, seq), &self.send_stats).await;
        }
    }

    pub fn peer_liveness(&self, id: &Hid, ttl: std::time::Duration) -> Option<Liveness> {
        self.peers.read().get(id).map(|p| p.lease.state(ttl))
    }

    pub fn expired_peers(&self, ttl: std::time::Duration) -> Vec<Hid> {
        self.peers
            .read()
            .iter()
            .filter(|(_, p)| p.lease.expired(ttl))
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn expire_peer(&self, id: &Hid) {
        if let Some(p) = self.peers.write().get_mut(id) {
            p.lease.observe(LivenessSignal::TransportClosed);
        }
    }

    pub fn evict_expired(&self, ttl: std::time::Duration) -> Vec<Hid> {
        let expired = self.expired_peers(ttl);
        for id in &expired {
            self.remove_peer(id);
        }
        expired
    }

    pub fn remove_peer(&self, id: &Hid) {
        self.kad.forget(id);
        if let Some(p) = self.peers.write().remove(id) {
            p.conn.close();
        }
    }

    pub fn routing_snapshot(&self) -> Vec<HumdAddr> {
        self.kad.snapshot()
    }

    pub fn routing_restore(&self, addrs: Vec<HumdAddr>) -> usize {
        self.kad.restore(addrs)
    }

    pub fn peers(&self) -> Vec<Hid> {
        self.peers.read().keys().copied().collect()
    }

    pub fn peer_caps(&self, id: &Hid) -> Option<PeerCapabilities> {
        self.peers.read().get(id).map(|p| {
            p.learned_caps
                .clone()
                .unwrap_or_else(|| p.conn.capabilities().clone())
        })
    }

    pub fn handshake_done(&self, id: &Hid) -> bool {
        self.peers
            .read()
            .get(id)
            .is_some_and(|p| p.learned_caps.is_some())
    }

    pub fn subscribe(&self) -> InboxSub {
        self.inbox.subscribe()
    }

    pub fn has_subscribers(&self) -> bool {
        self.inbox.has_subscribers()
    }

    pub fn inbox_dropped(&self) -> u64 {
        self.inbox.dropped()
    }

    pub fn expired_dusk(&self) -> u64 {
        self.expired_dusk.load(Ordering::SeqCst)
    }

    pub fn delivery_seen(&self) -> usize {
        self.delivery.len()
    }

    pub fn send_timeouts(&self) -> u64 {
        self.send_stats.timed_out()
    }

    pub fn tones_before_handshake(&self) -> u64 {
        self.unverified.load(Ordering::SeqCst)
    }

    pub fn send_failures(&self) -> u64 {
        self.send_stats.failed()
    }

    pub async fn publish(&self, topic: &str, payload: serde_json::Value) {
        self.publish_with_dusk(topic, payload, None).await
    }

    pub async fn publish_with_dusk(
        &self,
        topic: &str,
        payload: serde_json::Value,
        dusk_ms: Option<i64>,
    ) {
        let msg_id = mint_msg_id(&self.me);
        let rid = format!("gossip-{msg_id}");
        self.gossip.note_seen(&msg_id);
        let tone = gossip_tone_with_dusk(topic, &rid, &self.me, payload, &msg_id, dusk_ms);
        let sends: Vec<(Arc<dyn PeerConnection>, Tone)> = self
            .peers
            .read()
            .values()
            .map(|p| (p.conn.clone(), tone.clone()))
            .collect();
        for e in fanout(sends, &self.send_stats).await {
            tracing::debug!(target: "ensemble.gossip", topic, error = %e, "publish.failed");
        }
    }

    pub fn subscribe_topic(&self, topic: &str) -> broadcast::Receiver<serde_json::Value> {
        self.gossip.subscribe(topic)
    }

    pub async fn hive_advertise(&self, manifest: hives::HiveManifest) {
        let env = hives::HiveAnnounce::Advertise {
            humd_id: self.me.to_hex(),
            manifest: Box::new(manifest),
        };
        match serde_json::to_value(&env) {
            Ok(payload) => self.publish(hives::ANNOUNCE_TOPIC, payload).await,
            Err(e) => tracing::warn!(target: "ensemble.bees", error = %e, "advertise serialize"),
        }
    }

    pub async fn hive_retract(&self, name: &str) {
        let env = hives::HiveAnnounce::Retract {
            humd_id: self.me.to_hex(),
            name: name.to_string(),
        };
        match serde_json::to_value(&env) {
            Ok(payload) => self.publish(hives::ANNOUNCE_TOPIC, payload).await,
            Err(e) => tracing::warn!(target: "ensemble.bees", error = %e, "retract serialize"),
        }
    }

    pub fn hive_announcements(&self) -> mpsc::Receiver<hives::HiveAnnounce> {
        let mut raw = self.subscribe_topic(hives::ANNOUNCE_TOPIC);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                match raw.recv().await {
                    Ok(v) => match serde_json::from_value::<hives::HiveAnnounce>(v.clone()) {
                        Ok(env) => {
                            if tx.send(env).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::debug!(
                            target: "ensemble.bees",
                            error = %e,
                            payload = %v,
                            "announce parse"
                        ),
                    },
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
        rx
    }

    pub fn hive_discover_all(&self) -> mpsc::Receiver<(Hid, hives::HiveManifest)> {
        let mut raw = self.subscribe_topic(hives::ANNOUNCE_TOPIC);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                match raw.recv().await {
                    Ok(v) => {
                        let parsed: Result<hives::HiveAnnounce, _> = serde_json::from_value(v);
                        if let Ok(hives::HiveAnnounce::Advertise { humd_id, manifest }) = parsed
                            && let Ok(id) = Hid::from_hex(&humd_id)
                            && tx.send((id, *manifest)).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
        rx
    }

    pub fn hive_discover(
        &self,
        name: impl Into<String>,
    ) -> mpsc::Receiver<(Hid, hives::HiveManifest)> {
        let needle = name.into();
        let mut raw = self.hive_discover_all();
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            while let Some((id, manifest)) = raw.recv().await {
                if manifest.name != needle {
                    continue;
                }
                if tx.send((id, manifest)).await.is_err() {
                    break;
                }
            }
        });
        rx
    }

    pub async fn kad_find(&self, target: Hid, timeout: Duration) -> Option<HumdAddr> {
        if let Some(addr) = self.kad.get(&target) {
            return Some(addr);
        }

        {
            let peer_addrs: Vec<HumdAddr> = {
                let peers = self.peers.read();
                peers.values().map(|p| p.conn.peer().clone()).collect()
            };
            for addr in peer_addrs {
                self.kad.note_peer(addr);
            }
        }

        let deadline = tokio::time::Instant::now() + timeout;
        let per_query_timeout = std::cmp::max(timeout / 4, Duration::from_millis(50));

        let seed = self.kad.closest_to(&target, KAD_K);
        if seed.is_empty() {
            return None;
        }
        let mut shortlist = kad::LookupShortlist::new(target, seed);

        for _round in 0..KAD_MAX_ROUNDS {
            if tokio::time::Instant::now() >= deadline {
                return self.kad.get(&target);
            }
            let batch = shortlist.next_unqueried(KAD_ALPHA);
            if batch.is_empty() {
                break;
            }
            let before = shortlist.closest_distance();

            let mut joinset: JoinSet<Vec<HumdAddr>> = JoinSet::new();
            for addr in &batch {
                shortlist.mark_queried(addr.id);
                let conn = {
                    let peers = self.peers.read();
                    peers.get(&addr.id).map(|p| p.conn.clone())
                };
                if let Some(conn) = conn {
                    let kad = self.kad.clone();
                    let stats = self.send_stats.clone();
                    let me = self.me;
                    let tgt = target;
                    joinset.spawn(async move {
                        kad::query_peer(&kad, &conn, &stats, &me, &tgt, per_query_timeout).await
                    });
                }
            }
            if joinset.is_empty() {
                continue;
            }
            let drain = async {
                while let Some(res) = joinset.join_next().await {
                    let advertised_list = match res {
                        Ok(list) => list,
                        Err(_) => continue,
                    };
                    for advertised in advertised_list {
                        self.kad.note_peer(advertised.clone());
                        if advertised.id == self.me {
                            continue;
                        }
                        shortlist.insert(advertised);
                    }
                }
            };
            if tokio::time::timeout_at(deadline, drain).await.is_err() {
                joinset.abort_all();
                return self.kad.get(&target);
            }

            if let Some(addr) = self.kad.get(&target) {
                return Some(addr);
            }
            let after = shortlist.closest_distance();
            if after >= before {
                break;
            }
        }

        if let Some(addr) = self.kad.get(&target) {
            return Some(addr);
        }
        let _ = shortlist.closest();
        None
    }

    pub fn kad_routing_table_len(&self) -> usize {
        self.kad.table.lock().len()
    }

    pub fn kad_closest(&self, target: &Hid, count: usize) -> Vec<HumdAddr> {
        self.kad.closest_to(target, count)
    }

    pub async fn route(&self, tone: Tone) -> Result<(), RouteError> {
        let to_hex = tone
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or(RouteError::Untargeted)?;
        let target = Hid::from_hex(to_hex).map_err(|_| RouteError::Untargeted)?;
        let conn = {
            let peers = self.peers.read();
            peers.get(&target).map(|p| p.conn.clone())
        };
        let conn = conn.ok_or(RouteError::UnknownPeer(target))?;
        match send_bounded(&conn, tone, &self.send_stats).await {
            Ok(()) => Ok(()),
            Err(SendError::TimedOut) => Err(RouteError::PeerStalled),
            Err(SendError::Failed(why)) => Err(RouteError::SendFailed(anyhow::anyhow!(why))),
        }
    }
}

struct Drain {
    me: Hid,
    peers: Arc<RwLock<HashMap<Hid, Peer>>>,
    inbox: Inbox,
    gossip: Arc<GossipState>,
    kad: Arc<KadState>,
    delivery: Arc<DeliveryState>,
    expired_dusk: Arc<AtomicU64>,
    send_stats: Arc<SendStats>,
    unverified: Arc<AtomicU64>,
    strict: bool,
    rekey: bool,
}

impl Drain {
    async fn run(self, mut rx: mpsc::Receiver<Tone>, conn: Arc<dyn PeerConnection>) {
        let mut id = conn.peer().id;
        let mut handshook = false;

        while let Some(tone) = rx.recv().await {
            let chi = tone.get("chi").and_then(|v| v.as_str());

            if !handshook {
                if chi == Some("hello") {
                    handshook = true;
                    if self.hello(&mut id, &conn, parse_hello(&tone)).is_err() {
                        self.peers.write().remove(&id);
                        conn.close();
                        return;
                    }
                } else {
                    self.unverified.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                continue;
            }
            if handle_liveness(&self.peers, &self.send_stats, &self.me, &id, &conn, &tone).await {
                continue;
            }
            if !delivery::dispatchable(&tone, &self.delivery, &self.expired_dusk) {
                continue;
            }
            if chi == Some(GOSSIP_CHI)
                && handle_gossip(&self.send_stats, &self.gossip, &self.peers, &id, &tone).await
            {
                continue;
            }
            if matches!(chi, Some(KAD_FIND_NODE_CHI) | Some(KAD_FIND_NODE_RESP_CHI))
                && handle_kad(
                    &self.send_stats,
                    &self.kad,
                    &self.peers,
                    &id,
                    &self.me,
                    &tone,
                )
                .await
            {
                continue;
            }
            self.inbox.publish(tone);
        }

        if let Some(p) = self.peers.write().get_mut(&id) {
            p.lease.observe(LivenessSignal::TransportClosed);
        }
    }

    fn hello(
        &self,
        id: &mut Hid,
        conn: &Arc<dyn PeerConnection>,
        parsed: HelloParse,
    ) -> Result<(), ()> {
        match parsed {
            HelloParse::Verified(claimed, caps) => {
                if claimed != *id && !self.rekey {
                    tracing::warn!(
                        target: "ensemble",
                        transport_id = %id.short(),
                        claimed_id = %claimed.short(),
                        "hello.rejected: claimed id differs from the id this link was admitted under"
                    );
                    return Err(());
                }
                if claimed != *id {
                    rekey_peer(&self.peers, &self.kad, conn, *id, claimed);
                    *id = claimed;
                }
                if let Some(p) = self.peers.write().get_mut(id) {
                    p.learned_caps = Some(caps);
                }
                Ok(())
            }
            HelloParse::Unsigned(claimed, caps) => {
                if self.strict {
                    tracing::warn!(
                        target: "ensemble",
                        transport_id = %id.short(),
                        claimed_id = %claimed.short(),
                        "hello.rejected: strict_auth requires a signed hello"
                    );
                    return Err(());
                }
                if claimed == *id
                    && let Some(p) = self.peers.write().get_mut(id)
                {
                    p.learned_caps = Some(caps);
                }
                Ok(())
            }
            HelloParse::Invalid => Err(()),
        }
    }
}

fn rekey_peer(
    peers: &Arc<RwLock<HashMap<Hid, Peer>>>,
    kad: &Arc<KadState>,
    conn: &Arc<dyn PeerConnection>,
    old_id: Hid,
    new_id: Hid,
) {
    let mut w = peers.write();
    if let Some(p) = w.remove(&old_id) {
        w.insert(new_id, p);
    }
    drop(w);
    let mut addr = conn.peer().clone();
    addr.id = new_id;
    kad.note_peer(addr);
}

async fn handle_kad(
    send_stats: &SendStats,
    kad: &Arc<KadState>,
    peers: &Arc<RwLock<HashMap<Hid, Peer>>>,
    arrived_from: &Hid,
    me: &Hid,
    tone: &Tone,
) -> bool {
    let chi_val = tone.get("chi").and_then(|v| v.as_str());
    if chi_val == Some(kad::KAD_FIND_NODE_CHI) {
        let parsed = match kad::parse_find_node(tone) {
            Some(p) => p,
            None => return false,
        };
        let closest = kad.closest_to(&parsed.target, KAD_K);
        let resp_rid = format!(
            "kad-resp-{}",
            &parsed.query_id[..8.min(parsed.query_id.len())]
        );
        let resp = kad::find_node_resp_tone(&resp_rid, &parsed.query_id, me, &closest);
        let conn = {
            let peers = peers.read();
            peers.get(arrived_from).map(|p| p.conn.clone())
        };
        if let Some(conn) = conn
            && let Err(e) = send_bounded(&conn, resp, send_stats).await
        {
            tracing::debug!(
                target: "ensemble.kad",
                peer = %arrived_from.short(),
                error = %e,
                "find-node response send failed"
            );
        }
        true
    } else if chi_val == Some(kad::KAD_FIND_NODE_RESP_CHI) {
        let parsed = match kad::parse_find_node_resp(tone) {
            Some(p) => p,
            None => return false,
        };
        for addr in &parsed.closest {
            kad.note_peer(addr.clone());
        }
        let _ = kad.deliver_response(&parsed.query_id, parsed.closest);
        true
    } else {
        false
    }
}

async fn handle_liveness(
    peers: &Arc<RwLock<HashMap<Hid, Peer>>>,
    send_stats: &Arc<SendStats>,
    me: &Hid,
    arrived_from: &Hid,
    conn: &Arc<dyn PeerConnection>,
    tone: &Tone,
) -> bool {
    if let Some(p) = peers.write().get_mut(arrived_from) {
        p.lease.observe(LivenessSignal::Traffic);
    }
    let chi = tone.get("chi").and_then(|v| v.as_str());
    if chi == Some(PONG_CHI) {
        return true;
    }
    if chi != Some(PING_CHI) {
        return false;
    }
    if let Some(seq) = probe_seq(tone) {
        let _ = send_bounded(conn, pong_tone(me, arrived_from, seq), send_stats).await;
    }
    true
}

fn announce_claims_sender(payload: &serde_json::Value, arrived_from: &Hid) -> bool {
    match serde_json::from_value::<hives::HiveAnnounce>(payload.clone()) {
        Ok(hives::HiveAnnounce::Advertise { humd_id, .. }) => humd_id == arrived_from.to_hex(),
        Ok(hives::HiveAnnounce::Retract { humd_id, .. }) => humd_id == arrived_from.to_hex(),
        Err(_) => true,
    }
}

async fn handle_gossip(
    send_stats: &Arc<SendStats>,
    gossip: &Arc<gossip::GossipState>,
    peers: &Arc<RwLock<HashMap<Hid, Peer>>>,
    arrived_from: &Hid,
    tone: &Tone,
) -> bool {
    let parsed = match gossip::parse_gossip(tone) {
        Some(p) => p,
        None => return false,
    };
    if !gossip.note_seen(parsed.msg_id) {
        return true;
    }
    if parsed.topic == hives::ANNOUNCE_TOPIC && !announce_claims_sender(&parsed.payload, arrived_from) {
        tracing::warn!(
            target: "ensemble.bees",
            arrived_from = %arrived_from,
            "gossip.announce.impersonation-rejected — payload claims a humd_id the sender does not own"
        );
        return true;
    }
    if let Some(tx) = gossip.sender(parsed.topic) {
        let _ = tx.send(parsed.payload.clone());
    }
    let sends: Vec<(Arc<dyn PeerConnection>, Tone)> = peers
        .read()
        .iter()
        .filter(|(id, _)| *id != arrived_from)
        .map(|(_, p)| (p.conn.clone(), tone.clone()))
        .collect();
    for e in fanout(sends, send_stats).await {
        tracing::debug!(target: "ensemble.gossip", error = %e, "refan.failed");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn link_pair() -> (Arc<InMemoryEndpoint>, Arc<InMemoryEndpoint>) {
        InMemoryEndpoint::pair_concrete(
            Hid::random_humd(),
            PeerCapabilities::default(),
            Hid::random_humd(),
            PeerCapabilities::default(),
        )
    }

    fn tone(tag: &str) -> Tone {
        json!({
            "chi": "prompt",
            "rid": tag,
            "from": Hid::random_humd().to_hex(),
        })
    }

    fn worker_announce(claimed: &str, model: &str) -> serde_json::Value {
        let mut manifest = hives::HiveManifest::new("worker-bee", "0.1.0", "0.7.0");
        manifest.bee = vec!["worker".to_string()];
        manifest.models = vec![model.to_string()];
        serde_json::to_value(hives::HiveAnnounce::Advertise {
            humd_id: claimed.to_string(),
            manifest: Box::new(manifest),
        })
        .expect("serialize announce")
    }

    #[test]
    fn an_announce_under_the_senders_own_hid_is_accepted() {
        let me = Hid::random_humd();
        let payload = worker_announce(&me.to_hex(), "claude-opus-4-7");
        assert!(announce_claims_sender(&payload, &me));
    }

    #[test]
    fn an_announce_claiming_another_hums_hid_is_rejected() {
        let me = Hid::random_humd();
        let victim = Hid::random_humd();
        let payload = worker_announce(&victim.to_hex(), "claude-opus-4-7");
        assert!(
            !announce_claims_sender(&payload, &me),
            "a peer must not advertise capabilities under a Hid it does not own"
        );
    }

    #[test]
    fn an_unknown_payload_shape_passes_the_provenance_gate() {
        let me = Hid::random_humd();
        assert!(announce_claims_sender(&json!({ "kind": "something-new" }), &me));
    }

    async fn drain(rx: &mut mpsc::Receiver<Tone>) -> Vec<Tone> {
        let mut out = Vec::new();
        while let Ok(t) = rx.try_recv() {
            out.push(t);
        }
        out
    }

    fn rids(tones: &[Tone]) -> Vec<String> {
        tones
            .iter()
            .map(|t| t["rid"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn perfect_link_delivers_everything_in_order() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        for i in 0..5 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t0", "t1", "t2", "t3", "t4"]);
        let c = a.counters();
        assert_eq!(
            c,
            LinkCounters {
                offered: 5,
                delivered: 5,
                ..Default::default()
            }
        );
    }

    #[tokio::test]
    async fn drop_next_drops_exactly_n() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_faults(LinkFaults::default().drop_next(3));
        for i in 0..6 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t3", "t4", "t5"]);
        let c = a.counters();
        assert_eq!(c.offered, 6);
        assert_eq!(c.dropped, 3);
        assert_eq!(c.delivered, 3);
    }

    #[tokio::test]
    async fn drop_every_keeps_a_strict_period() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        let mut faults = LinkFaults::default();
        faults.script.drop_every = 4;
        a.set_faults(faults);
        for i in 0..12 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t0", "t1", "t2", "t4", "t5", "t6", "t8", "t9", "t10"]);
    }

    #[tokio::test]
    async fn dup_next_duplicates_exactly_n() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_faults(LinkFaults::default().dup_next(2));
        for i in 0..4 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t0", "t0", "t1", "t1", "t2", "t3"]);
        let c = a.counters();
        assert_eq!(c.duplicated, 2);
        assert_eq!(c.delivered, 6);
    }

    #[tokio::test]
    async fn reorder_reverses_the_pair() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_faults(LinkFaults::default().reorder_next(1));
        for i in 0..2 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t1", "t0"]);
        assert_eq!(a.counters().reordered, 1);
    }

    #[tokio::test]
    async fn reorder_hold_releases_via_flush() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_faults(LinkFaults::default().reorder_next(1));
        a.send(tone("lonely")).await.unwrap();
        assert!(drain(&mut rx).await.is_empty());
        assert!(a.flush_reorder());
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["lonely"]);
    }

    #[tokio::test]
    async fn statistical_loss_is_reproducible_from_its_seed() {
        async fn run(seed: u64) -> usize {
            let (a, b) = link_pair();
            let mut rx = b.take_receiver().unwrap();
            a.set_faults(LinkFaults::default().drop_pct(30, seed));
            for i in 0..200 {
                a.send(tone(&format!("t{i}"))).await.unwrap();
            }
            drain(&mut rx).await.len()
        }
        assert_eq!(run(42).await, run(42).await, "same seed, same pattern");
        assert_ne!(
            run(1).await,
            run(2).await,
            "different seeds, different patterns"
        );
        let survived = run(42).await;
        assert!(
            (120..170).contains(&survived),
            "survived {survived} of 200 at 30% loss"
        );
    }

    #[tokio::test]
    async fn partition_buffers_without_delivering() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_partitioned(true);
        for i in 0..3 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        assert!(drain(&mut rx).await.is_empty());
        assert_eq!(a.buffered(), 3);
        let c = a.counters();
        assert_eq!(c.buffered, 3);
        assert_eq!(c.delivered, 0);
    }

    #[tokio::test]
    async fn heal_drains_the_buffer() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_partitioned(true);
        for i in 0..3 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        a.set_partitioned(false);
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t0", "t1", "t2"]);
        assert_eq!(a.counters().lost_on_heal, 0);
    }

    #[tokio::test]
    async fn heal_is_lossy_not_a_perfect_replay() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_partitioned(true);
        for i in 0..6 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        a.set_faults(LinkFaults::default().drop_next(2));
        a.set_partitioned(false);

        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t2", "t3", "t4", "t5"]);
        let c = a.counters();
        assert_eq!(c.lost_on_heal, 2);
        assert_eq!(c.buffered, 6);
        assert_eq!(c.delivered, 4);
    }

    #[tokio::test]
    async fn heal_applies_dup_too() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_partitioned(true);
        for i in 0..3 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        a.set_faults(LinkFaults::default().dup_next(1));
        a.set_partitioned(false);
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["t0", "t0", "t1", "t2"]);
        assert_eq!(a.counters().duplicated, 1);
    }

    #[tokio::test]
    async fn partition_buffer_evicts_oldest_when_full() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_partitioned(true);
        for i in 0..(PARTITION_BUFFER_CAP + 5) {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        assert_eq!(a.buffered(), PARTITION_BUFFER_CAP);
        assert_eq!(a.counters().evicted, 5);
        a.set_partitioned(false);
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got.first().unwrap(), "t5");
        assert_eq!(got.len(), PARTITION_BUFFER_CAP);
    }

    #[tokio::test]
    async fn counters_account_for_every_offered_tone() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        let mut faults = LinkFaults::default();
        faults.script.drop_every = 5;
        faults.script.dup_every = 7;
        a.set_faults(faults);
        a.set_partitioned(true);
        for i in 0..3 {
            a.send(tone(&format!("p{i}"))).await.unwrap();
        }
        a.set_partitioned(false);
        for i in 0..40 {
            a.send(tone(&format!("t{i}"))).await.unwrap();
        }
        a.flush_reorder();

        let c = a.counters();
        let arrived = drain(&mut rx).await.len() as u64;
        assert_eq!(
            c.delivered, arrived,
            "delivered must match what the peer saw"
        );
        assert_eq!(c.offered, 43);
        assert_eq!(c.buffered, 3);
        assert_eq!(
            c.offered,
            c.delivered - c.duplicated + c.dropped + c.lost_on_heal
        );
    }

    #[tokio::test]
    async fn faults_are_per_direction() {
        let (a, b) = link_pair();
        let mut a_rx = a.take_receiver().unwrap();
        let mut b_rx = b.take_receiver().unwrap();
        a.set_faults(LinkFaults::default().drop_next(10));
        b.send(tone("survivor")).await.unwrap();
        a.send(tone("doomed")).await.unwrap();
        assert_eq!(
            rids(&drain(&mut a_rx).await),
            ["survivor"],
            "b→a is unaffected"
        );
        assert!(drain(&mut b_rx).await.is_empty(), "a→b drops");
    }

    #[tokio::test]
    async fn clear_faults_restores_a_perfect_link() {
        let (a, b) = link_pair();
        let mut rx = b.take_receiver().unwrap();
        a.set_faults(LinkFaults::default().drop_next(10));
        a.send(tone("lost")).await.unwrap();
        a.set_faults(LinkFaults::default());
        a.send(tone("arrives")).await.unwrap();
        let got = rids(&drain(&mut rx).await);
        assert_eq!(got, ["arrives"]);
    }

    #[test]
    fn hid_hex_round_trips() {
        let id = Hid::random_humd();
        let hex = id.to_hex();
        let parsed: Hid = serde_json::from_str(&format!("\"{}\"", hex)).unwrap();
        assert_eq!(id, parsed);
        assert_eq!(hex.len(), "humd_".len() + 64);
        assert!(hex.starts_with("humd_"));
    }

    #[test]
    fn hid_legacy_bare_hex_parses_as_humd() {
        let bare = "a4f2b8c19d3e0c5a7f00112233445566778899aabbccddeeff00112233445566";
        let parsed: Hid = serde_json::from_str(&format!("\"{}\"", bare)).unwrap();
        assert_eq!(parsed.prefix, HidPrefix::Humd);
    }

    #[test]
    fn hid_wbee_prefix_parses() {
        let id = Hid::random(HidPrefix::Wbee);
        let hex = id.to_hex();
        assert!(hex.starts_with("wbee_"));
        let parsed: Hid = serde_json::from_str(&format!("\"{}\"", hex)).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn pubkey_hash_is_deterministic() {
        let pk = b"test-pubkey";
        let a = Hid::from_pubkey(HidPrefix::Humd, pk);
        let b = Hid::from_pubkey(HidPrefix::Humd, pk);
        assert_eq!(a, b);
        let c = Hid::from_pubkey(HidPrefix::Humd, b"other");
        assert_ne!(a, c);
    }

    #[tokio::test]
    async fn in_memory_pair_ping_pong() {
        let a_id = Hid::random_humd();
        let b_id = Hid::random_humd();
        let (a, b) = InMemoryEndpoint::pair(
            a_id,
            PeerCapabilities {
                proto_version: "0.2.0".into(),
                ..Default::default()
            },
            b_id,
            PeerCapabilities {
                proto_version: "0.2.0".into(),
                ..Default::default()
            },
        );
        let mut rx_b = b.take_receiver().unwrap();
        a.send(json!({"chi": "hello", "rid": "1", "from": a_id.to_hex()}))
            .await
            .unwrap();
        let received = rx_b.recv().await.unwrap();
        assert_eq!(received.get("chi").unwrap(), "hello");
    }

    #[tokio::test]
    async fn ensemble_routes_by_humd_id() {
        let me = Hid::random_humd();
        let peer_id = Hid::random_humd();
        let other_id = Hid::random_humd();

        let ensemble = Ensemble::new(me);
        let (mine, theirs) = InMemoryEndpoint::pair(
            me,
            PeerCapabilities::default(),
            peer_id,
            PeerCapabilities::default(),
        );
        ensemble.add_peer(mine);
        let mut rx = theirs.take_receiver().unwrap();

        let first = rx.recv().await.unwrap();
        assert_eq!(first.get("chi").unwrap(), "hello");

        let tone = json!({"chi": "ping", "rid": "r1", "to": peer_id.to_hex()});
        ensemble.route(tone).await.unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.get("chi").unwrap(), "ping");

        let bad = json!({"chi": "ping", "rid": "r2", "to": other_id.to_hex()});
        let err = ensemble.route(bad).await.unwrap_err();
        assert!(matches!(err, RouteError::UnknownPeer(_)));

        let no_to = json!({"chi": "ping", "rid": "r3"});
        let err = ensemble.route(no_to).await.unwrap_err();
        assert!(matches!(err, RouteError::Untargeted));
    }

    #[tokio::test]
    async fn install_exchanges_hellos_and_learns_caps() {
        let a_key = HumdKey::generate();
        let b_key = HumdKey::generate();
        let a_id = a_key.hid();
        let b_id = b_key.hid();
        let a_caps = PeerCapabilities {
            proto_version: "0.2.0".into(),
            nests: vec!["claude-cli".into()],
            hosts: vec!["alice".into()],
            can_relay: true,
            free_slots: None,
            headroom: headroom::CellHeadroom::default(),
        };
        let b_caps = PeerCapabilities {
            proto_version: "0.2.0".into(),
            nests: vec!["claude-repl".into()],
            hosts: vec!["bob".into()],
            can_relay: false,
            free_slots: None,
            headroom: headroom::CellHeadroom::default(),
        };
        let (a_side, b_side) = InMemoryEndpoint::pair(a_id, b_caps.clone(), b_id, a_caps.clone());

        let ensemble_a = Ensemble::new(a_id);
        let ensemble_b = Ensemble::new(b_id);
        ensemble_a.install(a_side, a_caps.clone(), &a_key);
        ensemble_b.install(b_side, b_caps.clone(), &b_key);

        for _ in 0..50 {
            if ensemble_a.peers().contains(&b_id)
                && ensemble_b.peers().contains(&a_id)
                && ensemble_a
                    .peers
                    .read()
                    .get(&b_id)
                    .and_then(|p| p.learned_caps.as_ref())
                    .is_some()
                && ensemble_b
                    .peers
                    .read()
                    .get(&a_id)
                    .and_then(|p| p.learned_caps.as_ref())
                    .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let learned_b = ensemble_a.peer_caps(&b_id).expect("b registered on a");
        assert_eq!(learned_b.proto_version, "0.2.0");
        assert_eq!(learned_b.nests, vec!["claude-repl".to_string()]);
        assert_eq!(learned_b.hosts, vec!["bob".to_string()]);
        assert!(!learned_b.can_relay);

        let learned_a = ensemble_b.peer_caps(&a_id).expect("a registered on b");
        assert_eq!(learned_a.nests, vec!["claude-cli".to_string()]);
        assert!(learned_a.can_relay);
    }

    #[tokio::test]
    async fn second_hello_on_same_peer_passes_through() {
        let me_key = HumdKey::generate();
        let peer_key = HumdKey::generate();
        let me = me_key.hid();
        let peer_id = peer_key.hid();
        let (mine, theirs) = InMemoryEndpoint::pair(
            me,
            PeerCapabilities::default(),
            peer_id,
            PeerCapabilities::default(),
        );

        let ensemble = Ensemble::new(me);
        let mut sub = ensemble.subscribe();
        ensemble.install(
            mine,
            PeerCapabilities {
                proto_version: "0.3.0".into(),
                ..Default::default()
            },
            &me_key,
        );

        theirs
            .send(hello_tone(
                &peer_id,
                &peer_key,
                &PeerCapabilities {
                    proto_version: "0.3.0".into(),
                    ..Default::default()
                },
            ))
            .await
            .unwrap();
        theirs
            .send(json!({
                "chi": "hello",
                "rid": "tunneled-hello",
                "from": "nestler-via-tunnel",
                "bee": "vercel-ai",
            }))
            .await
            .unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_millis(500), sub.recv())
            .await
            .expect("subscribe channel timed out")
            .expect("subscribe channel closed");
        assert_eq!(got.get("chi").unwrap(), "hello");
        assert_eq!(got.get("rid").unwrap(), "tunneled-hello");
        assert_eq!(got.get("bee").unwrap(), "vercel-ai");
    }

    #[tokio::test]
    async fn subscribe_forwards_remote_tones_but_swallows_hello() {
        let me_key = HumdKey::generate();
        let peer_key = HumdKey::generate();
        let me = me_key.hid();
        let peer_id = peer_key.hid();
        let (mine, theirs) = InMemoryEndpoint::pair(
            me,
            PeerCapabilities::default(),
            peer_id,
            PeerCapabilities::default(),
        );

        let ensemble = Ensemble::new(me);
        let mut sub = ensemble.subscribe();
        ensemble.install(
            mine,
            PeerCapabilities {
                proto_version: "0.2.0".into(),
                ..Default::default()
            },
            &me_key,
        );

        theirs
            .send(hello_tone(
                &peer_id,
                &peer_key,
                &PeerCapabilities {
                    proto_version: "0.2.0".into(),
                    ..Default::default()
                },
            ))
            .await
            .unwrap();
        theirs
            .send(json!({"chi": "ping", "rid": "r1", "from": peer_id.to_hex()}))
            .await
            .unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_millis(500), sub.recv())
            .await
            .expect("subscribe channel timed out")
            .expect("subscribe channel closed");
        assert_eq!(got.get("chi").unwrap(), "ping");
        assert_eq!(got.get("rid").unwrap(), "r1");
    }
}

#[cfg(test)]
mod invariants {
    use super::*;
    use serde_json::json;

    fn caps() -> PeerCapabilities {
        PeerCapabilities::default()
    }

    #[tokio::test]
    async fn the_opening_frame_precedes_application_traffic() {
        let me = Hid::random_humd();
        let peer = Hid::random_humd();
        let key = HumdKey::generate();
        let ensemble = Ensemble::new(me);
        let (mine, theirs) = InMemoryEndpoint::pair_concrete(me, caps(), peer, caps());
        let mut rx = theirs
            .take_receiver()
            .expect("far end must expose a receiver");

        ensemble.install(mine, caps(), &key);
        ensemble
            .route(json!({"chi": "prompt", "rid": "r1", "to": peer.to_hex()}))
            .await
            .expect("routing to the only peer cannot fail");

        let first = rx.recv().await.expect("receiver closed");
        assert_eq!(first["chi"], "hello", "the hello must be the first frame");
        let second = rx.recv().await.expect("receiver closed");
        assert_eq!(second["rid"], "r1", "application traffic follows the hello");
    }

    #[tokio::test]
    async fn a_tone_that_outruns_the_handshake_is_never_published() {
        let me = Hid::random_humd();
        let peer = Hid::random_humd();
        let key = HumdKey::generate();
        let ensemble = Ensemble::new(me);
        let (mine, theirs) = InMemoryEndpoint::pair_concrete(me, caps(), peer, caps());

        ensemble.install(mine, caps(), &key);

        let mut inbox = ensemble.subscribe();
        theirs
            .send(json!({"chi": "prompt", "rid": "early", "to": me.to_hex()}))
            .await
            .expect("in-memory send cannot fail");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(
            ensemble.tones_before_handshake(),
            1,
            "the drop must be counted"
        );
        assert!(
            inbox.try_recv().is_err(),
            "an unauthenticated tone must not publish"
        );
    }

    #[tokio::test]
    async fn an_unsigned_hello_is_refused_under_the_default_posture() {
        let me = Hid::random_humd();
        let peer = Hid::random_humd();
        let ensemble = Ensemble::new(me);
        assert!(
            ensemble.strict_auth(),
            "strict auth must be the default posture"
        );
        let (mine, theirs) = InMemoryEndpoint::pair_concrete(me, caps(), peer, caps());

        ensemble.install_unsigned(mine, caps());
        theirs
            .send(hello_tone_unsigned(&peer, &caps()))
            .await
            .expect("in-memory send cannot fail");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            !ensemble.peers().contains(&peer),
            "an unsigned peer must not be admitted under strict auth"
        );
    }

    #[tokio::test]
    async fn the_same_unsigned_hello_is_accepted_when_strictness_is_waived() {
        let me = Hid::random_humd();
        let peer = Hid::random_humd();
        let ensemble = Ensemble::with_strict_auth(me, false);
        let (mine, theirs) = InMemoryEndpoint::pair_concrete(me, caps(), peer, caps());

        ensemble.install_unsigned(mine, caps());
        theirs
            .send(hello_tone_unsigned(&peer, &caps()))
            .await
            .expect("in-memory send cannot fail");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            ensemble.handshake_done(&peer),
            "waived strictness must admit the peer"
        );
    }

    #[tokio::test]
    async fn a_removed_peer_leaves_the_routing_table() {
        let me = Hid::random_humd();
        let peer = Hid::random_humd();
        let ensemble = Ensemble::new(me);
        let (mine, _theirs) = InMemoryEndpoint::pair_concrete(me, caps(), peer, caps());

        ensemble.add_peer(mine);
        assert!(ensemble.kad_closest(&peer, 4).iter().any(|a| a.id == peer));

        ensemble.remove_peer(&peer);
        assert_eq!(
            ensemble.kad_routing_table_len(),
            0,
            "a dead peer must not route"
        );
    }
}
