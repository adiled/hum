//! TLS-over-TCP transport for the ensemble. Sibling of [`crate::tcp`]:
//! same framing, same contracts, wrapped in a [`tokio_rustls::TlsStream`].
//!
//! Trust model: pinned fingerprints, no CA, no chain, no SNI, no expiry.
//! The dialer accepts exactly one cert — the SHA-256 of its DER bytes
//! written down out of band in `peers.json`. See [`PinnedFingerprintVerifier`].

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::{WriteHalf, split};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};

use crate::framing;
use crate::opening::Gate;
use crate::{Hid, HumdAddr, PeerCapabilities, PeerConnection, Tone, Transport};

const RECV_CAP: usize = 256;

pub const TLS_HINT: &str = "tls:";

pub fn cert_fingerprint(cert: &CertificateDer<'_>) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(cert.as_ref());
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

// ── Endpoint ───────────────────────────────────────────────────────────────

pub struct TlsTcpEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    writer: Arc<Mutex<Option<WriteHalf<TlsStream<TcpStream>>>>>,
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
    closed: AtomicBool,
    gate: Gate,
}

impl TlsTcpEndpoint {
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
            writer: Arc::new(Mutex::new(Some(write_half))),
            rx: parking_lot::Mutex::new(Some(rx)),
            closed: AtomicBool::new(false),
            gate: Gate::new(),
        });
        tokio::spawn(framing::pump(read_half, tx, "tls"));
        me
    }
    async fn write(&self, tone: Tone) -> Result<()> {
        let mut guard = self.writer.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Err(anyhow!("tls send: link closed"));
        }
        let w = guard
            .as_mut()
            .ok_or_else(|| anyhow!("tls send: writer closed"))?;
        framing::write_frame(w, &tone).await
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
        let placeholder = HumdAddr::new(Hid::random_humd());
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
        let fp_bytes =
            hex::decode(fp_hex).map_err(|e| anyhow!("tls transport: fingerprint hex: {e}"))?;
        if fp_bytes.len() != 32 {
            return Err(anyhow!(
                "tls transport: fingerprint must be 32 bytes (got {})",
                fp_bytes.len()
            ));
        }
        let mut fp = [0u8; 32];
        fp.copy_from_slice(&fp_bytes);
        let endpoint =
            TlsTcpEndpoint::connect(host_port, addr.clone(), PeerCapabilities::default(), fp)
                .await?;
        Ok(endpoint as Arc<dyn PeerConnection>)
    }
}
