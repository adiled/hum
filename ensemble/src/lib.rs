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

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use parking_lot::RwLock;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc};

pub mod tcp;
pub use tcp::{TcpEndpoint, TcpListener, TcpTransport};

/// Domain-separation tag binds a signature to the ensemble handshake.
/// Bump the version suffix if the canonical message shape changes.
const HANDSHAKE_DOMAIN: &str = "hum-ensemble-handshake-v1";

/// Tolerance window for `signed_at` skew, both directions.
const HANDSHAKE_SKEW_MS: i64 = 60_000;

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

/// Ed25519 signing key for a humd. The pubkey's SHA-256 is the
/// [`HumdId`] — identity is content-addressable, no separate registry.
///
/// v0 sim: each humd mints one at spawn and signs every hello with it.
/// Real key management (persistence, rotation, cert chains) lives at
/// T2+ in the daemon — this type is the shared crypto seam.
pub struct HumdKey(pub SigningKey);

impl HumdKey {
    /// Mint a fresh random keypair. Tests and v0 sim only — real humds
    /// will load a persisted key from the install root.
    pub fn generate() -> Self {
        Self(SigningKey::generate(&mut rand::thread_rng()))
    }

    /// Public key bytes — the input to [`HumdId::from_pubkey`] and the
    /// `pubkey` field carried in the hello.
    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    /// Derive the humd's content-addressable id from its pubkey.
    pub fn humd_id(&self) -> HumdId {
        HumdId::from_pubkey(&self.pubkey_bytes())
    }
}

impl fmt::Debug for HumdKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HumdKey")
            .field("pubkey", &hex::encode(self.pubkey_bytes()))
            .finish()
    }
}

/// Canonical message a humd signs to prove it owns the pubkey claiming
/// the named id at the named time. Domain-separated so a signature
/// over arbitrary bytes can never be replayed as a handshake.
fn handshake_message(humd_id: &HumdId, signed_at_ms: i64) -> Vec<u8> {
    format!("{}:{}:{}", HANDSHAKE_DOMAIN, humd_id.to_hex(), signed_at_ms).into_bytes()
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    /// Spare inference slots this peer claims to have free. `None` means
    /// unbounded / unspecified; `Some(0)` means full. Drives overflow
    /// peer selection — a humd at capacity routes new prompts to a peer
    /// whose `free_slots` is `None` or `Some(n) where n > 0`.
    #[serde(default)]
    pub free_slots: Option<usize>,
}

/// First tone over a fresh connection — each side names itself and what
/// it brings. The on-wire shape is loose JSON (`chi:"hello"`); this
/// struct is the typed mirror for callers who want to deserialize.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleHello {
    pub humd_id: HumdId,
    pub caps: PeerCapabilities,
}

/// Outcome of parsing a `chi:"hello"` tone.
///
/// `Verified` means the tone carried a pubkey + signature, the pubkey
/// hashed to the claimed humd_id, and the signature verified. `Unsigned`
/// means no pubkey was present — a T1-compat handshake that names an
/// id but doesn't prove ownership. `Invalid` means a pubkey was present
/// but verification failed (wrong hash, bad sig, stale timestamp, etc.)
/// — the sender tried to authenticate and failed, which is hostile.
#[derive(Debug, Clone)]
pub enum HelloParse {
    Verified(HumdId, PeerCapabilities),
    Unsigned(HumdId, PeerCapabilities),
    Invalid,
}

/// Build an unsigned `chi:"hello"` — the T1 back-compat shape. The peer
/// names itself and lists caps but provides no proof of ownership.
/// Strict-auth ensembles reject this; lax ones learn caps and proceed.
pub fn hello_tone_unsigned(me: &HumdId, caps: &PeerCapabilities) -> Tone {
    serde_json::json!({
        "chi": "hello",
        "rid": format!("hello-{}", me.short()),
        "from": me.to_hex(),
        "humd_id": me.to_hex(),
        "proto_version": caps.proto_version,
        "nests": caps.nests,
        "hosts": caps.hosts,
        "can_relay": caps.can_relay,
        "free_slots": caps.free_slots,
    })
}

/// Build the `chi:"hello"` tone a humd emits on connection install.
/// Carries identity + capabilities + an ed25519 signature over a
/// timestamped canonical message — the receiver verifies before
/// admitting the peer.
///
/// `humd_id` is derived from `key`'s pubkey; the parameter is kept so
/// callers can pin a specific id (sim test fixtures, primarily) and
/// have the verifier catch any inconsistency.
pub fn hello_tone(me: &HumdId, key: &HumdKey, caps: &PeerCapabilities) -> Tone {
    let signed_at = now_ms();
    let msg = handshake_message(me, signed_at);
    let sig: Signature = key.0.sign(&msg);
    serde_json::json!({
        "chi": "hello",
        "rid": format!("hello-{}", me.short()),
        "from": me.to_hex(),
        "humd_id": me.to_hex(),
        "pubkey": hex::encode(key.pubkey_bytes()),
        "proto_version": caps.proto_version,
        "nests": caps.nests,
        "hosts": caps.hosts,
        "can_relay": caps.can_relay,
        "free_slots": caps.free_slots,
        "signed_at": signed_at,
        "signature": hex::encode(sig.to_bytes()),
    })
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
/// Max tones held while partitioned. Realistic enough for sim narratives
/// (a few dozen petals during a partition window); large enough not to
/// fall behind in the tests we run. If the queue fills, oldest tones drop
/// — that matches the real-world "lossy link" semantic for an unbounded
/// outage.
pub const PARTITION_BUFFER_CAP: usize = 64;

pub struct InMemoryEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    tx: mpsc::Sender<Tone>,
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
    /// Sim-controlled link state. When `dropped == true`, `send()` accepts
    /// the tone and buffers it (bounded VecDeque) instead of pushing it
    /// to the peer's receiver. On `set_partitioned(false)`, the buffered
    /// tones flush to the peer in order before normal operation resumes.
    partition: parking_lot::Mutex<PartitionState>,
}

struct PartitionState {
    dropped: bool,
    buffer: VecDeque<Tone>,
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
        let (a, b) = Self::pair_concrete(a_id, a_caps, b_id, b_caps);
        (a as Arc<dyn PeerConnection>, b as Arc<dyn PeerConnection>)
    }

    /// Like `pair`, but returns concrete `Arc<InMemoryEndpoint>`s so
    /// callers (the sim) can drive `set_partitioned` on each side.
    pub fn pair_concrete(
        a_id: HumdId,
        a_caps: PeerCapabilities,
        b_id: HumdId,
        b_caps: PeerCapabilities,
    ) -> (Arc<InMemoryEndpoint>, Arc<InMemoryEndpoint>) {
        let (tx_ab, rx_ab) = mpsc::channel::<Tone>(256);
        let (tx_ba, rx_ba) = mpsc::channel::<Tone>(256);
        let a = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(b_id),
            caps: b_caps.clone(),
            tx: tx_ab,
            rx: parking_lot::Mutex::new(Some(rx_ba)),
            partition: parking_lot::Mutex::new(PartitionState {
                dropped: false,
                buffer: VecDeque::new(),
            }),
        });
        let b = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(a_id),
            caps: a_caps,
            tx: tx_ba,
            rx: parking_lot::Mutex::new(Some(rx_ab)),
            partition: parking_lot::Mutex::new(PartitionState {
                dropped: false,
                buffer: VecDeque::new(),
            }),
        });
        (a, b)
    }

    /// Toggle the partition on this endpoint. While `dropped == true`,
    /// `send()` queues tones in a bounded buffer (FIFO, oldest dropped
    /// when full) instead of delivering them. Flipping back to `false`
    /// flushes the buffer to the peer in original order.
    ///
    /// Sim-facing knob — production transports never call this. Note:
    /// partition is one-directional per endpoint. To fully isolate two
    /// peers, the caller flips both endpoints in the pair.
    pub fn set_partitioned(&self, dropped: bool) {
        // Drain the buffer under the lock if we're healing — but issue
        // the actual sends *after* releasing it so the await doesn't
        // happen while holding a sync mutex.
        let drained: Vec<Tone> = {
            let mut p = self.partition.lock();
            p.dropped = dropped;
            if !dropped {
                p.buffer.drain(..).collect()
            } else {
                Vec::new()
            }
        };
        if !drained.is_empty() {
            // try_send for the flush — if the receiver is closed or full
            // we silently drop, which is the realistic "buffer overrun
            // during long partition" semantic. Tests use a short window
            // so this branch should never fire in practice.
            for tone in drained {
                let _ = self.tx.try_send(tone);
            }
        }
    }
}

#[async_trait]
impl PeerConnection for InMemoryEndpoint {
    fn peer(&self) -> &HumdAddr { &self.peer }
    fn capabilities(&self) -> &PeerCapabilities { &self.caps }

    async fn send(&self, tone: Tone) -> Result<()> {
        // Partitioned: buffer with a bounded queue (oldest evicted when
        // capacity is hit). Tone is "accepted" from the caller's point
        // of view — the wire just hasn't delivered yet.
        {
            let mut p = self.partition.lock();
            if p.dropped {
                if p.buffer.len() >= PARTITION_BUFFER_CAP {
                    p.buffer.pop_front();
                }
                p.buffer.push_back(tone);
                return Ok(());
            }
        }
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

/// A peer entry: the live link plus what we've learned about them.
/// `learned_caps` starts `None` and fills in when their `chi:"hello"`
/// arrives — distinct from `conn.capabilities()` which the transport
/// hands us at dial time (and may be a stub for some transports).
struct Peer {
    conn: Arc<dyn PeerConnection>,
    learned_caps: Option<PeerCapabilities>,
}

/// One humd's view of the ensemble: peers it knows about, their
/// connections, their capabilities. Owned by the daemon.
///
/// Incoming tones from every installed peer fan into a single
/// `broadcast` channel — subscribe via [`Ensemble::subscribe`] to see
/// them. The `chi:"hello"` tones are absorbed here (they update
/// `learned_caps`) and not rebroadcast; everything else passes through.
pub struct Ensemble {
    me: HumdId,
    peers: Arc<RwLock<HashMap<HumdId, Peer>>>,
    inbox: broadcast::Sender<Tone>,
    /// When true, peers whose `chi:"hello"` is missing or fails
    /// verification are ejected (T3+ federation semantics). When false
    /// (default, T1), unsigned hellos are tolerated — caps are learned
    /// without proof of ownership and the connection stays installed.
    /// Invalid (signed-but-fails-verify) hellos are *always* ejected
    /// regardless of mode: a present pubkey that fails to verify is
    /// hostile, not legacy.
    strict_auth: bool,
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
        // 256 keeps recent tones available for slow subscribers without
        // unbounded memory; lagging consumers see Lagged and resync.
        let (inbox, _) = broadcast::channel(256);
        Self {
            me,
            peers: Arc::new(RwLock::new(HashMap::new())),
            inbox,
            strict_auth: false,
        }
    }

    /// Build an ensemble that rejects peers whose hellos aren't
    /// cryptographically verified. Federation (T3+) wants this on;
    /// own-devices (T1) leaves it off and tolerates unsigned T1 hellos.
    pub fn with_strict_auth(me: HumdId, strict: bool) -> Self {
        let mut e = Self::new(me);
        e.strict_auth = strict;
        e
    }

    pub fn me(&self) -> HumdId { self.me }

    pub fn strict_auth(&self) -> bool { self.strict_auth }

    /// Wire a peer connection into the ensemble: announce ourselves with
    /// a signed `chi:"hello"`, register the peer, and start draining
    /// its receiver into the shared inbox. The peer's first hello is
    /// verified before any of its tones reach subscribers — a bad
    /// signature, id/pubkey mismatch, or stale timestamp closes the
    /// connection and removes the peer entry.
    ///
    /// Replaces any prior entry for the same id (old drainer task ends
    /// when its receiver drops).
    pub fn install(
        &self,
        conn: Arc<dyn PeerConnection>,
        my_caps: PeerCapabilities,
        my_key: &HumdKey,
    ) {
        let id = conn.peer().id;
        let hello = hello_tone(&self.me, my_key, &my_caps);
        // Fire-and-forget the hello — if the channel is full or closed
        // the drainer / peer will surface it; install must not block.
        let hello_conn = conn.clone();
        tokio::spawn(async move {
            let _ = hello_conn.send(hello).await;
        });

        let rx = conn.take_receiver();
        self.peers.write().insert(
            id,
            Peer { conn: conn.clone(), learned_caps: None },
        );

        if let Some(mut rx) = rx {
            let peers = self.peers.clone();
            let inbox = self.inbox.clone();
            let conn_for_drain = conn.clone();
            let strict = self.strict_auth;
            tokio::spawn(async move {
                // Only the FIRST chi:"hello" off this connection is the
                // peer handshake — we absorb it to learn caps. Any
                // subsequent chi:"hello" is application-level (a
                // tunnelled nestler announcing itself, etc.) and must
                // pass through to subscribers.
                let mut handshake_seen = false;
                while let Some(tone) = rx.recv().await {
                    let is_hello = tone.get("chi").and_then(|v| v.as_str()) == Some("hello");
                    if is_hello && !handshake_seen {
                        handshake_seen = true;
                        match parse_hello(&tone) {
                            HelloParse::Verified(claimed_id, caps) if claimed_id == id => {
                                if let Some(p) = peers.write().get_mut(&id) {
                                    p.learned_caps = Some(caps);
                                }
                            }
                            HelloParse::Verified(claimed_id, _) => {
                                tracing::warn!(
                                    target: "ensemble",
                                    transport_id = %id.short(),
                                    claimed_id = %claimed_id.short(),
                                    "hello.rejected: claimed humd_id does not match transport-peer id"
                                );
                                peers.write().remove(&id);
                                conn_for_drain.close();
                                return;
                            }
                            HelloParse::Unsigned(claimed_id, caps) => {
                                if strict {
                                    tracing::warn!(
                                        target: "ensemble",
                                        transport_id = %id.short(),
                                        claimed_id = %claimed_id.short(),
                                        "hello.rejected: strict_auth requires signed hello"
                                    );
                                    peers.write().remove(&id);
                                    conn_for_drain.close();
                                    return;
                                }
                                // T1 compat: learn caps without proof. We
                                // still require the claimed id match the
                                // transport view — anything else is just
                                // a confused peer, not a hostile one,
                                // but the registry key has to match.
                                if claimed_id == id {
                                    if let Some(p) = peers.write().get_mut(&id) {
                                        p.learned_caps = Some(caps);
                                    }
                                }
                            }
                            HelloParse::Invalid => {
                                // Pubkey was present and failed to
                                // verify — always hostile. Eject in both
                                // strict and lax modes.
                                peers.write().remove(&id);
                                conn_for_drain.close();
                                return;
                            }
                        }
                        // First hello absorbed — handshake done.
                        continue;
                    }
                    // Everything else (including subsequent hellos) fans
                    // out. Receivers may be absent — broadcast drops.
                    let _ = inbox.send(tone);
                }
            });
        }
    }

    /// Back-compat shim: install with default caps and an *unsigned*
    /// hello. Existing tests / T1 callers that don't own a HumdKey can
    /// keep using this; new code should prefer `install` so the hello
    /// is signed and `with_strict_auth` ensembles will admit the peer.
    pub fn add_peer(&self, conn: Arc<dyn PeerConnection>) {
        self.install_unsigned(conn, PeerCapabilities::default());
    }

    /// Like [`Ensemble::add_peer`] but with caller-supplied caps — the
    /// outbound unsigned hello carries the real `nests` / `free_slots`
    /// instead of all-empty defaults, so the peer's `learned_caps` ends
    /// up populated for routing decisions (overflow, model coverage).
    pub fn add_peer_with_caps(&self, conn: Arc<dyn PeerConnection>, caps: PeerCapabilities) {
        self.install_unsigned(conn, caps);
    }

    /// Install a peer connection without signing the outbound hello.
    /// Mirror of [`Ensemble::install`] for callers that don't hold an
    /// identity yet (T1) — strict-auth ensembles on the other end will
    /// reject; lax-auth ones learn caps without crypto.
    pub fn install_unsigned(
        &self,
        conn: Arc<dyn PeerConnection>,
        my_caps: PeerCapabilities,
    ) {
        let id = conn.peer().id;
        let hello = hello_tone_unsigned(&self.me, &my_caps);
        let hello_conn = conn.clone();
        tokio::spawn(async move {
            let _ = hello_conn.send(hello).await;
        });

        let rx = conn.take_receiver();
        self.peers.write().insert(
            id,
            Peer { conn: conn.clone(), learned_caps: None },
        );

        if let Some(mut rx) = rx {
            let peers = self.peers.clone();
            let inbox = self.inbox.clone();
            let conn_for_drain = conn.clone();
            let strict = self.strict_auth;
            tokio::spawn(async move {
                let mut handshake_seen = false;
                while let Some(tone) = rx.recv().await {
                    let is_hello = tone.get("chi").and_then(|v| v.as_str()) == Some("hello");
                    if is_hello && !handshake_seen {
                        handshake_seen = true;
                        match parse_hello(&tone) {
                            HelloParse::Verified(claimed_id, caps) if claimed_id == id => {
                                if let Some(p) = peers.write().get_mut(&id) {
                                    p.learned_caps = Some(caps);
                                }
                            }
                            HelloParse::Verified(claimed_id, _) => {
                                tracing::warn!(
                                    target: "ensemble",
                                    transport_id = %id.short(),
                                    claimed_id = %claimed_id.short(),
                                    "hello.rejected: claimed humd_id does not match transport-peer id"
                                );
                                peers.write().remove(&id);
                                conn_for_drain.close();
                                return;
                            }
                            HelloParse::Unsigned(claimed_id, caps) => {
                                if strict {
                                    peers.write().remove(&id);
                                    conn_for_drain.close();
                                    return;
                                }
                                if claimed_id == id {
                                    if let Some(p) = peers.write().get_mut(&id) {
                                        p.learned_caps = Some(caps);
                                    }
                                }
                            }
                            HelloParse::Invalid => {
                                peers.write().remove(&id);
                                conn_for_drain.close();
                                return;
                            }
                        }
                        continue;
                    }
                    let _ = inbox.send(tone);
                }
            });
        }
    }

    pub fn remove_peer(&self, id: &HumdId) {
        if let Some(p) = self.peers.write().remove(id) {
            p.conn.close();
        }
    }

    pub fn peers(&self) -> Vec<HumdId> {
        self.peers.read().keys().copied().collect()
    }

    /// Capabilities the peer announced via `chi:"hello"`. Falls back to
    /// the transport-supplied caps if no hello has arrived yet.
    pub fn peer_caps(&self, id: &HumdId) -> Option<PeerCapabilities> {
        self.peers.read().get(id).map(|p| {
            p.learned_caps
                .clone()
                .unwrap_or_else(|| p.conn.capabilities().clone())
        })
    }

    /// Subscribe to incoming tones from every installed peer. Hellos
    /// are absorbed by the ensemble; subscribers only see real traffic.
    pub fn subscribe(&self) -> broadcast::Receiver<Tone> {
        self.inbox.subscribe()
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
            peers.get(&target).map(|p| p.conn.clone())
        };
        let conn = conn.ok_or(RouteError::UnknownPeer(target))?;
        conn.send(tone).await.map_err(RouteError::SendFailed)
    }
}

/// Pull caps out of a `chi:"hello"` tone AND verify its signature.
///
/// Returns `Some((claimed_id, caps))` only if all of:
///   - `pubkey`, `signature`, `signed_at`, `humd_id` parse cleanly
///   - `sha256(pubkey) == claimed humd_id`
///   - signature verifies over the canonical handshake message
///   - `signed_at` is within ±60s of local clock
///
/// Returns `None` on any failure — the drainer interprets that as
/// "close the connection, don't admit the peer." A `tracing::warn!`
/// names the specific failure so operators can debug.
pub fn parse_hello_caps(tone: &Tone) -> Option<(HumdId, PeerCapabilities)> {
    let proto_version = tone.get("proto_version")?.as_str()?.to_string();

    let claimed_humd_id_hex = tone.get("humd_id")?.as_str()?;
    let claimed_humd_id_bytes = hex::decode(claimed_humd_id_hex).ok()?;
    if claimed_humd_id_bytes.len() != 32 {
        tracing::warn!(target: "ensemble", "hello.rejected: humd_id wrong length");
        return None;
    }
    let mut claimed_id_arr = [0u8; 32];
    claimed_id_arr.copy_from_slice(&claimed_humd_id_bytes);
    let claimed_id = HumdId(claimed_id_arr);

    let pubkey_hex = tone.get("pubkey").and_then(|v| v.as_str())?;
    let pubkey_bytes = hex::decode(pubkey_hex).ok()?;
    if pubkey_bytes.len() != 32 {
        tracing::warn!(target: "ensemble", "hello.rejected: pubkey wrong length");
        return None;
    }
    let mut pubkey_arr = [0u8; 32];
    pubkey_arr.copy_from_slice(&pubkey_bytes);

    if HumdId::from_pubkey(&pubkey_arr) != claimed_id {
        tracing::warn!(
            target: "ensemble",
            humd_id = %claimed_id.short(),
            "hello.rejected: humd_id does not match sha256(pubkey)"
        );
        return None;
    }

    let signed_at = tone.get("signed_at").and_then(|v| v.as_i64())?;
    let drift = (now_ms() - signed_at).abs();
    if drift > HANDSHAKE_SKEW_MS {
        tracing::warn!(
            target: "ensemble",
            humd_id = %claimed_id.short(),
            drift_ms = drift,
            "hello.rejected: signed_at outside skew window"
        );
        return None;
    }

    let sig_hex = tone.get("signature").and_then(|v| v.as_str())?;
    let sig_bytes = hex::decode(sig_hex).ok()?;
    if sig_bytes.len() != 64 {
        tracing::warn!(target: "ensemble", "hello.rejected: signature wrong length");
        return None;
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    let verifying_key = VerifyingKey::from_bytes(&pubkey_arr).ok()?;
    let msg = handshake_message(&claimed_id, signed_at);
    if verifying_key.verify(&msg, &signature).is_err() {
        tracing::warn!(
            target: "ensemble",
            humd_id = %claimed_id.short(),
            "hello.rejected: signature verification failed"
        );
        return None;
    }

    let nests = tone
        .get("nests")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let hosts = tone
        .get("hosts")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let can_relay = tone.get("can_relay").and_then(|v| v.as_bool()).unwrap_or(false);
    let free_slots = tone
        .get("free_slots")
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_u64().map(|n| n as usize)
            }
        });
    Some((claimed_id, PeerCapabilities { proto_version, nests, hosts, can_relay, free_slots }))
}

/// Three-way parse of a `chi:"hello"` tone — signed/verified, unsigned
/// (T1 compat), or invalid. The drainer maps each arm to an admission
/// decision (admit, admit-if-lax, eject).
pub fn parse_hello(tone: &Tone) -> HelloParse {
    let has_pubkey = tone.get("pubkey").and_then(|v| v.as_str()).is_some();
    if has_pubkey {
        match parse_hello_caps(tone) {
            Some((id, caps)) => HelloParse::Verified(id, caps),
            None => HelloParse::Invalid,
        }
    } else {
        let Some(proto_version) = tone
            .get("proto_version")
            .and_then(|v| v.as_str())
            .map(String::from)
        else {
            return HelloParse::Invalid;
        };
        let Some(humd_hex) = tone.get("humd_id").and_then(|v| v.as_str()) else {
            return HelloParse::Invalid;
        };
        let Ok(bytes) = hex::decode(humd_hex) else {
            return HelloParse::Invalid;
        };
        if bytes.len() != 32 {
            return HelloParse::Invalid;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let claimed_id = HumdId(arr);
        let nests = tone
            .get("nests")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let hosts = tone
            .get("hosts")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let can_relay = tone.get("can_relay").and_then(|v| v.as_bool()).unwrap_or(false);
        let free_slots = tone
            .get("free_slots")
            .and_then(|v| if v.is_null() { None } else { v.as_u64().map(|n| n as usize) });
        HelloParse::Unsigned(
            claimed_id,
            PeerCapabilities { proto_version, nests, hosts, can_relay, free_slots },
        )
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

        // `add_peer` fires a hello first — drain it before asserting on
        // routed traffic so the test reads what it actually sent.
        let first = rx.recv().await.unwrap();
        assert_eq!(first.get("chi").unwrap(), "hello");

        // Route by `to: <peer_id hex>`.
        let tone = json!({"chi": "ping", "rid": "r1", "to": peer_id.to_hex()});
        ensemble.route(tone).await.unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.get("chi").unwrap(), "ping");

        // Unknown peer errors.
        let bad = json!({"chi": "ping", "rid": "r2", "to": other_id.to_hex()});
        let err = ensemble.route(bad).await.unwrap_err();
        assert!(matches!(err, RouteError::UnknownPeer(_)));

        // Missing `to` errors.
        let no_to = json!({"chi": "ping", "rid": "r3"});
        let err = ensemble.route(no_to).await.unwrap_err();
        assert!(matches!(err, RouteError::Untargeted));
    }

    /// Two ensembles wired by an InMemoryEndpoint pair should each
    /// learn the other's HumdId + caps via the install handshake.
    #[tokio::test]
    async fn install_exchanges_hellos_and_learns_caps() {
        let a_key = HumdKey::generate();
        let b_key = HumdKey::generate();
        let a_id = a_key.humd_id();
        let b_id = b_key.humd_id();
        let a_caps = PeerCapabilities {
            proto_version: "0.2.0".into(),
            nests: vec!["claude-cli".into()],
            hosts: vec!["alice".into()],
            can_relay: true,
            free_slots: None,
        };
        let b_caps = PeerCapabilities {
            proto_version: "0.2.0".into(),
            nests: vec!["claude-repl".into()],
            hosts: vec!["bob".into()],
            can_relay: false,
            free_slots: None,
        };
        let (a_side, b_side) = InMemoryEndpoint::pair(
            a_id, b_caps.clone(),  // a's transport-view of b
            b_id, a_caps.clone(),  // b's transport-view of a
        );

        let ensemble_a = Ensemble::new(a_id);
        let ensemble_b = Ensemble::new(b_id);
        ensemble_a.install(a_side, a_caps.clone(), &a_key);
        ensemble_b.install(b_side, b_caps.clone(), &b_key);

        // Each side's drainer eats the other's hello and writes
        // learned_caps. Poll briefly — the spawned tasks need a tick.
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

    /// Second + subsequent hellos on the same peer connection are
    /// application-level (e.g. a tunneled nestler announcing itself
    /// via the ensemble) and must surface to subscribers. Only the
    /// first hello — the handshake — is absorbed.
    #[tokio::test]
    async fn second_hello_on_same_peer_passes_through() {
        let me_key = HumdKey::generate();
        let peer_key = HumdKey::generate();
        let me = me_key.humd_id();
        let peer_id = peer_key.humd_id();
        let (mine, theirs) = InMemoryEndpoint::pair(
            me, PeerCapabilities::default(),
            peer_id, PeerCapabilities::default(),
        );

        let ensemble = Ensemble::new(me);
        let mut sub = ensemble.subscribe();
        ensemble.install(mine, PeerCapabilities { proto_version: "0.3.0".into(), ..Default::default() }, &me_key);

        // First hello — handshake, absorbed.
        theirs
            .send(hello_tone(&peer_id, &peer_key, &PeerCapabilities { proto_version: "0.3.0".into(), ..Default::default() }))
            .await
            .unwrap();
        // Second hello — application-level, should fan out.
        theirs
            .send(json!({
                "chi": "hello",
                "rid": "tunneled-hello",
                "from": "nestler-via-tunnel",
                "nestling": "vercel-ai",
            }))
            .await
            .unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_millis(500), sub.recv())
            .await
            .expect("subscribe channel timed out")
            .expect("subscribe channel closed");
        assert_eq!(got.get("chi").unwrap(), "hello");
        assert_eq!(got.get("rid").unwrap(), "tunneled-hello");
        assert_eq!(got.get("nestling").unwrap(), "vercel-ai");
    }

    /// Non-hello tones from a peer must reach `subscribe()` listeners;
    /// hellos are absorbed and never surface.
    #[tokio::test]
    async fn subscribe_forwards_remote_tones_but_swallows_hello() {
        let me_key = HumdKey::generate();
        let peer_key = HumdKey::generate();
        let me = me_key.humd_id();
        let peer_id = peer_key.humd_id();
        let (mine, theirs) = InMemoryEndpoint::pair(
            me, PeerCapabilities::default(),
            peer_id, PeerCapabilities::default(),
        );

        let ensemble = Ensemble::new(me);
        let mut sub = ensemble.subscribe();
        ensemble.install(mine, PeerCapabilities { proto_version: "0.2.0".into(), ..Default::default() }, &me_key);

        // The peer side sends a hello (which the ensemble should
        // absorb) followed by a real tone (which should fan out).
        theirs
            .send(hello_tone(&peer_id, &peer_key, &PeerCapabilities { proto_version: "0.2.0".into(), ..Default::default() }))
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
