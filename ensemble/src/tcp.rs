//! Real-wire TCP transport for the ensemble.
//!
//! NDJSON over `tokio::net::TcpStream`. One framing rule: each tone is
//! a single JSON value followed by `\n`. Mirrors the unix-socket shape
//! thrumd uses (`thrumd/src/conn.rs`) — same parser, different socket.
//!
//! Auth lives one layer up: the ensemble's signed `chi:"hello"` exchange
//! authenticates the peer once a connection is installed. This module
//! only carries bytes.
//!
//! Shape:
//! - [`TcpEndpoint`] — one live [`PeerConnection`] backed by a TCP
//!   stream. Spawns a reader task that parses NDJSON into the receiver
//!   mpsc; `send` serialises + writes under a `tokio::Mutex` so
//!   concurrent senders can't interleave half-lines.
//! - [`TcpListener`] — accepts inbound connections. The accepted
//!   endpoint starts with a placeholder [`HumdAddr`]; the ensemble
//!   drainer learns the real id once the peer's first hello arrives.
//! - [`TcpTransport`] — [`Transport`] impl. `connect` dials the first
//!   `tcp:host:port` hint in the [`HumdAddr`].

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

use crate::{HumdAddr, HumdId, PeerCapabilities, PeerConnection, Tone, Transport};

/// Inbound-channel capacity — matches `InMemoryEndpoint`.
const RECV_CAP: usize = 256;

/// One TCP-backed peer link. Reader task drains NDJSON lines into the
/// receiver mpsc; `send` writes a serialised tone + newline under a
/// mutex (NDJSON framing requires atomic line writes).
pub struct TcpEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    /// Serialises concurrent writers. `Option` so `close` can drop the
    /// write half and subsequent sends fail cleanly.
    writer: Mutex<Option<OwnedWriteHalf>>,
    /// Inbound stream. Take-once.
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
}

impl TcpEndpoint {
    /// Dial a remote `host:port` and wrap the resulting stream.
    pub async fn connect(
        addr: &str,
        peer: HumdAddr,
        caps: PeerCapabilities,
    ) -> Result<Arc<Self>> {
        let stream = TcpStream::connect(addr).await?;
        // Disable Nagle: tones are small and latency-sensitive, and the
        // NDJSON framing means we never want a partial line stalling in
        // the kernel waiting for more bytes.
        let _ = stream.set_nodelay(true);
        Ok(Self::from_stream(stream, peer, caps))
    }

    /// Wrap an already-accepted stream. Used by `TcpListener::accept`.
    pub fn from_stream(
        stream: TcpStream,
        peer: HumdAddr,
        caps: PeerCapabilities,
    ) -> Arc<Self> {
        let _ = stream.set_nodelay(true);
        let (read_half, write_half) = stream.into_split();
        let (tx, rx) = mpsc::channel::<Tone>(RECV_CAP);
        let me = Arc::new(Self {
            peer,
            caps,
            writer: Mutex::new(Some(write_half)),
            rx: parking_lot::Mutex::new(Some(rx)),
        });
        tokio::spawn(read_loop(read_half, tx));
        me
    }
}

#[async_trait]
impl PeerConnection for TcpEndpoint {
    fn peer(&self) -> &HumdAddr { &self.peer }
    fn capabilities(&self) -> &PeerCapabilities { &self.caps }

    async fn send(&self, tone: Tone) -> Result<()> {
        let mut line = serde_json::to_vec(&tone)
            .map_err(|e| anyhow!("tcp send: serialize: {e}"))?;
        line.push(b'\n');
        let mut guard = self.writer.lock().await;
        let w = guard
            .as_mut()
            .ok_or_else(|| anyhow!("tcp send: writer closed"))?;
        w.write_all(&line)
            .await
            .map_err(|e| anyhow!("tcp send: write: {e}"))?;
        Ok(())
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn close(&self) {
        // Drop the writer half and the receiver. Best-effort, idempotent.
        // We can't await shutdown from a sync method, so spawn a task to
        // gracefully close the write half if it's still present.
        let writer_slot = {
            let mut guard = self.writer.try_lock();
            match guard {
                Ok(ref mut g) => g.take(),
                Err(_) => None, // contended — let the holder finish; their write will fail next
            }
        };
        if let Some(mut w) = writer_slot {
            tokio::spawn(async move {
                let _ = w.shutdown().await;
            });
        }
        let _ = self.rx.lock().take();
    }
}

/// Reader loop — parses NDJSON lines into `Tone`s and forwards. Exits on
/// EOF, read error, or when the receiver is dropped.
async fn read_loop(read_half: OwnedReadHalf, tx: mpsc::Sender<Tone>) {
    let mut lines = BufReader::new(read_half).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break, // EOF
            Err(e) => {
                tracing::trace!(target: "ensemble.tcp", err = %e, "tcp.read.failed");
                break;
            }
        };
        if line.is_empty() {
            continue;
        }
        let tone: Tone = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::trace!(target: "ensemble.tcp", err = %e, "tcp.parse.failed");
                continue;
            }
        };
        if tx.send(tone).await.is_err() {
            // Receiver dropped — endpoint closed.
            break;
        }
    }
}

// ── Listener ───────────────────────────────────────────────────────────────

/// Inbound TCP acceptor. Each `accept()` yields one new [`TcpEndpoint`]
/// with a placeholder peer id — the real id arrives in the first hello
/// and is owned by the ensemble drainer.
pub struct TcpListener {
    inner: tokio::net::TcpListener,
}

impl TcpListener {
    pub async fn bind(addr: &str) -> Result<Self> {
        let inner = tokio::net::TcpListener::bind(addr).await?;
        Ok(Self { inner })
    }

    /// Block on the next inbound connection. Returns a wrapped endpoint
    /// whose `HumdAddr` is a placeholder until the ensemble learns the
    /// peer's real id via its `chi:"hello"`.
    pub async fn accept(&self) -> Result<Arc<TcpEndpoint>> {
        let (stream, _remote) = self.inner.accept().await?;
        let placeholder = HumdAddr::new(HumdId::random());
        Ok(TcpEndpoint::from_stream(
            stream,
            placeholder,
            PeerCapabilities::default(),
        ))
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.inner.local_addr().map_err(Into::into)
    }
}

// ── Transport impl ─────────────────────────────────────────────────────────

/// [`Transport`] over TCP. `connect` reads the first `tcp:host:port`
/// hint off the addr and dials it.
pub struct TcpTransport;

impl TcpTransport {
    pub fn new() -> Self { Self }
}

impl Default for TcpTransport {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Transport for TcpTransport {
    async fn connect(&self, addr: &HumdAddr) -> Result<Arc<dyn PeerConnection>> {
        let hint = addr
            .hints
            .iter()
            .find_map(|h| h.strip_prefix("tcp:"))
            .ok_or_else(|| anyhow!("tcp transport: no tcp: hint in HumdAddr"))?;
        let endpoint = TcpEndpoint::connect(hint, addr.clone(), PeerCapabilities::default()).await?;
        Ok(endpoint as Arc<dyn PeerConnection>)
    }
}
