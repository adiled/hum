//! `ensemble` — the mesh of humds.
//!
//! One humd hosts many hums; the ensemble is the network of humds
//! cooperating. This crate owns the daemon-native shape that survives
//! across trust tiers (T1 own-devices → T4 open p2p):
//!
//! - [`Hid`] — content-addressable identity, `hash(pubkey)`.
//! - [`HumdAddr`] — id plus optional contact hints (transport-shaped).
//! - [`PeerCapabilities`] — what a peer claims to do at handshake.
//! - [`PeerConnection`] — opaque link to one peer; send/recv tones.
//! - [`Transport`] — the seam: connect / accept implementations
//!   (in-memory for the sim, TCP+TLS / libp2p / Tor later as
//!   bees).
//! - [`Ensemble`] — local registry: peers by [`Hid`], `route` for
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
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use parking_lot::{Mutex, RwLock};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;

pub mod tcp;
pub use tcp::{TcpEndpoint, TcpListener, TcpTransport};

pub mod tls;
pub use tls::{
    cert_fingerprint, client_config_pinned, PinnedFingerprintVerifier, TlsTcpEndpoint,
    TlsTcpListener, TlsTcpTransport, TLS_FP_HINT, TLS_HINT,
};

pub mod iroh;
pub use iroh::{dialable_addr, IrohEndpoint, IrohTransport, IROH_ALPN, IROH_IP_HINT};

pub mod delivery;
pub mod send;
pub use send::{send_bounded, SendError, SendStats, SEND_TIMEOUT};
pub use delivery::{DeliveryState, DELIVERY_SEEN_CAP};

pub mod gossip;
pub use gossip::{
    gossip_tone, gossip_tone_with_dusk, mint_msg_id, GossipState, GOSSIP_CHI,
    GOSSIP_SEEN_CAP,
};

pub mod liveness;
pub use liveness::{
    ping_tone, pong_tone, probe_seq, Lease, Liveness, LivenessSignal, PING_CHI, PONG_CHI,
};

pub mod kad;
pub use kad::{
    find_node_resp_tone, find_node_tone, mint_query_id, parse_find_node, parse_find_node_resp,
    KBucket, KadFindOutcome, KadState, RoutingTable, XorDistance, KAD_ALPHA,
    KAD_FIND_NODE_CHI, KAD_FIND_NODE_RESP_CHI, KAD_K, KAD_MAX_ROUNDS,
};

pub mod hives;
pub mod uri;
pub use hives::{BindAddr, HiveAnnounce, HiveManifest, Propensity, ToolEntry, ANNOUNCE_TOPIC};
pub use uri::{AliasResolver, HostRef, HumUri, UriParseError};

// Headroom advertise — `PeerCapabilities` gains a runtime snapshot of
// free slots / pressure / p95 latency so peer humds can route away from
// saturated nodes. Filled in by Tier 2 agent.
pub mod headroom;

#[cfg(feature = "onchain")]
pub mod onchain;

/// Domain-separation tag binds a signature to the ensemble handshake.
/// Bump the version suffix if the canonical message shape changes.
const HANDSHAKE_DOMAIN: &str = "hum-ensemble-handshake-v1";

/// Tolerance window for `signed_at` skew, both directions.
const HANDSHAKE_SKEW_MS: i64 = 60_000;

/// Tones flow through the ensemble as loose JSON — same shape humd's thrum module
/// uses on the wire. Strict typing lives in `thrum_core::Tone` for
/// callers that need it; here we stay loose so any new chi flows
/// through without a type bump.
pub type Tone = serde_json::Value;

// ── Identity ───────────────────────────────────────────────────────────────
/// Content-addressable identity, moved to `ids`. Re-exported here for
/// back-compat so existing `ensemble::Hid` / `ensemble::HidPrefix` call
/// sites keep compiling.
pub use hum_identity::{Hid, HidPrefix, HidParseError};
/// Ed25519 signing key for a humd. The pubkey's SHA-256 is the
/// [`Hid`] — identity is content-addressable, no separate registry.
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

    /// Public key bytes — the input to [`Hid::from_pubkey`] and the
    /// `pubkey` field carried in the hello.
    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    /// Derive the humd's content-addressable hid from its pubkey.
    pub fn hid(&self) -> Hid {
        Hid::from_pubkey(HidPrefix::Humd, &self.pubkey_bytes())
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
fn handshake_message(humd_id: &Hid, signed_at_ms: i64) -> Vec<u8> {
    format!("{}:{}:{}", HANDSHAKE_DOMAIN, humd_id.to_hex(), signed_at_ms).into_bytes()
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Hid plus optional contact hints — a peer's "where" alongside its
/// "who." Sketched like a slim multiaddr: a list of transport-specific
/// strings the dialer can try. T1 might list `["tcp:host:port"]`; T4
/// might list multiple addresses for NAT punching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumdAddr {
    pub id: Hid,
    #[serde(default)]
    pub hints: Vec<String>,
}

impl HumdAddr {
    pub fn new(id: Hid) -> Self { Self { id, hints: Vec::new() } }
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
    /// Live capacity snapshot — see [`headroom::CellHeadroom`]. Carries
    /// the pressure tier (Cool/Warm/Hot/Refuse) + p95 latency so peers
    /// can route away from saturated nodes without waiting for hard
    /// failure. Empty default = no nest / not yet measured; treat as
    /// available unless explicitly Refuse.
    #[serde(default)]
    pub headroom: headroom::CellHeadroom,
}

/// First tone over a fresh connection — each side names itself and what
/// it brings. The on-wire shape is loose JSON (`chi:"hello"`); this
/// struct is the typed mirror for callers who want to deserialize.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleHello {
    pub humd_id: Hid,
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
    Verified(Hid, PeerCapabilities),
    Unsigned(Hid, PeerCapabilities),
    Invalid,
}

/// Build an unsigned `chi:"hello"` — the T1 back-compat shape. The peer
/// names itself and lists caps but provides no proof of ownership.
/// Strict-auth ensembles reject this; lax ones learn caps and proceed.
pub fn hello_tone_unsigned(me: &Hid, caps: &PeerCapabilities) -> Tone {
    serde_json::json!({
        "chi": "hello",
        "rid": hum_identity::HumId::mint().to_string(),
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
pub fn hello_tone(me: &Hid, key: &HumdKey, caps: &PeerCapabilities) -> Tone {
    let signed_at = now_ms();
    let msg = handshake_message(me, signed_at);
    let sig: Signature = key.0.sign(&msg);
    serde_json::json!({
        "chi": "hello",
        "rid": hum_identity::HumId::mint().to_string(),
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

// ── Link fault model (sim) ─────────────────────────────────────────────────

/// Deterministic faults, counted in offered tones. Takes precedence over
/// [`Noise`] so a scripted scenario lands exactly where it says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    /// Decremented per offered tone.
    pub drop_next: usize,
    pub dup_next: usize,
    /// Decremented per offered pair; reverses its delivery.
    pub reorder_next: usize,
    pub drop_every: usize,
    pub dup_every: usize,
}

impl Script {
    /// `counted` wins over `every`: a scripted `drop_next` of 3 drops
    /// exactly the next 3, whatever the periodic rule would have said.
    fn take(counted: &mut usize, every: usize, offered: u64) -> bool {
        match *counted {
            0 => every > 0 && offered % every as u64 == 0,
            n => {
                *counted = n - 1;
                true
            }
        }
    }

    fn verdict(&mut self, offered: u64) -> Option<Verdict> {
        let drop = Self::take(&mut self.drop_next, self.drop_every, offered);
        let duplicate = Self::take(&mut self.dup_next, self.dup_every, offered);
        (drop || duplicate).then_some(Verdict { drop, duplicate })
    }
}

/// Probabilistic faults, drawn from the link's seeded PRNG.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Noise {
    pub drop_pct: u8,
    pub dup_pct: u8,
}

impl Noise {
    fn verdict(&self, rng: &mut impl Rng) -> Verdict {
        Verdict {
            drop: self.drop_pct > 0 && rng.gen_range(0..100u8) < self.drop_pct,
            duplicate: self.dup_pct > 0 && rng.gen_range(0..100u8) < self.dup_pct,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub drop: bool,
    pub duplicate: bool,
}

#[derive(Debug, Clone)]
pub struct LinkFaults {
    pub script: Script,
    pub noise: Noise,
    seed: u64,
    offered: u64,
}

impl Default for LinkFaults {
    fn default() -> Self {
        Self { script: Script::default(), noise: Noise::default(), seed: 0x5EED_C0DE, offered: 0 }
    }
}

impl LinkFaults {
    pub fn script(mut self, script: Script) -> Self {
        self.script = script;
        self
    }

    pub fn noise(mut self, noise: Noise) -> Self {
        self.noise = noise;
        self
    }

    pub fn drop_next(mut self, n: usize) -> Self {
        self.script.drop_next = n;
        self
    }

    pub fn dup_next(mut self, n: usize) -> Self {
        self.script.dup_next = n;
        self
    }

    pub fn reorder_next(mut self, n: usize) -> Self {
        self.script.reorder_next = n;
        self
    }

    pub fn drop_pct(mut self, pct: u8, seed: u64) -> Self {
        self.noise.drop_pct = pct.min(100);
        self.seed = seed;
        self
    }

    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// `offered` advances for every tone the link accepts, partitioned
    /// tones included, so a scenario can be written against a global
    /// position rather than a per-state one.
    fn verdict(&mut self, rng: &mut impl Rng) -> Verdict {
        self.offered += 1;
        self.script.verdict(self.offered).unwrap_or_else(|| self.noise.verdict(rng))
    }
}

/// Ground truth for a link: what it was handed versus what arrived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkCounters {
    pub offered: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub reordered: u64,
    pub buffered: u64,
    pub lost_on_heal: u64,
    pub evicted: u64,
    /// Sends handed to a stalled link, which never completed.
    pub stalled_sends: u64,
}

impl LinkCounters {
    /// Counters accrued after `base` was taken.
    pub fn since(&self, base: &Self) -> Self {
        Self {
            offered: self.offered - base.offered,
            delivered: self.delivered - base.delivered,
            dropped: self.dropped - base.dropped,
            duplicated: self.duplicated - base.duplicated,
            reordered: self.reordered - base.reordered,
            buffered: self.buffered - base.buffered,
            lost_on_heal: self.lost_on_heal - base.lost_on_heal,
            evicted: self.evicted - base.evicted,
            stalled_sends: self.stalled_sends - base.stalled_sends,
        }
    }
}

// ── In-memory transport (sim) ──────────────────────────────────────────────

/// Two `InMemoryEndpoint`s wired together with `mpsc` channels. Lets
/// the sim build a ring/mesh/star of fake-networked humds inside one
/// process with deterministic, low-latency delivery.
///
/// Delivery is fault-injectable via [`LinkFaults`]: loss, duplication,
/// reordering, and partition all behave the way a real link does, and
/// [`LinkCounters`] records what actually happened. Nothing here runs in
/// production — the daemon only ever sees the [`PeerConnection`] trait.
/// Max tones held while partitioned. Realistic enough for sim narratives
/// (a few dozen petals during a partition window); large enough not to
/// fall behind in the tests we run. If the queue fills, oldest tones drop
/// — that matches the real-world "lossy link" semantic for an unbounded
/// outage.
pub const PARTITION_BUFFER_CAP: usize = 64;

pub struct InMemoryEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    /// Outbound sender. An Option so `kill` can drop it: dropping the
    /// last sender is what closes the *peer's* receiver and ends its
    /// drainer. `close` only drops our own receiver, which stops us
    /// reading without telling the peer anything.
    tx: Mutex<Option<mpsc::Sender<Tone>>>,
    rx: Mutex<Option<mpsc::Receiver<Tone>>>,
    /// Sim-controlled link state. When `partitioned == true`, `send()`
    /// accepts the tone and buffers it (bounded VecDeque) instead of
    /// pushing it to the peer's receiver. On `set_partitioned(false)`
    /// the buffer is flushed — but *through the fault model*, not
    /// replayed intact. A healing link is still a lossy link; that
    /// asymmetry is the whole point, and it is what a real mesh does.
    partition: Mutex<PartitionState>,
    /// Fault profile for this link. Default = perfect, so existing tests
    /// keep their current semantics until they opt in.
    faults: Mutex<LinkFaults>,
    /// Seeded PRNG for the statistical knobs. Held separately from
    /// `faults` so `LinkFaults` stays `Clone` and comparable.
    rng: Mutex<StdRng>,
    /// Ground truth. Read by tests to assert the ensemble's own loss
    /// accounting against what the link actually did.
    counters: Mutex<LinkCounters>,
    /// Held tone during a reorder pair, awaiting its partner so the pair
    /// can be delivered in reverse.
    reorder_hold: Mutex<Option<Tone>>,
    /// Set by [`InMemoryEndpoint::kill`]. A killed link is gone, not
    /// slow: sends fail and the peer drainer sees its receiver close,
    /// which is what marks the lease `TransportClosed`. Distinct from
    /// `partitioned`, which keeps the link nominally up.
    killed: AtomicBool,
    /// Set by [`InMemoryEndpoint::stall`]. A stalled link accepts the
    /// connection and stops draining: the write never completes, which
    /// is what a full socket buffer with a non-reading peer looks like
    /// from the writer's side. Distinct from `killed` (gone) and from
    /// `partitioned` (nominally up and buffering) — a stalled peer
    /// looks perfectly healthy to a lease.
    stalled: AtomicBool,
}

struct PartitionState {
    partitioned: bool,
    buffer: VecDeque<Tone>,
}

impl InMemoryEndpoint {
    /// Build a connected pair (`a`, `b`). `a.send(t)` flows to b's
    /// receiver; `b.send(t)` flows to a's receiver. Each endpoint
    /// claims the other's id + caps.
    pub fn pair(
        a_id: Hid,
        a_caps: PeerCapabilities,
        b_id: Hid,
        b_caps: PeerCapabilities,
    ) -> (Arc<dyn PeerConnection>, Arc<dyn PeerConnection>) {
        let (a, b) = Self::pair_concrete(a_id, a_caps, b_id, b_caps);
        (a as Arc<dyn PeerConnection>, b as Arc<dyn PeerConnection>)
    }

    /// Like `pair`, but returns concrete `Arc<InMemoryEndpoint>`s so
    /// callers (the sim) can drive `set_partitioned` on each side.
    pub fn pair_concrete(
        a_id: Hid,
        a_caps: PeerCapabilities,
        b_id: Hid,
        b_caps: PeerCapabilities,
    ) -> (Arc<InMemoryEndpoint>, Arc<InMemoryEndpoint>) {
        let (tx_ab, rx_ab) = mpsc::channel::<Tone>(256);
        let (tx_ba, rx_ba) = mpsc::channel::<Tone>(256);
        // Each direction of the link gets its own PRNG stream. Sharing one
        // seed would make both halves fail identically at the same
        // points, which is a correlated outage — the opposite of what a
        // two-way link actually does.
        let seed_a = LinkFaults::default().seed;
        let seed_b = seed_a ^ 0x9E37_79B9_7F4A_7C15;
        let a = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(b_id),
            caps: b_caps.clone(),
            tx: Mutex::new(Some(tx_ab)),
            rx: Mutex::new(Some(rx_ba)),
            partition: Mutex::new(PartitionState {
                partitioned: false,
                buffer: VecDeque::new(),
            }),
            faults: Mutex::new(LinkFaults { seed: seed_a, ..Default::default() }),
            rng: Mutex::new(StdRng::seed_from_u64(seed_a)),
            counters: Mutex::new(LinkCounters::default()),
            reorder_hold: Mutex::new(None),
            killed: AtomicBool::new(false),
            stalled: AtomicBool::new(false),
        });
        let b = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(a_id),
            caps: a_caps,
            tx: Mutex::new(Some(tx_ba)),
            rx: Mutex::new(Some(rx_ab)),
            partition: Mutex::new(PartitionState {
                partitioned: false,
                buffer: VecDeque::new(),
            }),
            faults: Mutex::new(LinkFaults { seed: seed_b, ..Default::default() }),
            rng: Mutex::new(StdRng::seed_from_u64(seed_b)),
            counters: Mutex::new(LinkCounters::default()),
            reorder_hold: Mutex::new(None),
            killed: AtomicBool::new(false),
            stalled: AtomicBool::new(false),
        });
        (a, b)
    }

    /// Toggle the partition on this endpoint. While `dropped == true`,
    /// `send()` queues tones in a bounded buffer (FIFO, oldest dropped
    /// when full) instead of delivering them. Flipping back to `false`
    /// flushes the buffer to the peer in original order.
    ///
    /// Partition is per-endpoint and per-direction; isolate a link by
    /// flipping both of its endpoints. Healing routes the buffer back
    /// through the fault model, so a recovered link is lossy like any
    /// other — never a perfect replay.
    pub fn set_partitioned(&self, partitioned: bool) {
        let drained = {
            let mut p = self.partition.lock();
            p.partitioned = partitioned;
            match partitioned {
                true => Vec::new(),
                false => p.buffer.drain(..).collect(),
            }
        };
        for tone in drained {
            let verdict = self.take_verdict();
            let lost = verdict.drop || self.push(tone, verdict.duplicate).is_err();
            if lost {
                self.counters.lock().lost_on_heal += 1;
            }
        }
    }

    /// `try_send` variant for the synchronous heal-flush path, where
    /// holding a lock across an await isn't an option.
    fn try_emit(&self, tone: Tone) -> Result<()> {
        let guard = self.tx.lock();
        let Some(tx) = guard.as_ref() else {
            return Err(anyhow::anyhow!("link to {} is dead", self.peer.id.short()));
        };
        tx.try_send(tone).map_err(|e| anyhow::anyhow!("push: {e}"))
    }

    /// Send without holding the sender lock across the await. Taking
    /// the sender out and putting it back is what keeps `kill` from
    /// deadlocking against a send that has already committed to it.
    async fn emit(&self, tone: Tone) -> Result<()> {
        if self.stalled.load(Ordering::SeqCst) {
            // Never completes. Modelled as a hang rather than an error
            // because that is what the real transport does, and an
            // error would let a fix that merely checks the return value
            // pass without ever testing the deadline.
            self.counters.lock().stalled_sends += 1;
            std::future::pending::<()>().await;
        }
        let tx = {
            let mut guard = self.tx.lock();
            match guard.take() {
                Some(tx) => tx,
                None => return Err(anyhow::anyhow!("link to {} is dead", self.peer.id.short())),
            }
        };
        let sent = tx.send(tone).await;
        // Reclaim the sender unless `kill` won the race and dropped it.
        let mut guard = self.tx.lock();
        if guard.is_none() {
            *guard = Some(tx);
        }
        sent.map_err(|e| anyhow::anyhow!("send: {e}"))
    }

    /// One tone onto the wire, optionally twice. `try_send` — a full or
    /// dead receiver during a heal flush is a lost tone, not an error the
    /// caller can act on.
    fn push(&self, tone: Tone, duplicate: bool) -> Result<()> {
        self.try_emit(tone.clone())?;
        self.counters.lock().delivered += 1;
        if duplicate && self.try_emit(tone).is_ok() {
            let mut c = self.counters.lock();
            c.delivered += 1;
            c.duplicated += 1;
        }
        Ok(())
    }

    fn take_verdict(&self) -> Verdict {
        let mut rng = self.rng.lock();
        self.faults.lock().verdict(&mut *rng)
    }

    /// `reorder_next` counts *pairs*. A tone held back always pairs with
    /// its successor, so an outstanding hold outranks the budget.
    fn take_reorder(&self) -> bool {
        if self.reorder_hold.lock().is_some() {
            let mut f = self.faults.lock();
            f.script.reorder_next = f.script.reorder_next.saturating_sub(1);
            return true;
        }
        self.faults.lock().script.reorder_next > 0
    }

    pub fn set_faults(&self, faults: LinkFaults) {
        let seed = faults.seed;
        *self.faults.lock() = faults;
        *self.rng.lock() = StdRng::seed_from_u64(seed);
    }

    pub fn faults(&self) -> LinkFaults { self.faults.lock().clone() }

    pub fn counters(&self) -> LinkCounters { *self.counters.lock() }

    pub fn buffered(&self) -> usize { self.partition.lock().buffer.len() }

    pub fn is_partitioned(&self) -> bool { self.partition.lock().partitioned }

    /// Frees a tone held for reorder when no partner arrives.
    pub fn flush_reorder(&self) -> bool {
        match self.reorder_hold.lock().take() {
            Some(tone) => self.try_emit(tone).is_ok(),
            None => false,
        }
    }

    /// Make this link stop draining. Sends to it hang rather than fail,
    /// and the peer stays registered — a stall is invisible to a liveness
    /// lease, which is the whole problem. Not the same as [`Self::kill`]
    /// (the link is gone, so the far side's receiver closes) or as a
    /// partition (the link stays nominally up and keeps buffering).
    pub fn stall(&self) {
        self.stalled.store(true, Ordering::SeqCst);
    }

    /// Let a stalled link drain again. A stall is a fault, not a
    /// teardown, so it has to be reversible.
    pub fn unstall(&self) {
        self.stalled.store(false, Ordering::SeqCst);
    }

    pub fn is_stalled(&self) -> bool {
        self.stalled.load(Ordering::SeqCst)
    }

    /// Drop the link as if the peer's process had vanished. Sends start
    /// failing and the drainer at the far end sees its receiver close,
    /// which marks that peer's lease `TransportClosed`.
    ///
    /// A partition is not this: a partitioned link stays nominally up
    /// and its peer may still be alive, so its lease keeps renewing and
    /// it is never reaped. That difference is the point — a partition
    /// heals itself, a dead peer needs a redial.
    pub fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        // Dropping our sender is what closes the peer's receiver, which
        // ends its drainer and marks its lease TransportClosed. Dropping
        // our own receiver would only stop *us* reading, leaving the
        // peer with a link that looks fine.
        self.tx.lock().take();
    }

    /// Whether this link has been killed.
    pub fn is_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PeerConnection for InMemoryEndpoint {
    fn peer(&self) -> &HumdAddr { &self.peer }
    fn capabilities(&self) -> &PeerCapabilities { &self.caps }

    async fn send(&self, tone: Tone) -> Result<()> {
        if self.killed.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("link to {} is dead", self.peer.id.short()));
        }
        self.counters.lock().offered += 1;

        {
            let mut p = self.partition.lock();
            if p.partitioned {
                if p.buffer.len() >= PARTITION_BUFFER_CAP {
                    p.buffer.pop_front();
                    self.counters.lock().evicted += 1;
                }
                p.buffer.push_back(tone);
                self.counters.lock().buffered += 1;
                return Ok(());
            }
        }

        let verdict = self.take_verdict();
        if verdict.drop {
            self.counters.lock().dropped += 1;
            return Ok(());
        }

        if self.take_reorder() {
            let held = self.reorder_hold.lock().take();
            if let Some(held) = held {
                self.counters.lock().reordered += 1;
                self.emit(tone).await?;
                self.counters.lock().delivered += 1;
                self.emit(held).await?;
                self.counters.lock().delivered += 1;
                return Ok(());
            }
            *self.reorder_hold.lock() = Some(tone);
            return Ok(());
        }

        self.emit(tone.clone()).await?;
        self.counters.lock().delivered += 1;
        if verdict.duplicate {
            self.emit(tone).await?;
            let mut c = self.counters.lock();
            c.delivered += 1;
            c.duplicated += 1;
        }
        Ok(())
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn close(&self) {
        // Drop both halves: the sender so the peer stops reading, the
        // receiver so we stop expecting. Idempotent.
        self.tx.lock().take();
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
    /// Liveness lease, stamped by the drainer on every inbound tone.
    lease: Lease,
}

/// One humd's view of the ensemble: peers it knows about, their
/// connections, their capabilities. Owned by the daemon.
///
/// Incoming tones from every installed peer fan into a single
/// `broadcast` channel — subscribe via [`Ensemble::subscribe`] to see
/// them. The `chi:"hello"` tones are absorbed here (they update
/// `learned_caps`) and not rebroadcast; everything else passes through.
pub struct Ensemble {
    me: Hid,
    peers: Arc<RwLock<HashMap<Hid, Peer>>>,
    inbox: Inbox,
    /// Shared gossip seen-set + per-topic broadcast senders. One Arc per
    /// ensemble; cloned into every install() drainer task so the dedup
    /// + topic dispatch happens without locking the main peer map.
    gossip: Arc<GossipState>,
    /// Kademlia routing table + pending FIND_NODE queries. Built up
    /// from `install()`'s bootstrap (each newly-connected peer's
    /// HumdAddr is stashed) and from advertised peers in incoming
    /// `kad-find-node-resp` tones. Drives `kad_find` lookups for
    /// HumdIds we haven't yet connected to directly.
    kad: Arc<KadState>,
    /// When true, peers whose `chi:"hello"` is missing or fails
    /// verification are ejected (T3+ federation semantics). When false
    /// (default, T1), unsigned hellos are tolerated — caps are learned
    /// without proof of ownership and the connection stays installed.
    /// Invalid (signed-but-fails-verify) hellos are *always* ejected
    /// regardless of mode: a present pubkey that fails to verify is
    /// hostile, not legacy.
    strict_auth: bool,
    /// Tones dropped for arriving past their own `dusk`. Counts what the
    /// expiry rule actually caught, so a scenario can assert on it
    /// instead of inferring from what did arrive.
    expired_dusk: Arc<AtomicU64>,
    /// Mids already dispatched here, so a retransmit is delivered once.
    delivery: Arc<DeliveryState>,
    /// Sends that did not complete. A stalled peer has to be a number
    /// someone can alert on, not a `Lagged(n)` on a broadcast receiver.
    send_stats: Arc<SendStats>,
}

/// The local fan-out point for tones arriving from peers. Cloned into
/// every install() drainer so the sender and its accounting travel
/// together.
#[derive(Clone)]
pub struct Inbox {
    tx: broadcast::Sender<Tone>,
    subscribers: Arc<AtomicUsize>,
    dropped: Arc<AtomicU64>,
}

impl Inbox {
    fn new() -> Self {
        // 256 keeps recent tones available for slow subscribers without
        // unbounded memory; lagging consumers see Lagged and resync.
        let (tx, _) = broadcast::channel(256);
        Self { tx, subscribers: Arc::new(AtomicUsize::new(0)), dropped: Arc::new(AtomicU64::new(0)) }
    }

    /// Hand a drained tone to local subscribers. Returns false when there
    /// were none, in which case the tone is gone — not queued, not
    /// retried. Counted and logged because a tone accepted off the
    /// network and then destroyed here is otherwise invisible.
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
        InboxSub { rx: self.tx.subscribe(), subscribers: self.subscribers.clone() }
    }

    pub fn has_subscribers(&self) -> bool { self.subscribers.load(Ordering::SeqCst) > 0 }

    pub fn dropped(&self) -> u64 { self.dropped.load(Ordering::Relaxed) }
}

/// A live inbox subscription. Derefs to the receiver, so existing
/// `rx.recv()` call sites are unchanged; dropping it unregisters.
pub struct InboxSub {
    rx: broadcast::Receiver<Tone>,
    subscribers: Arc<AtomicUsize>,
}

impl Deref for InboxSub {
    type Target = broadcast::Receiver<Tone>;
    fn deref(&self) -> &Self::Target { &self.rx }
}

impl std::ops::DerefMut for InboxSub {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.rx }
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
    /// The peer stopped reading and the write did not complete in time.
    /// Distinct from a plain failure because the connection was closed
    /// on purpose — the peer should be gone from the registry shortly.
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
            strict_auth: false,
            expired_dusk: Arc::new(AtomicU64::new(0)),
            delivery: DeliveryState::new(),
            send_stats: SendStats::new(),
        }
    }

    /// Build an ensemble that rejects peers whose hellos aren't
    /// cryptographically verified. Federation (T3+) wants this on;
    /// own-devices (T1) leaves it off and tolerates unsigned T1 hellos.
    pub fn with_strict_auth(me: Hid, strict: bool) -> Self {
        let mut e = Self::new(me);
        e.strict_auth = strict;
        e
    }

    pub fn me(&self) -> Hid { self.me }

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
            Peer { conn: conn.clone(), learned_caps: None, lease: Lease::new() },
        );
        // Bootstrap the kad routing table with the peer we just wired.
        // The HumdAddr from the transport carries whatever dial hints
        // that transport produced (e.g. iroh: prefix); kad reuses them
        // verbatim when advertising this peer to remote FIND_NODE callers.
        self.kad.note_peer(conn.peer().clone());

        if let Some(mut rx) = rx {
            let peers = self.peers.clone();
            let inbox = self.inbox.clone();
            let conn_for_drain = conn.clone();
            let strict = self.strict_auth;
            let gossip = self.gossip.clone();
            let expired_dusk = self.expired_dusk.clone();
            let delivery = self.delivery.clone();
            let send_stats = self.send_stats.clone();
            let kad = self.kad.clone();
            let my_id = self.me;
            tokio::spawn(async move {
                // Only the FIRST chi:"hello" off this connection is the
                // peer handshake — we absorb it to learn caps. Any
                // subsequent chi:"hello" is application-level (a
                // tunnelled nestler announcing itself, etc.) and must
                // pass through to subscribers.
                //
                // `id` is the transport-level peer id (real for iroh,
                // placeholder for TCP-accept etc.). On a verified
                // hello whose claimed_id differs, we re-key the
                // registry from `id` to `claimed_id` — the signature
                // is authoritative over the transport view.
                let mut id = id;
                let mut handshake_seen = false;
                while let Some(tone) = rx.recv().await {
                    let is_hello = tone.get("chi").and_then(|v| v.as_str()) == Some("hello");
                    if is_hello && !handshake_seen {
                        handshake_seen = true;
                        match parse_hello(&tone) {
                            HelloParse::Verified(claimed_id, caps) => {
                                if claimed_id != id {
                                    rekey_peer(&peers, &kad, &conn_for_drain, id, claimed_id);
                                    id = claimed_id;
                                }
                                if let Some(p) = peers.write().get_mut(&id) {
                                    p.learned_caps = Some(caps);
                                }
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
                                // T1 compat: learn caps without proof.
                                // Unsigned hellos can't re-key safely
                                // (no sig to back the claim), so the
                                // registry key has to match the
                                // transport view.
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
                    // Gossip pub-sub: chi:"gossip-publish" gets deduped
                    // against the seen-set, dispatched to topic
                    // subscribers, and re-fanned to every OTHER peer.
                    // Falls through to the inbox fan-out if the tone is
                    // malformed (treats it as opaque application data).
                    if handle_liveness(&peers, &my_id, &id, &conn_for_drain, &tone).await {
                        continue;
                    }
                    // After the liveness stamp: an expired tone still
                    // proves the link is alive, it just has nothing
                    // left worth delivering or re-fanning. `admit`
                    // also drops a `mid` this ensemble already
                    // dispatched, so a retransmit is delivered once.
                    // Checked before gossip so a duplicate is not
                    // re-fanned either.
                    if !delivery::admit(&tone, &delivery, &expired_dusk) {
                        continue;
                    }
                    if tone.get("chi").and_then(|v| v.as_str()) == Some(GOSSIP_CHI) {
                        if handle_gossip(&send_stats, &gossip, &peers, &id, &tone).await {
                            continue;
                        }
                    }
                    // Kademlia DHT: FIND_NODE queries / responses are
                    // absorbed by the kad layer (responses notify a
                    // pending lookup; queries are answered with the
                    // routing table's K closest to target). Malformed
                    // tones fall through to the regular inbox fan-out.
                    let chi_val = tone.get("chi").and_then(|v| v.as_str());
                    if chi_val == Some(KAD_FIND_NODE_CHI)
                        || chi_val == Some(KAD_FIND_NODE_RESP_CHI)
                    {
                        if handle_kad(&send_stats, &kad, &peers, &id, &my_id, &tone).await {
                            continue;
                        }
                    }
                    // Everything else (including subsequent hellos) fans
                    // out. Receivers may be absent — broadcast drops.
                    inbox.publish(tone);
                }
                // The transport's receiver closed. Mark the lease dead
                // so a sweep can reap it; the registry entry itself
                // outlives the drainer by design, because the daemon
                // owns eviction and redial.
                if let Some(p) = peers.write().get_mut(&id) {
                    p.lease.observe(LivenessSignal::TransportClosed);
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
            Peer { conn: conn.clone(), learned_caps: None, lease: Lease::new() },
        );
        // Bootstrap the kad routing table — same as `install`.
        self.kad.note_peer(conn.peer().clone());

        if let Some(mut rx) = rx {
            let peers = self.peers.clone();
            let inbox = self.inbox.clone();
            let conn_for_drain = conn.clone();
            let strict = self.strict_auth;
            let gossip = self.gossip.clone();
            let expired_dusk = self.expired_dusk.clone();
            let delivery = self.delivery.clone();
            let send_stats = self.send_stats.clone();
            let kad = self.kad.clone();
            let my_id = self.me;
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
                    if handle_liveness(&peers, &my_id, &id, &conn_for_drain, &tone).await {
                        continue;
                    }
                    // After the liveness stamp: an expired tone still
                    // proves the link is alive, it just has nothing
                    // left worth delivering or re-fanning. `admit`
                    // also drops a `mid` this ensemble already
                    // dispatched, so a retransmit is delivered once.
                    // Checked before gossip so a duplicate is not
                    // re-fanned either.
                    if !delivery::admit(&tone, &delivery, &expired_dusk) {
                        continue;
                    }
                    if tone.get("chi").and_then(|v| v.as_str()) == Some(GOSSIP_CHI) {
                        if handle_gossip(&send_stats, &gossip, &peers, &id, &tone).await {
                            continue;
                        }
                    }
                    let chi_val = tone.get("chi").and_then(|v| v.as_str());
                    if chi_val == Some(KAD_FIND_NODE_CHI)
                        || chi_val == Some(KAD_FIND_NODE_RESP_CHI)
                    {
                        if handle_kad(&send_stats, &kad, &peers, &id, &my_id, &tone).await {
                            continue;
                        }
                    }
                    inbox.publish(tone);
                }
                // The transport's receiver closed. Mark the lease dead
                // so a sweep can reap it; the registry entry itself
                // outlives the drainer by design, because the daemon
                // owns eviction and redial.
                if let Some(p) = peers.write().get_mut(&id) {
                    p.lease.observe(LivenessSignal::TransportClosed);
                }
            });
        }
    }

    /// Send a `chi:"peer-ping"` to every installed peer. A peer whose
    /// link is wedged will not answer, and the answer is what renews
    /// its lease — so a sweep after `ttl` reaps it.
    pub async fn probe_all(&self, seq: u64) {
        let peers: Vec<(Hid, Arc<dyn PeerConnection>)> = self
            .peers
            .read()
            .iter()
            .map(|(id, p)| (*id, p.conn.clone()))
            .collect();
        for (id, conn) in peers {
            let _ = send_bounded(&conn, ping_tone(&self.me, &id, seq), &self.send_stats).await;
        }
    }

    /// Ping one peer. Direction matters: a probe answers a question
    /// about the link in the direction it travels, so probing a->b
    /// says nothing about whether b->a still works.
    pub async fn probe_one(&self, id: &Hid, seq: u64) {
        // Clone out from under the lock: holding the registry read
        // lock across the send would block add_peer/remove_peer for
        // as long as the link takes.
        let conn = self.peers.read().get(id).map(|p| p.conn.clone());
        if let Some(conn) = conn {
            let _ = send_bounded(&conn, ping_tone(&self.me, id, seq), &self.send_stats).await;
        }
    }

    /// Liveness of one peer. `None` if it isn't installed.
    pub fn peer_liveness(&self, id: &Hid, ttl: std::time::Duration) -> Option<Liveness> {
        self.peers.read().get(id).map(|p| p.lease.state(ttl))
    }

    /// Every peer that has stopped answering, and is due for eviction.
    pub fn expired_peers(&self, ttl: std::time::Duration) -> Vec<Hid> {
        self.peers
            .read()
            .iter()
            .filter(|(_, p)| p.lease.expired(ttl))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Mark a peer dead as if its transport had closed. Lets a scenario
    /// produce a death without waiting out a TTL, which is the only way
    /// to test eviction deterministically.
    pub fn expire_peer(&self, id: &Hid) {
        if let Some(p) = self.peers.write().get_mut(id) {
            p.lease.observe(LivenessSignal::TransportClosed);
        }
    }

    /// Evict every peer whose lease has run out, closing each link.
    /// Returns the evicted ids so the caller can redial them.
    pub fn evict_expired(&self, ttl: std::time::Duration) -> Vec<Hid> {
        let expired = self.expired_peers(ttl);
        for id in &expired {
            self.remove_peer(id);
        }
        expired
    }

    pub fn remove_peer(&self, id: &Hid) {
        if let Some(p) = self.peers.write().remove(id) {
            p.conn.close();
        }
    }

    pub fn peers(&self) -> Vec<Hid> {
        self.peers.read().keys().copied().collect()
    }

    /// Capabilities the peer announced via `chi:"hello"`. Falls back to
    /// the transport-supplied caps if no hello has arrived yet.
    pub fn peer_caps(&self, id: &Hid) -> Option<PeerCapabilities> {
        self.peers.read().get(id).map(|p| {
            p.learned_caps
                .clone()
                .unwrap_or_else(|| p.conn.capabilities().clone())
        })
    }

    /// True once this peer's `chi:"hello"` has been parsed and its caps
    /// learned. A scenario that needs a clean fault budget waits on this
    /// — the handshake is link setup and is itself faultable.
    pub fn handshake_done(&self, id: &Hid) -> bool {
        self.peers.read().get(id).is_some_and(|p| p.learned_caps.is_some())
    }

    /// Subscribe to incoming tones from every installed peer. Hellos
    /// are absorbed by the ensemble; subscribers only see real traffic.
    pub fn subscribe(&self) -> InboxSub { self.inbox.subscribe() }

    /// True once something is attached to the inbox. Callers that install
    /// peers before the local pump is up wait on this instead of racing it.
    pub fn has_subscribers(&self) -> bool { self.inbox.has_subscribers() }

    /// Tones that reached a drainer and were then destroyed because no
    /// pump was listening. Never zero on a healthy daemon; a non-zero
    /// value is a boot-order race that silently ate network traffic.
    pub fn inbox_dropped(&self) -> u64 { self.inbox.dropped() }

    /// Tones dropped for arriving past their `dusk`.
    pub fn expired_dusk(&self) -> u64 {
        self.expired_dusk.load(Ordering::SeqCst)
    }

    /// Number of mids currently remembered for at-most-once delivery.
    pub fn delivery_seen(&self) -> usize {
        self.delivery.len()
    }

    /// Peer sends that did not complete. `timed_out` is the interesting
    /// one: it means a peer stopped reading, and each one should have
    /// cost that peer its place in the registry.
    pub fn send_timeouts(&self) -> u64 {
        self.send_stats.timed_out()
    }

    /// Peer sends that failed outright, as opposed to stalling.
    pub fn send_failures(&self) -> u64 {
        self.send_stats.failed()
    }

    /// Publish a gossip message to every installed peer. Mints a fresh
    /// `msg_id`, marks it seen locally
    /// (so we don't re-fan it on the inevitable echo), and sends a
    /// `chi:"gossip-publish"` tone over every `PeerConnection`. Local
    /// `subscribe_topic` subscribers do NOT see their own publish — that
    /// matches typical pub-sub ergonomics (and matches the test fixture
    /// in `gossip_integration.rs`); use a direct channel if you want to
    /// hear yourself.
    ///
    /// Best-effort: per-peer send failures are logged but don't abort
    /// the broadcast — one slow link can't stall the mesh. Sits ABOVE
    /// `route()` semantically; both share the `PeerConnection.send`
    /// wire but `publish` is mesh-wide and `route` is unicast.
    pub async fn publish(&self, topic: &str, payload: serde_json::Value) {
        self.publish_with_dusk(topic, payload, None).await
    }

    /// As [`Self::publish`], with a lifetime in ms. Every hop re-fans,
    /// so a slow mesh can deliver a gossip tone long after it was sent;
    /// `dusk_ms` is how long it stays worth acting on. `None` (the
    /// default) never expires — see [`gossip_tone_with_dusk`] for why
    /// there is no default TTL.
    pub async fn publish_with_dusk(
        &self,
        topic: &str,
        payload: serde_json::Value,
        dusk_ms: Option<i64>,
    ) {
        let msg_id = mint_msg_id(&self.me);
        let rid = format!("gossip-{msg_id}");
        // Mark seen locally so the next-hop echo (peer re-fans back to
        // us) is dropped at the drainer's seen check.
        self.gossip.note_seen(&msg_id);
        let tone = gossip_tone_with_dusk(topic, &rid, &self.me, payload, &msg_id, dusk_ms);
        let conns: Vec<Arc<dyn PeerConnection>> = {
            let peers = self.peers.read();
            peers.values().map(|p| p.conn.clone()).collect()
        };
        for conn in conns {
            if let Err(e) = send_bounded(&conn, tone.clone(), &self.send_stats).await {
                tracing::debug!(
                    target: "ensemble.gossip",
                    peer = %conn.peer().id.short(),
                    topic = topic,
                    error = %e,
                    "publish send failed"
                );
            }
        }
    }

    /// Subscribe to a gossip topic. Returns a `broadcast::Receiver`
    /// scoped to ONE topic — distinct from `subscribe()` which sees
    /// every tone the ensemble drainer fans out. The channel is created
    /// lazily on first call and shared across subsequent subscribers
    /// to the same topic.
    pub fn subscribe_topic(&self, topic: &str) -> broadcast::Receiver<serde_json::Value> {
        self.gossip.subscribe(topic)
    }

    /// Announce a bee running on this humd to the mesh. Wraps a
    /// [`HiveAnnounce::Advertise`] in a gossip-publish on
    /// [`ANNOUNCE_TOPIC`]. Call on each nestler handshake; safe to call
    /// repeatedly (the receiver dedups on payload hash via gossip's
    /// seen-set, and a manifest update is just a new advertise tone).
    pub async fn hive_advertise(&self, manifest: hives::HiveManifest) {
        let env = hives::HiveAnnounce::Advertise {
            humd_id: self.me.to_hex(),
            manifest,
        };
        match serde_json::to_value(&env) {
            Ok(payload) => self.publish(hives::ANNOUNCE_TOPIC, payload).await,
            Err(e) => tracing::warn!(target: "ensemble.bees", error = %e, "advertise serialize"),
        }
    }

    /// Announce that a bee has gone away. Same channel as
    /// [`Ensemble::hive_advertise`], envelope kind = `retract`.
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

    /// Raw subscription to every bee announcement on the mesh.
    /// Returns a typed mpsc receiver of [`HiveAnnounce`] envelopes
    /// (parsed from the underlying gossip topic). Malformed payloads
    /// are logged and dropped. Backed by a tokio task; drop the
    /// receiver to stop it.
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

    /// Discover humds advertising a bee with the given `name`.
    /// Returns an mpsc receiver of `(Hid, HiveManifest)` pairs
    /// for matching `Advertise` envelopes. Retract envelopes are
    /// dropped (caller should track their own roster of seen humds and
    /// expire entries on retract — surfaced via [`Self::hive_announcements`]).
    pub fn hive_discover(&self, name: impl Into<String>) -> mpsc::Receiver<(Hid, hives::HiveManifest)> {
        let needle = name.into();
        let mut raw = self.subscribe_topic(hives::ANNOUNCE_TOPIC);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                match raw.recv().await {
                    Ok(v) => {
                        let parsed: Result<hives::HiveAnnounce, _> =
                            serde_json::from_value(v);
                        if let Ok(hives::HiveAnnounce::Advertise { humd_id, manifest }) = parsed {
                            if manifest.name != needle {
                                continue;
                            }
                            if let Ok(id) = Hid::from_hex(&humd_id) {
                                if tx.send((id, manifest)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
        rx
    }

    /// Iterative Kademlia FIND_NODE lookup for `target`. Returns the
    /// HumdAddr matching `target` if any peer's routing table knew it,
    /// otherwise `None`.
    ///
    /// Procedure:
    ///   1. Seed the routing table with every currently-installed peer
    ///      (idempotent — `install` already does this, this is belt
    ///      and braces).
    ///   2. Take the α=3 closest unqueried addresses from local table.
    ///   3. Send `chi:"kad-find-node"` to each in parallel via the
    ///      peer's [`PeerConnection::send`] — same wire as gossip.
    ///   4. Merge every advertised peer from incoming responses back
    ///      into the routing table. Round-by-round, re-query the α
    ///      closest unqueried until no closer node is returned.
    ///   5. Terminate when no round produces a strictly-closer entry,
    ///      after `KAD_MAX_ROUNDS`, or on `timeout` — whichever first.
    ///
    /// We trust returned HumdAddrs from peers — no signature on the
    /// resp yet. A hostile peer can return arbitrary addresses; the
    /// caller's job is to attempt to dial and verify the handshake.
    pub async fn kad_find(&self, target: Hid, timeout: Duration) -> Option<HumdAddr> {
        // Quick check: target already in our routing table (we
        // installed a connection to it directly).
        if let Some(addr) = self.kad.get(&target) {
            return Some(addr);
        }

        // Belt-and-braces: ensure every installed peer is in the
        // routing table. install() already does this, but this keeps
        // kad_find honest if a peer's HumdAddr was somehow missed.
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
        // Per-query timeout: a fraction of the wall budget so a slow
        // peer can't starve the whole lookup. Min 50ms — much shorter
        // than that and InMemoryEndpoint setup races eat the budget.
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
                // No more peers to query — converged.
                break;
            }
            let before = shortlist.closest_distance();

            // Resolve each batch entry to a live connection. We can
            // only query peers we already have a PeerConnection for.
            // Advertised-but-not-installed peers come back as routing
            // hints but can't be dialed without a transport (T4 dial
            // path is a follow-up).
            let mut joinset: JoinSet<Vec<HumdAddr>> = JoinSet::new();
            for addr in &batch {
                shortlist.mark_queried(addr.id);
                let conn = {
                    let peers = self.peers.read();
                    peers.get(&addr.id).map(|p| p.conn.clone())
                };
                if let Some(conn) = conn {
                    let kad = self.kad.clone();
                    let me = self.me;
                    let tgt = target;
                    joinset.spawn(async move {
                        kad::query_peer(&kad, &conn, &me, &tgt, per_query_timeout).await
                    });
                }
            }
            if joinset.is_empty() {
                // Every closest unqueried peer is uninstalled — can't
                // make progress. Mark them queried (already done above)
                // and continue; next round will pick the next α.
                continue;
            }
            while let Some(res) = joinset.join_next().await {
                let advertised_list = match res {
                    Ok(list) => list,
                    Err(_) => continue,
                };
                for advertised in advertised_list {
                    // Note into routing table AND shortlist.
                    self.kad.note_peer(advertised.clone());
                    if advertised.id == self.me {
                        continue;
                    }
                    shortlist.insert(advertised);
                }
            }

            // Found it directly in this round's resp?
            if let Some(addr) = self.kad.get(&target) {
                return Some(addr);
            }
            let after = shortlist.closest_distance();
            if after >= before {
                // No closer node returned this round — Kademlia
                // termination condition.
                break;
            }
        }

        // Final check: maybe a stale resp filled the table after the
        // loop terminated.
        if let Some(addr) = self.kad.get(&target) {
            return Some(addr);
        }
        // No exact match. Caller treats None as "not found"; the
        // shortlist's closest may still be useful for diagnostics.
        let _ = shortlist.closest();
        None
    }

    /// Snapshot of the routing table size — useful for tests and
    /// diagnostics that want to assert the table grew during a lookup.
    pub fn kad_routing_table_len(&self) -> usize {
        self.kad.table.lock().len()
    }

    /// Snapshot the routing table's `count` peers closest to `target`
    /// in XOR space. Exposed for callers that want to drive their own
    /// dial-after-lookup loop (the in-memory test fixture, primarily;
    /// real transports will hide this behind a `kad_find_and_dial`).
    pub fn kad_closest(&self, target: &Hid, count: usize) -> Vec<HumdAddr> {
        self.kad.closest_to(target, count)
    }

    /// Send a tone to the peer named in `tone.to` (must be present and
    /// a valid hex Hid). Tone is `serde_json::Value` per thrum-core's
    /// loose shape.
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
pub fn parse_hello_caps(tone: &Tone) -> Option<(Hid, PeerCapabilities)> {
    let proto_version = tone.get("proto_version")?.as_str()?.to_string();

    let claimed_humd_id_hex = tone.get("humd_id")?.as_str()?;
    let claimed_id = match Hid::from_hex(claimed_humd_id_hex) {
        Ok(h) => h,
        Err(_) => {
            tracing::warn!(target: "ensemble", "hello.rejected: humd_id unparseable");
            return None;
        }
    };

    let pubkey_hex = tone.get("pubkey").and_then(|v| v.as_str())?;
    let pubkey_bytes = hex::decode(pubkey_hex).ok()?;
    if pubkey_bytes.len() != 32 {
        tracing::warn!(target: "ensemble", "hello.rejected: pubkey wrong length");
        return None;
    }
    let mut pubkey_arr = [0u8; 32];
    pubkey_arr.copy_from_slice(&pubkey_bytes);

    if Hid::from_pubkey(HidPrefix::Humd, &pubkey_arr) != claimed_id {
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
    let headroom = tone
        .get("headroom")
        .and_then(|v| serde_json::from_value::<headroom::CellHeadroom>(v.clone()).ok())
        .unwrap_or_default();
    Some((
        claimed_id,
        PeerCapabilities { proto_version, nests, hosts, can_relay, free_slots, headroom },
    ))
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
        let Ok(claimed_id) = Hid::from_hex(humd_hex) else {
            return HelloParse::Invalid;
        };
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
        let headroom = tone
            .get("headroom")
            .and_then(|v| serde_json::from_value::<headroom::CellHeadroom>(v.clone()).ok())
            .unwrap_or_default();
        HelloParse::Unsigned(
            claimed_id,
            PeerCapabilities { proto_version, nests, hosts, can_relay, free_slots, headroom },
        )
    }
}

/// Drainer-side handling for a `chi:"gossip-publish"` tone. Returns
/// `true` if the tone was consumed by the gossip layer (don't fan into
/// the regular inbox), `false` if it was malformed (fall back to
/// treating it as opaque traffic).
///
/// Semantics:
///   1. Parse topic + msg_id + payload. Malformed → return false.
///   2. Check `msg_id` against the seen-set. Already seen → consumed
///      (return true), nothing else happens (dedup short-circuit).
///   3. Mark seen. Dispatch payload to any local topic subscribers.
///   4. Re-fan the original tone to every OTHER installed peer (skip
///      the peer it arrived from — `arrived_from`).
///
/// Send failures during re-fan are logged but don't stop the loop:
/// gossip is best-effort; one dead link can't deafen the mesh.
/// Drainer-side handling for `chi:"kad-find-node"` and
/// `chi:"kad-find-node-resp"` tones.
///
/// Returns `true` if the tone was consumed by the kad layer (don't fan
/// into the regular inbox), `false` if malformed (fall back to the
/// inbox so callers see the raw tone).
///
/// Semantics:
///   - `kad-find-node`: parse, look up K closest HumdAddrs to the
///     advertised `target` in our routing table, send a
///     `kad-find-node-resp` back over the same peer connection.
///   - `kad-find-node-resp`: insert every advertised HumdAddr into the
///     routing table, then deliver to the pending oneshot keyed by
///     `query_id`. If no waiter is registered (timeout already fired,
///     or this is a duplicate resp) we still keep the new routing
///     info — a stale resp is still useful peer-discovery signal.
/// Move an installed peer entry from the transport-level placeholder
/// id to the cryptographically-verified id from its signed hello.
/// Called by the install drainer when the first verified hello reveals
/// the real id is different from what the transport reported (TCP-
/// accept and other transports that can't authenticate the peer
/// pre-handshake).
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
        // Reply to the peer that asked. We send K closest from our
        // routing table — they may include `arrived_from` itself, which
        // is harmless (the caller filters self / already-queried).
        let closest = kad.closest_to(&parsed.target, KAD_K);
        let resp_rid = format!("kad-resp-{}", &parsed.query_id[..8.min(parsed.query_id.len())]);
        let resp = kad::find_node_resp_tone(&resp_rid, &parsed.query_id, me, &closest);
        let conn = {
            let peers = peers.read();
            peers.get(arrived_from).map(|p| p.conn.clone())
        };
        if let Some(conn) = conn {
            if let Err(e) = send_bounded(&conn, resp, send_stats).await {
                tracing::debug!(
                    target: "ensemble.kad",
                    peer = %arrived_from.short(),
                    error = %e,
                    "find-node response send failed"
                );
            }
        }
        true
    } else if chi_val == Some(kad::KAD_FIND_NODE_RESP_CHI) {
        let parsed = match kad::parse_find_node_resp(tone) {
            Some(p) => p,
            None => return false,
        };
        // Note every advertised peer into the routing table — useful
        // even if no waiter is registered (drives passive discovery).
        for addr in &parsed.closest {
            kad.note_peer(addr.clone());
        }
        // Deliver to a pending lookup, if any. False return just means
        // "no live waiter" — not an error.
        let _ = kad.deliver_response(&parsed.query_id, parsed.closest);
        true
    } else {
        false
    }
}

/// Renew a peer's lease and answer any probe. Returns true when the
/// tone was a liveness control message and should not reach
/// subscribers — a `peer-ping` is answered here and swallowed, a
/// `peer-pong` is absorbed silently, and everything else falls through.
async fn handle_liveness(
    peers: &Arc<RwLock<HashMap<Hid, Peer>>>,
    me: &Hid,
    arrived_from: &Hid,
    conn: &Arc<dyn PeerConnection>,
    tone: &Tone,
) -> bool {
    // The arrival itself is the lease renewal, for every inbound tone.
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
    // Answer the probe, then swallow it: a probe is link maintenance,
    // not application traffic.
    if let Some(seq) = probe_seq(tone) {
        let _ = conn.send(pong_tone(me, arrived_from, seq)).await;
    }
    true
}

async fn handle_gossip(
    send_stats: &SendStats,
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
        // Already saw this msg_id — drop. Don't dispatch, don't re-fan.
        return true;
    }
    // Local dispatch: if anyone subscribed to this topic, deliver the
    // payload. Subscribers see the payload value only, not the wire
    // envelope — they don't care about msg_id / from at the API level.
    if let Some(tx) = gossip.sender(parsed.topic) {
        let _ = tx.send(parsed.payload.clone());
    }
    // Re-fan to every OTHER installed peer. Snapshot the connection
    // list under the read lock, then release it before the awaits so
    // we don't hold parking_lot across an await point.
    let others: Vec<Arc<dyn PeerConnection>> = {
        let peers = peers.read();
        peers
            .iter()
            .filter(|(id, _)| *id != arrived_from)
            .map(|(_, p)| p.conn.clone())
            .collect()
    };
    for conn in others {
        if let Err(e) = send_bounded(&conn, tone.clone(), send_stats).await {
            tracing::debug!(
                target: "ensemble.gossip",
                peer = %conn.peer().id.short(),
                error = %e,
                "re-fan send failed"
            );
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Link fault model ───────────────────────────────────────────────

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

    async fn drain(rx: &mut mpsc::Receiver<Tone>) -> Vec<Tone> {
        let mut out = Vec::new();
        while let Ok(t) = rx.try_recv() {
            out.push(t);
        }
        out
    }

    fn rids(tones: &[Tone]) -> Vec<String> {
        tones.iter().map(|t| t["rid"].as_str().unwrap().to_string()).collect()
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
        assert_eq!(c, LinkCounters { offered: 5, delivered: 5, ..Default::default() });
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
        assert_ne!(run(1).await, run(2).await, "different seeds, different patterns");
        let survived = run(42).await;
        // 200 draws at 30% loss; sd is ~6.5, so this is a 3-sigma band.
        assert!((120..170).contains(&survived), "survived {survived} of 200 at 30% loss");
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

    /// The behaviour this whole model exists to make possible: a link
    /// that recovers can still lose what it buffered. Replaying the
    /// buffer intact would let a partition test pass without ever
    /// exercising recovery.
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

    /// The oracle has to be internally consistent or no test can trust it.
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
        assert_eq!(c.delivered, arrived, "delivered must match what the peer saw");
        assert_eq!(c.offered, 43);
        assert_eq!(c.buffered, 3);
        // `delivered` counts duplicate copies, so discount them to
        // compare against what was offered and not lost.
        assert_eq!(c.offered, c.delivered - c.duplicated + c.dropped + c.lost_on_heal);
    }

    #[tokio::test]
    async fn faults_are_per_direction() {
        let (a, b) = link_pair();
        let mut a_rx = a.take_receiver().unwrap();
        let mut b_rx = b.take_receiver().unwrap();
        a.set_faults(LinkFaults::default().drop_next(10));
        b.send(tone("survivor")).await.unwrap();
        a.send(tone("doomed")).await.unwrap();
        assert_eq!(rids(&drain(&mut a_rx).await), ["survivor"], "b→a is unaffected");
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
        // `humd_` prefix (5 chars) + `_` (already in prefix) + 64 hex tail.
        assert_eq!(hex.len(), "humd_".len() + 64);
        assert!(hex.starts_with("humd_"));
    }

    #[test]
    fn hid_legacy_bare_hex_parses_as_humd() {
        // peers.json files written before the rename carry bare 64-hex.
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
        let me = Hid::random_humd();
        let peer_id = Hid::random_humd();
        let other_id = Hid::random_humd();

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
    /// learn the other's Hid + caps via the install handshake.
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
        let me = me_key.hid();
        let peer_id = peer_key.hid();
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

    /// Non-hello tones from a peer must reach `subscribe()` listeners;
    /// hellos are absorbed and never surface.
    #[tokio::test]
    async fn subscribe_forwards_remote_tones_but_swallows_hello() {
        let me_key = HumdKey::generate();
        let peer_key = HumdKey::generate();
        let me = me_key.hid();
        let peer_id = peer_key.hid();
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
