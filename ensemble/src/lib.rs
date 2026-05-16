//! `ensemble` — the mesh of humds.
//!
//! One humd hosts many hums; the ensemble is the network of humds
//! cooperating. This crate owns the daemon-native shape that survives
//! across trust tiers (T1 own-devices → T4 open p2p):
//!
//! - [`HumdId`] — content-addressable identity, `hash(pubkey)`.
//! - [`HumdAddr`] — id plus optional contact hints (transport-shaped).
//! - [`PeerCapabilities`] — what a peer claims to do at handshake.
//! - [`PeerConnection`] — opaque link to one peer; send/recv tones.
//! - [`Transport`] — the seam: connect / accept implementations
//!   (in-memory for the sim, TCP+TLS / libp2p / Tor later as
//!   nestlings).
//! - [`Ensemble`] — local registry: peers by [`HumdId`], `route` for
//!   tones with a `to:` field, capability lookup.
//!
//! Cribbed in shape from libp2p's `Transport` + `PeerId` and Iroh's
//! `Endpoint` + `NodeId`. Wane sits in [`thrum_core::WaneTracker`];
//! event-sourcing semantics (Matrix-style lazy convergence) live in
//! the daemon's graft layer.
//!
//! Trust tiers don't appear in the types — they show up as which
//! `Transport` impl the daemon plugs in. T1 = `InMemoryTransport` for
//! tests / `StaticPeersTransport` for known boxes; T4 = a future
//! libp2p impl with DHT discovery. Daemon code is identical across
//! all of them.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::RwLock;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

/// Tones flow through the ensemble as loose JSON — same shape thrumd
/// uses on the wire. Strict typing lives in `thrum_core::Tone` for
/// callers that need it; here we stay loose so any new chi flows
/// through without a type bump.
pub type Tone = serde_json::Value;

// ── Identity ───────────────────────────────────────────────────────────────

/// Content-addressable identity of one humd in the ensemble.
///
/// Today: 32-byte SHA-256 of a public key (Ed25519 once T2+ wires real
/// crypto; random until then). Encoded as 64-char lowercase hex on the
/// wire. Stable per machine install; persists across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HumdId(#[serde(with = "hex::serde")] pub [u8; 32]);

impl HumdId {
    /// Mint a fresh id from a public key fingerprint.
    pub fn from_pubkey(pubkey: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(pubkey);
        let digest = h.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest[..32]);
        Self(out)
    }

    /// Mint a random id. Use only for tests / pre-crypto bring-up.
    pub fn random() -> Self {
        let mut out = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut out);
        Self(out)
    }

    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }
    pub fn to_hex(&self) -> String { hex::encode(self.0) }
    /// First 8 hex chars — for human-readable logs.
    pub fn short(&self) -> String { hex::encode(&self.0[..4]) }
}

impl fmt::Display for HumdId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// HumdId plus optional contact hints — a peer's "where" alongside its
/// "who." Sketched like a slim multiaddr: a list of transport-specific
/// strings the dialer can try. T1 might list `["tcp:host:port"]`; T4
/// might list multiple addresses for NAT punching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumdAddr {
    pub id: HumdId,
    #[serde(default)]
    pub hints: Vec<String>,
}

impl HumdAddr {
    pub fn new(id: HumdId) -> Self { Self { id, hints: Vec::new() } }
    pub fn with_hint(mut self, h: impl Into<String>) -> Self {
        self.hints.push(h.into());
        self
    }
}

// ── Capabilities ───────────────────────────────────────────────────────────

/// What a peer announces at the ensemble handshake. Extensible — new
/// fields land via additive minor versions. Mirrors libp2p protocol
/// negotiation, lighter and JSON-shaped.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeerCapabilities {
    /// thrum protocol version the peer speaks ("0.2.0", …).
    pub proto_version: String,
    /// Nest-kinds this peer can host (e.g. ["claude-cli","claude-repl"]).
    #[serde(default)]
    pub nests: Vec<String>,
    /// Hums this peer currently hosts (advertised on connect; updated
    /// over time via ensemble gossip).
    #[serde(default)]
    pub hosts: Vec<String>,
    /// Willing to relay tones for other humds (acts as a hop).
    #[serde(default)]
    pub can_relay: bool,
}

// ── Transport seam ─────────────────────────────────────────────────────────

/// One live link to one peer. Send + receive tones; that's it.
///
/// Implementations: in-memory channel pair for tests / sim; TCP+TLS
/// stream for T1-T3; libp2p stream for T4. The daemon never sees the
/// wire — it only sees tones in and out.
#[async_trait]
pub trait PeerConnection: Send + Sync {
    fn peer(&self) -> &HumdAddr;
    fn capabilities(&self) -> &PeerCapabilities;
    async fn send(&self, tone: Tone) -> Result<()>;
    /// Take ownership of the incoming-tone receiver. Callable once per
    /// connection — subsequent calls return None.
    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>>;
    /// Close the link best-effort. Idempotent.
    fn close(&self);
}

/// How peer connections come into being.
///
/// Outbound (`connect`) for daemons that initiate; inbound (`accept`)
/// for daemons that listen. A real transport implements both; the
/// in-memory sim transport implements only outbound (sim wires
/// connections by hand).
#[async_trait]
pub trait Transport: Send + Sync {
    /// Dial a peer. Identity verification happens here in real
    /// impls (cert chain, signed handshake, etc.).
    async fn connect(&self, addr: &HumdAddr) -> Result<Arc<dyn PeerConnection>>;
}

// ── In-memory transport (sim) ──────────────────────────────────────────────

/// Two `InMemoryEndpoint`s wired together with `mpsc` channels. Lets
/// the sim build a ring/mesh/star of fake-networked humds inside one
/// process with deterministic, low-latency delivery.
///
/// Latency / drop / partition behaviour is a follow-up — for v0 the
/// channels deliver instantly and never drop. The sim layer wraps
/// these with controllable middleware.
pub struct InMemoryEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    tx: mpsc::Sender<Tone>,
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
}

impl InMemoryEndpoint {
    /// Build a connected pair (`a`, `b`). `a.send(t)` flows to b's
    /// receiver; `b.send(t)` flows to a's receiver. Each endpoint
    /// claims the other's id + caps.
    pub fn pair(
        a_id: HumdId,
        a_caps: PeerCapabilities,
        b_id: HumdId,
        b_caps: PeerCapabilities,
    ) -> (Arc<dyn PeerConnection>, Arc<dyn PeerConnection>) {
        let (tx_ab, rx_ab) = mpsc::channel::<Tone>(256);
        let (tx_ba, rx_ba) = mpsc::channel::<Tone>(256);
        // a's view: peer is b. a sends via tx_ab; a receives via rx_ba.
        let a: Arc<dyn PeerConnection> = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(b_id),
            caps: b_caps.clone(),
            tx: tx_ab,
            rx: parking_lot::Mutex::new(Some(rx_ba)),
        });
        let b: Arc<dyn PeerConnection> = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(a_id),
            caps: a_caps,
            tx: tx_ba,
            rx: parking_lot::Mutex::new(Some(rx_ab)),
        });
        (a, b)
    }
}

#[async_trait]
impl PeerConnection for InMemoryEndpoint {
    fn peer(&self) -> &HumdAddr { &self.peer }
    fn capabilities(&self) -> &PeerCapabilities { &self.caps }

    async fn send(&self, tone: Tone) -> Result<()> {
        self.tx.send(tone).await.map_err(|e| anyhow::anyhow!("send: {e}"))
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn close(&self) {
        // Dropping the only sender drops the channel — receiver gets None.
        // We can't drop tx through &self without interior mutability; mark
        // closed by replacing rx with None so subsequent takes report empty.
        let _ = self.rx.lock().take();
    }
}

// ── Ensemble registry ──────────────────────────────────────────────────────

/// One humd's view of the ensemble: peers it knows about, their
/// connections, their capabilities. Owned by the daemon.
pub struct Ensemble {
    me: HumdId,
    peers: RwLock<HashMap<HumdId, Arc<dyn PeerConnection>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("no peer with id {0}")]
    UnknownPeer(HumdId),
    #[error("tone has no `to` humd_id")]
    Untargeted,
    #[error("send failed: {0}")]
    SendFailed(anyhow::Error),
}

impl Ensemble {
    pub fn new(me: HumdId) -> Self {
        Self { me, peers: RwLock::new(HashMap::new()) }
    }

    pub fn me(&self) -> HumdId { self.me }

    /// Register a peer. Replaces any prior connection for the same id.
    pub fn add_peer(&self, conn: Arc<dyn PeerConnection>) {
        let id = conn.peer().id;
        self.peers.write().insert(id, conn);
    }

    pub fn remove_peer(&self, id: &HumdId) {
        if let Some(c) = self.peers.write().remove(id) {
            c.close();
        }
    }

    pub fn peers(&self) -> Vec<HumdId> {
        self.peers.read().keys().copied().collect()
    }

    pub fn peer_caps(&self, id: &HumdId) -> Option<PeerCapabilities> {
        self.peers.read().get(id).map(|p| p.capabilities().clone())
    }

    /// Send a tone to the peer named in `tone.to` (must be present and
    /// a valid hex HumdId). Tone is `serde_json::Value` per thrum-core's
    /// loose shape.
    pub async fn route(&self, tone: Tone) -> Result<(), RouteError> {
        let to_hex = tone
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or(RouteError::Untargeted)?;
        let bytes = hex::decode(to_hex).map_err(|_| RouteError::Untargeted)?;
        if bytes.len() != 32 { return Err(RouteError::Untargeted); }
        let mut id = [0u8; 32];
        id.copy_from_slice(&bytes);
        let target = HumdId(id);
        let conn = {
            let peers = self.peers.read();
            peers.get(&target).cloned()
        };
        let conn = conn.ok_or(RouteError::UnknownPeer(target))?;
        conn.send(tone).await.map_err(RouteError::SendFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn humd_id_hex_round_trips() {
        let id = HumdId::random();
        let hex = id.to_hex();
        let parsed: HumdId = serde_json::from_str(&format!("\"{}\"", hex)).unwrap();
        assert_eq!(id, parsed);
        assert_eq!(hex.len(), 64);
    }

    #[test]
    fn pubkey_hash_is_deterministic() {
        let pk = b"test-pubkey";
        let a = HumdId::from_pubkey(pk);
        let b = HumdId::from_pubkey(pk);
        assert_eq!(a, b);
        let c = HumdId::from_pubkey(b"other");
        assert_ne!(a, c);
    }

    #[tokio::test]
    async fn in_memory_pair_ping_pong() {
        let a_id = HumdId::random();
        let b_id = HumdId::random();
        let (a, b) = InMemoryEndpoint::pair(
            a_id, PeerCapabilities { proto_version: "0.2.0".into(), ..Default::default() },
            b_id, PeerCapabilities { proto_version: "0.2.0".into(), ..Default::default() },
        );
        let mut rx_b = b.take_receiver().unwrap();
        a.send(json!({"chi": "hello", "rid": "1", "from": a_id.to_hex()})).await.unwrap();
        let received = rx_b.recv().await.unwrap();
        assert_eq!(received.get("chi").unwrap(), "hello");
    }

    #[tokio::test]
    async fn ensemble_routes_by_humd_id() {
        let me = HumdId::random();
        let peer_id = HumdId::random();
        let other_id = HumdId::random();

        let ensemble = Ensemble::new(me);
        let (mine, theirs) = InMemoryEndpoint::pair(
            me, PeerCapabilities::default(),
            peer_id, PeerCapabilities::default(),
        );
        ensemble.add_peer(mine);
        let mut rx = theirs.take_receiver().unwrap();

        // Route by `to: <peer_id hex>`.
        let tone = json!({"chi": "hello", "rid": "r1", "to": peer_id.to_hex()});
        ensemble.route(tone).await.unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.get("chi").unwrap(), "hello");

        // Unknown peer errors.
        let bad = json!({"chi": "hello", "rid": "r2", "to": other_id.to_hex()});
        let err = ensemble.route(bad).await.unwrap_err();
        assert!(matches!(err, RouteError::UnknownPeer(_)));

        // Missing `to` errors.
        let no_to = json!({"chi": "hello", "rid": "r3"});
        let err = ensemble.route(no_to).await.unwrap_err();
        assert!(matches!(err, RouteError::Untargeted));
    }
}
