//! NDJSON over TCP. Framing and the frame ceiling live in [`crate::framing`].
//! Identity is settled one layer up by the signed `chi:"hello"` exchange.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, mpsc};

use crate::framing;
use crate::opening::Gate;
use crate::{Hid, HumdAddr, PeerCapabilities, PeerConnection, Tone, Transport};

const RECV_CAP: usize = 256;

pub struct TcpEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    writer: Arc<Mutex<Option<OwnedWriteHalf>>>,
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
    closed: AtomicBool,
    gate: Gate,
}

impl TcpEndpoint {
    pub async fn connect(addr: &str, peer: HumdAddr, caps: PeerCapabilities) -> Result<Arc<Self>> {
        let stream = TcpStream::connect(addr).await?;
        let _ = stream.set_nodelay(true);
        Ok(Self::from_stream(stream, peer, caps))
    }

    pub fn from_stream(stream: TcpStream, peer: HumdAddr, caps: PeerCapabilities) -> Arc<Self> {
        let _ = stream.set_nodelay(true);
        let (read_half, write_half) = stream.into_split();
        let (tx, rx) = mpsc::channel::<Tone>(RECV_CAP);
        let me = Arc::new(Self {
            peer,
            caps,
            writer: Arc::new(Mutex::new(Some(write_half))),
            rx: parking_lot::Mutex::new(Some(rx)),
            closed: AtomicBool::new(false),
            gate: Gate::new(),
        });
        tokio::spawn(framing::pump(read_half, tx, "tcp"));
        me
    }
    async fn write(&self, tone: Tone) -> Result<()> {
        let mut guard = self.writer.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Err(anyhow!("tcp send: link closed"));
        }
        let w = guard
            .as_mut()
            .ok_or_else(|| anyhow!("tcp send: writer closed"))?;
        framing::write_frame(w, &tone).await
    }
}

#[async_trait]
impl PeerConnection for TcpEndpoint {
    fn peer(&self) -> &HumdAddr {
        &self.peer
    }
    fn capabilities(&self) -> &PeerCapabilities {
        &self.caps
    }

    async fn send(&self, tone: Tone) -> Result<()> {
        self.gate.wait().await;
        self.write(tone).await
    }

    async fn send_opening(&self, tone: Tone) -> Result<()> {
        let result = self.write(tone).await;
        self.gate.opened();
        result
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn arm_opening(&self) {
        self.gate.arm();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.gate.shut();
        let writer = self.writer.clone();
        tokio::spawn(async move {
            if let Some(mut w) = writer.lock().await.take() {
                let _ = tokio::io::AsyncWriteExt::shutdown(&mut w).await;
            }
        });
        let _ = self.rx.lock().take();
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
        let placeholder = HumdAddr::new(Hid::random_humd());
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
    pub fn new() -> Self {
        Self
    }
}

impl Default for TcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for TcpTransport {
    async fn connect(&self, addr: &HumdAddr) -> Result<Arc<dyn PeerConnection>> {
        let hint = addr
            .hints
            .iter()
            .find_map(|h| h.strip_prefix("tcp:"))
            .ok_or_else(|| anyhow!("tcp transport: no tcp: hint in HumdAddr"))?;
        let endpoint =
            TcpEndpoint::connect(hint, addr.clone(), PeerCapabilities::default()).await?;
        Ok(endpoint as Arc<dyn PeerConnection>)
    }
}
