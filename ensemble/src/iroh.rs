//! Iroh QUIC transport for the ensemble.
//!
//! [`iroh::Endpoint`] gives us peer-to-peer QUIC: TLS-encrypted, NAT
//! hole-punched, addressed by an Ed25519 public key (iroh's `NodeId`).
//! We frame each tone as one NDJSON line on a single bi-directional
//! stream — same wire shape as [`crate::tcp`], different socket. Auth
//! at the QUIC layer means a connection is already cryptographically
//! bound to the remote NodeId by the time the ensemble handshake runs;
//! we still send the `chi:"hello"` so the ensemble's Hid/cap layer
//! stays transport-agnostic.
//!
//! Hid mapping: iroh's NodeId *is* an Ed25519 verifying key. Our
//! `Hid = sha256(pubkey)`. The IrohEndpoint stores both — `peer()`
//! returns the [`HumdAddr`] derived from the NodeId.
//!
//! Shape:
//! - [`IrohEndpoint`] — one live [`PeerConnection`] wrapping an open
//!   bi-directional stream on an established `iroh::Connection`. The
//!   read half is drained by a background task into the receiver mpsc;
//!   `send` writes a serialised tone + newline under a `tokio::Mutex`.
//! - [`IrohTransport`] — [`Transport`] impl. Wraps a built
//!   `iroh::Endpoint` for outbound dials; reads the first `iroh:<nodeid>`
//!   hint off the [`HumdAddr`] to find the dial target.
//! - ALPN: [`IROH_ALPN`] (`b"hum/0.5"`) — bumps when the wire shape
//!   breaks compat, surfaces as a clean handshake reject otherwise.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, Mutex};

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, PublicKey, TransportAddr};

use crate::{HumdAddr, Hid, PeerCapabilities, PeerConnection, Tone, Transport};

/// QUIC ALPN advertised by both sides. Pinned to the thrum protocol's
/// major.minor — if the on-wire framing or handshake shape changes
/// incompatibly, bump this and old peers fail to negotiate cleanly.
pub const IROH_ALPN: &[u8] = b"hum/0.5";

/// Inbound-channel capacity — matches `InMemoryEndpoint` / `TcpEndpoint`.
const RECV_CAP: usize = 256;

/// Hint prefix on a [`HumdAddr`] that signals "dial this NodeId over
/// iroh." Value after the colon is the NodeId as 64-char lowercase hex
/// (the same shape iroh's `PublicKey` accepts via `from_str`).
pub const IROH_HINT: &str = "iroh:";

/// Hint prefix for a direct IP/port the dialer should attempt — needed
/// when relay is disabled and address-lookup isn't configured (e.g. in
/// loopback tests). Value is a `SocketAddr` string. Multiple `iroh-ip:`
/// hints accumulate as multiple direct addresses on the dial target.
pub const IROH_IP_HINT: &str = "iroh-ip:";

/// Convert an iroh [`EndpointId`] (Ed25519 verifying key) into our
/// content-addressable [`Hid`]. `Hid = sha256(pubkey)`.
pub fn humd_id_from_node_id(node_id: &EndpointId) -> Hid {
    let mut h = Sha256::new();
    h.update(node_id.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    Hid::from(out)
}

/// One iroh-backed peer link. A bi-directional QUIC stream carries
/// NDJSON tones in both directions; a background reader task drains
/// `recv` into the receiver mpsc.
pub struct IrohEndpoint {
    peer: HumdAddr,
    node_id: EndpointId,
    caps: PeerCapabilities,
    /// The underlying QUIC connection. Held so the stream stays open
    /// for the lifetime of the endpoint — dropping all `Connection`
    /// handles tears down the link.
    _conn: Connection,
    /// Serialises concurrent writers. `Option` so `close` can drop the
    /// send half and subsequent sends fail cleanly. NDJSON framing
    /// requires atomic line writes.
    sender: Mutex<Option<SendStream>>,
    /// Inbound stream. Take-once, mirrored from the sibling transports.
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
}

impl IrohEndpoint {
    /// Build an endpoint from an established [`Connection`] + an open
    /// bi-directional stream pair. The reader half is owned by a
    /// background task that drains lines into the receiver mpsc.
    pub fn from_streams(
        node_id: EndpointId,
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        caps: PeerCapabilities,
    ) -> Arc<Self> {
        let humd_id = humd_id_from_node_id(&node_id);
        let peer = HumdAddr::new(humd_id).with_hint(format!("{IROH_HINT}{}", hex::encode(node_id.as_bytes())));
        let (tx, rx) = mpsc::channel::<Tone>(RECV_CAP);
        let me = Arc::new(Self {
            peer,
            node_id,
            caps,
            _conn: conn,
            sender: Mutex::new(Some(send)),
            rx: parking_lot::Mutex::new(Some(rx)),
        });
        tokio::spawn(read_loop(recv, tx));
        me
    }

    /// Dial a remote [`EndpointAddr`] over the supplied [`Endpoint`]
    /// and open a fresh bi-directional stream. Returns once the QUIC
    /// handshake + stream open completes (iroh requires us to write
    /// the first byte before the peer sees the stream — we prime with
    /// a newline that the reader treats as an empty NDJSON line).
    pub async fn connect(
        endpoint: &Endpoint,
        addr: EndpointAddr,
        caps: PeerCapabilities,
    ) -> Result<Arc<Self>> {
        let node_id = addr.id;
        let conn = endpoint
            .connect(addr, IROH_ALPN)
            .await
            .map_err(|e| anyhow!("iroh connect: {e}"))?;
        let (mut send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow!("iroh open_bi: {e}"))?;
        // iroh / QUIC streams are lazy: the peer doesn't see the stream
        // until the dialer writes. Push a no-op newline so accept_bi
        // resolves on the other side before our first real tone.
        send.write_all(b"\n")
            .await
            .map_err(|e| anyhow!("iroh prime stream: {e}"))?;
        Ok(Self::from_streams(node_id, conn, send, recv, caps))
    }

    /// The Ed25519 NodeId of the remote peer.
    pub fn node_id(&self) -> &EndpointId {
        &self.node_id
    }
}

#[async_trait]
impl PeerConnection for IrohEndpoint {
    fn peer(&self) -> &HumdAddr {
        &self.peer
    }
    fn capabilities(&self) -> &PeerCapabilities {
        &self.caps
    }

    async fn send(&self, tone: Tone) -> Result<()> {
        let mut line = serde_json::to_vec(&tone)
            .map_err(|e| anyhow!("iroh send: serialize: {e}"))?;
        line.push(b'\n');
        let mut guard = self.sender.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| anyhow!("iroh send: stream closed"))?;
        s.write_all(&line)
            .await
            .map_err(|e| anyhow!("iroh send: write: {e}"))?;
        Ok(())
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn close(&self) {
        // Drop the send half and the receiver. Best-effort, idempotent.
        // We can't await `finish` from a sync method, so spawn a task.
        let slot = match self.sender.try_lock() {
            Ok(mut g) => g.take(),
            // Contended — let the holder finish; their next write will
            // fail when the connection drops below.
            Err(_) => None,
        };
        if let Some(mut s) = slot {
            tokio::spawn(async move {
                let _ = s.finish();
            });
        }
        let _ = self.rx.lock().take();
    }
}

/// Reader loop — parses NDJSON lines off the iroh recv stream and
/// forwards them. Exits on EOF, read error, or when the receiver mpsc
/// is dropped.
async fn read_loop(recv: RecvStream, tx: mpsc::Sender<Tone>) {
    let mut lines = BufReader::new(recv).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break, // EOF
            Err(e) => {
                tracing::trace!(target: "ensemble.iroh", err = %e, "iroh.read.failed");
                break;
            }
        };
        if line.is_empty() {
            // Priming newline from `connect`, or just an empty frame —
            // skip without surfacing.
            continue;
        }
        let tone: Tone = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::trace!(target: "ensemble.iroh", err = %e, "iroh.parse.failed");
                continue;
            }
        };
        if tx.send(tone).await.is_err() {
            break;
        }
    }
}

// ── Transport impl ─────────────────────────────────────────────────────────

/// [`Transport`] over iroh QUIC. Holds a bound `iroh::Endpoint` and
/// dials out by NodeId. To accept inbound connections, callers drive
/// `endpoint.accept()` themselves and wrap each accepted connection
/// with [`IrohTransport::wrap_incoming`].
pub struct IrohTransport {
    endpoint: Endpoint,
}

impl IrohTransport {
    /// Wrap an already-built [`Endpoint`]. Caller is responsible for
    /// having configured the [`IROH_ALPN`] in the builder so the
    /// endpoint can both dial and accept on the hum protocol.
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    /// Convenience: bind a fresh [`Endpoint`] with sensible defaults
    /// for direct connections (no relay, no DNS lookup, loopback-only
    /// is fine). For T4 / federated deployments, build the endpoint
    /// yourself with the n0 preset and pass it to [`new`].
    pub async fn bind_direct() -> Result<Self> {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![IROH_ALPN.to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .map_err(|e| anyhow!("iroh bind: {e}"))?;
        Ok(Self::new(endpoint))
    }

    /// Bind an [`Endpoint`] with iroh's default public-relay mesh —
    /// enables NAT hole-punching for WAN T2-T4 peers. Use
    /// `bind_direct()` for loopback/LAN; this one for cross-internet
    /// peers behind NAT/firewalls. Requires DNS to reach iroh's
    /// hosted relays.
    pub async fn bind_relayed() -> Result<Self> {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![IROH_ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| anyhow!("iroh bind (relayed): {e}"))?;
        Ok(Self::new(endpoint))
    }

    /// Bind with an explicit relay URL — for self-hosted relay
    /// deployments (e.g. an org-run iroh-relay node). Use when n0's
    /// public mesh isn't trusted or reachable. Uses default QUIC
    /// address-discovery ports on the relay.
    pub async fn bind_with_relay(relay_url: iroh::RelayUrl) -> Result<Self> {
        let map = iroh::RelayMap::from(relay_url);
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![IROH_ALPN.to_vec()])
            .relay_mode(iroh::RelayMode::Custom(map))
            .bind()
            .await
            .map_err(|e| anyhow!("iroh bind (custom relay): {e}"))?;
        Ok(Self::new(endpoint))
    }

    /// The wrapped iroh endpoint. Useful for callers that need to
    /// `accept()`, inspect bound sockets, or share one endpoint between
    /// transport and other iroh-based protocols.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// This endpoint's NodeId (its Ed25519 public key). Pair with
    /// `bound_sockets()` to build the [`HumdAddr`] the peer needs to
    /// dial us back.
    pub fn node_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Accept the next inbound connection. Performs the QUIC handshake
    /// and waits for the dialer to open the bi-directional stream
    /// (they prime it with a newline in [`IrohEndpoint::connect`]).
    pub async fn accept(&self) -> Result<Arc<IrohEndpoint>> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow!("iroh accept: endpoint closed"))?;
        let conn = incoming
            .await
            .map_err(|e| anyhow!("iroh accept: handshake: {e}"))?;
        let node_id = conn.remote_id();
        let (send, mut recv) = conn
            .accept_bi()
            .await
            .map_err(|e| anyhow!("iroh accept: accept_bi: {e}"))?;
        // Eat the priming byte the dialer wrote; the read_loop will
        // pick up real frames from here on. Use a tiny buffer — we
        // only care about clearing any zero/newline bytes the dialer
        // sent to unblock accept_bi.
        let mut prime = [0u8; 1];
        let _ = recv.read(&mut prime).await;
        // Re-buffer: hand the now-primed `recv` to the same read_loop
        // wrapped in a BufReader inside `from_streams`. Note we
        // already consumed one byte (`\n`); subsequent bytes are real
        // NDJSON. The `BufReader::lines()` reader treats the empty
        // prefix correctly because the next real tone is a full JSON
        // line followed by `\n`.
        Ok(IrohEndpoint::from_streams(
            node_id,
            conn,
            send,
            recv,
            PeerCapabilities::default(),
        ))
    }
}

#[async_trait]
impl Transport for IrohTransport {
    async fn connect(&self, addr: &HumdAddr) -> Result<Arc<dyn PeerConnection>> {
        let hint = addr
            .hints
            .iter()
            .find_map(|h| h.strip_prefix(IROH_HINT))
            .ok_or_else(|| anyhow!("iroh transport: no `iroh:` hint in HumdAddr"))?;
        let node_id_bytes = hex::decode(hint)
            .with_context(|| format!("iroh transport: hint `{hint}` is not hex"))?;
        if node_id_bytes.len() != 32 {
            return Err(anyhow!("iroh transport: NodeId must be 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&node_id_bytes);
        let node_id = PublicKey::from_bytes(&arr)
            .map_err(|e| anyhow!("iroh transport: invalid NodeId: {e}"))?;
        // Direct-IP hints — needed when address lookup / relay aren't
        // configured (loopback tests, T1 LAN). Multiple hints stack.
        let direct_addrs: Vec<TransportAddr> = addr
            .hints
            .iter()
            .filter_map(|h| h.strip_prefix(IROH_IP_HINT))
            .filter_map(|s| s.parse().ok())
            .map(TransportAddr::Ip)
            .collect();
        let endpoint_addr = EndpointAddr::from_parts(node_id, direct_addrs);
        let endpoint = IrohEndpoint::connect(
            &self.endpoint,
            endpoint_addr,
            PeerCapabilities::default(),
        )
        .await?;
        Ok(endpoint as Arc<dyn PeerConnection>)
    }
}
