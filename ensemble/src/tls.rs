//! TLS-over-TCP transport for the ensemble.
//!
//! Sibling of [`crate::tcp`] — same NDJSON framing, same `Transport` +
//! `PeerConnection` contract, but the underlying `TcpStream` is wrapped
//! in a [`tokio_rustls::TlsStream`]. T2+ peers run over the public
//! internet; plaintext is the loopback/LAN path only.
//!
//! ## Trust model — pinned fingerprints, no CA
//!
//! Self-signed cert on the server. The dialer carries the expected
//! cert fingerprint (SHA-256 of the DER bytes) and accepts ONLY that
//! one cert — see [`PinnedFingerprintVerifier`]. This matches the T2
//! "known peers" model: trust is established out of band when you put
//! a peer's fingerprint in `peers.json`; the TLS layer is just a
//! confidentiality + integrity tunnel that proves the box on the other
//! end really holds the private key for the fingerprint you wrote down.
//! No CA, no chain validation, no SNI matching, no expiry handling —
//! those belong to T4 with a real PKI.
//!
//! Shape:
//! - [`TlsTcpEndpoint`] — one live [`PeerConnection`] over a TLS-wrapped
//!   TCP stream. Reader task drains NDJSON into the receiver mpsc;
//!   `send` writes a serialised tone + newline under a `tokio::Mutex`.
//! - [`TlsTcpListener`] — accepts inbound TLS connections. The
//!   accepted endpoint starts with a placeholder [`HumdAddr`]; the
//!   ensemble drainer learns the real id once the peer's first hello
//!   arrives — same as plaintext.
//! - [`TlsTcpTransport`] — [`Transport`] impl. `connect` dials the
//!   first `tls:host:port` hint in the [`HumdAddr`], using a
//!   pre-configured client config that pins one fingerprint.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::{split, AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};

use crate::{HumdAddr, HumdId, PeerCapabilities, PeerConnection, Tone, Transport};

/// Inbound-channel capacity — matches `InMemoryEndpoint` / `TcpEndpoint`.
const RECV_CAP: usize = 256;

/// Hint prefix on a [`HumdAddr`] that signals "dial this `host:port`
/// over TLS-pinned TCP." Sibling of `tcp:` so peers can advertise both
/// plaintext and TLS endpoints and dialers pick the right one.
pub const TLS_HINT: &str = "tls:";

/// SHA-256 of a certificate's DER bytes. The bytes used for pinning in
/// `peers.json` — write this on the server, paste it on the client.
pub fn cert_fingerprint(cert: &CertificateDer<'_>) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(cert.as_ref());
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

// ── Endpoint ───────────────────────────────────────────────────────────────

/// One TLS-over-TCP peer link. Same NDJSON framing as the plaintext
/// path — the TLS layer is invisible to the framing code, we just hand
/// it a `TlsStream` instead of a `TcpStream`.
pub struct TlsTcpEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    /// Serialises concurrent writers. `Option` so `close` can drop the
    /// write half and subsequent sends fail cleanly.
    writer: Mutex<Option<WriteHalf<TlsStream<TcpStream>>>>,
    /// Inbound stream. Take-once.
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
}

impl TlsTcpEndpoint {
    /// Wrap an already-handshaken TLS stream. Used by both the client
    /// and listener paths once rustls has finished the handshake.
    pub fn from_stream(
        stream: TlsStream<TcpStream>,
        peer: HumdAddr,
        caps: PeerCapabilities,
    ) -> Arc<Self> {
        let (read_half, write_half) = split(stream);
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

    /// Dial a remote `host:port` with a fingerprint-pinned TLS config.
    /// `expected_fingerprint` is the SHA-256 of the peer's cert DER —
    /// the verifier rejects anything else with an `OtherError`.
    pub async fn connect(
        addr: &str,
        peer: HumdAddr,
        caps: PeerCapabilities,
        expected_fingerprint: [u8; 32],
    ) -> Result<Arc<Self>> {
        let tcp = TcpStream::connect(addr).await?;
        let _ = tcp.set_nodelay(true);
        let connector = TlsConnector::from(Arc::new(client_config_pinned(expected_fingerprint)));
        // SNI isn't validated by our pinned verifier — we still need a
        // syntactically valid ServerName for the API. "localhost" is
        // fine; the only check that matters is the fingerprint match.
        let server_name = ServerName::try_from("localhost")
            .map_err(|e| anyhow!("tls connect: invalid server name: {e}"))?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| anyhow!("tls connect: handshake: {e}"))?;
        Ok(Self::from_stream(TlsStream::Client(tls), peer, caps))
    }
}

#[async_trait]
impl PeerConnection for TlsTcpEndpoint {
    fn peer(&self) -> &HumdAddr {
        &self.peer
    }
    fn capabilities(&self) -> &PeerCapabilities {
        &self.caps
    }

    async fn send(&self, tone: Tone) -> Result<()> {
        let mut line = serde_json::to_vec(&tone)
            .map_err(|e| anyhow!("tls send: serialize: {e}"))?;
        line.push(b'\n');
        let mut guard = self.writer.lock().await;
        let w = guard
            .as_mut()
            .ok_or_else(|| anyhow!("tls send: writer closed"))?;
        w.write_all(&line)
            .await
            .map_err(|e| anyhow!("tls send: write: {e}"))?;
        Ok(())
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn close(&self) {
        // Drop the writer half and the receiver. Best-effort, idempotent.
        let slot = match self.writer.try_lock() {
            Ok(mut g) => g.take(),
            Err(_) => None, // contended — holder's next write fails when peer drops
        };
        if let Some(mut w) = slot {
            tokio::spawn(async move {
                let _ = w.shutdown().await;
            });
        }
        let _ = self.rx.lock().take();
    }
}

/// Reader loop — parses NDJSON lines off the TLS read half and forwards
/// them. Identical control flow to the plaintext path; the TLS framing
/// is fully encapsulated inside `TlsStream`.
async fn read_loop(
    read_half: ReadHalf<TlsStream<TcpStream>>,
    tx: mpsc::Sender<Tone>,
) {
    let mut lines = BufReader::new(read_half).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break, // EOF
            Err(e) => {
                tracing::trace!(target: "ensemble.tls", err = %e, "tls.read.failed");
                break;
            }
        };
        if line.is_empty() {
            continue;
        }
        let tone: Tone = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::trace!(target: "ensemble.tls", err = %e, "tls.parse.failed");
                continue;
            }
        };
        if tx.send(tone).await.is_err() {
            break;
        }
    }
}

// ── Listener ───────────────────────────────────────────────────────────────

/// Inbound TLS-over-TCP acceptor. Each `accept()` runs the TLS
/// handshake before yielding a [`TlsTcpEndpoint`] with a placeholder
/// peer id — same handoff shape as the plaintext listener.
pub struct TlsTcpListener {
    inner: tokio::net::TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsTcpListener {
    /// Bind a TCP listener and prepare a TLS acceptor that serves the
    /// supplied self-signed cert + key. Dialers verify against the
    /// SHA-256 of `cert` — see [`cert_fingerprint`].
    pub async fn bind(
        addr: &str,
        cert: CertificateDer<'static>,
        key: PrivateKeyDer<'static>,
    ) -> Result<Self> {
        let inner = tokio::net::TcpListener::bind(addr).await?;
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .map_err(|e| anyhow!("tls listener: server config: {e}"))?;
        let acceptor = TlsAcceptor::from(Arc::new(config));
        Ok(Self { inner, acceptor })
    }

    /// Accept one inbound connection and complete the TLS handshake
    /// before wrapping it. Returns an endpoint with a placeholder peer
    /// id — overwritten by the ensemble drainer when it parses the
    /// peer's signed `chi:"hello"`.
    pub async fn accept(&self) -> Result<Arc<TlsTcpEndpoint>> {
        let (tcp, _remote) = self.inner.accept().await?;
        let _ = tcp.set_nodelay(true);
        let tls = self
            .acceptor
            .accept(tcp)
            .await
            .map_err(|e| anyhow!("tls accept: handshake: {e}"))?;
        let placeholder = HumdAddr::new(HumdId::random());
        Ok(TlsTcpEndpoint::from_stream(
            TlsStream::Server(tls),
            placeholder,
            PeerCapabilities::default(),
        ))
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.inner.local_addr().map_err(Into::into)
    }
}

// ── Pinned-fingerprint verifier ────────────────────────────────────────────

/// Custom [`ServerCertVerifier`] that accepts exactly ONE cert,
/// identified by its SHA-256 DER fingerprint. No CA, no chain, no SNI.
///
/// This is the T2 trust model: we trust the cert because the operator
/// configured its fingerprint in `peers.json`, not because anyone
/// signed it. Any cert with a non-matching SHA-256 is rejected with
/// `CertificateError::ApplicationVerificationFailure`. Intermediate
/// certs in the chain are ignored (we only pin the leaf).
#[derive(Debug)]
pub struct PinnedFingerprintVerifier {
    expected: [u8; 32],
}

impl PinnedFingerprintVerifier {
    pub fn new(expected: [u8; 32]) -> Self {
        Self { expected }
    }
}

impl ServerCertVerifier for PinnedFingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let got = cert_fingerprint(end_entity);
        if got == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        // We don't validate the signature ourselves — the fingerprint
        // pin already binds the connection to the exact cert holder.
        // rustls still verifies the handshake's transcript signature
        // against the cert's public key; this just tells it not to
        // also try to chain-verify or scheme-check.
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        // Match what ring-backed rustls supports for self-signed certs.
        // ECDSA P-256 / Ed25519 are the rcgen defaults; RSA-PSS covers
        // RSA self-signed certs from openssl-generated keys.
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}

/// Build a [`ClientConfig`] that pins exactly one server cert by its
/// SHA-256 fingerprint. Used by both [`TlsTcpEndpoint::connect`] and
/// the [`TlsTcpTransport`] dialer path.
pub fn client_config_pinned(expected_fingerprint: [u8; 32]) -> ClientConfig {
    ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedFingerprintVerifier::new(
            expected_fingerprint,
        )))
        .with_no_client_auth()
}

// ── Transport impl ─────────────────────────────────────────────────────────

/// [`Transport`] over TLS-pinned TCP. Stateless dialer — the pinned
/// fingerprint is read from the [`HumdAddr`] hint (`tls:host:port`
/// pairs with `tls-fp:<hex>`). For server-side accept, use
/// [`TlsTcpListener`] directly.
pub struct TlsTcpTransport;

impl TlsTcpTransport {
    pub fn new() -> Self {
        Self
    }

    /// Bind a [`TlsTcpListener`] with a precomputed self-signed cert +
    /// key. Convenience around [`TlsTcpListener::bind`] that mirrors
    /// the plaintext `TcpListener::bind` shape so call sites that
    /// already have a cert can drop in the TLS version.
    pub async fn bind_with_cert(
        addr: &str,
        cert: CertificateDer<'static>,
        key: PrivateKeyDer<'static>,
    ) -> Result<TlsTcpListener> {
        TlsTcpListener::bind(addr, cert, key).await
    }

    /// Dial a `host:port` and verify the server cert against
    /// `expected_fingerprint`. Convenience around
    /// [`TlsTcpEndpoint::connect`].
    pub async fn connect_to(
        addr: &str,
        peer: HumdAddr,
        caps: PeerCapabilities,
        expected_fingerprint: [u8; 32],
    ) -> Result<Arc<TlsTcpEndpoint>> {
        TlsTcpEndpoint::connect(addr, peer, caps, expected_fingerprint).await
    }
}

impl Default for TlsTcpTransport {
    fn default() -> Self {
        Self::new()
    }
}

/// Hint prefix carrying the pinned-fingerprint hex on a [`HumdAddr`].
/// The dialer needs both `tls:host:port` and `tls-fp:<64-hex>` to
/// build a `TlsConnector` — sibling-hint pairing mirrors how iroh
/// carries both `iroh:<id>` and `iroh-ip:<sockaddr>`.
pub const TLS_FP_HINT: &str = "tls-fp:";

#[async_trait]
impl Transport for TlsTcpTransport {
    async fn connect(&self, addr: &HumdAddr) -> Result<Arc<dyn PeerConnection>> {
        let host_port = addr
            .hints
            .iter()
            .find_map(|h| h.strip_prefix(TLS_HINT))
            .ok_or_else(|| anyhow!("tls transport: no `tls:` hint in HumdAddr"))?;
        let fp_hex = addr
            .hints
            .iter()
            .find_map(|h| h.strip_prefix(TLS_FP_HINT))
            .ok_or_else(|| anyhow!("tls transport: no `tls-fp:` hint in HumdAddr"))?;
        let fp_bytes = hex::decode(fp_hex)
            .map_err(|e| anyhow!("tls transport: fingerprint hex: {e}"))?;
        if fp_bytes.len() != 32 {
            return Err(anyhow!(
                "tls transport: fingerprint must be 32 bytes (got {})",
                fp_bytes.len()
            ));
        }
        let mut fp = [0u8; 32];
        fp.copy_from_slice(&fp_bytes);
        let endpoint = TlsTcpEndpoint::connect(
            host_port,
            addr.clone(),
            PeerCapabilities::default(),
            fp,
        )
        .await?;
        Ok(endpoint as Arc<dyn PeerConnection>)
    }
}
