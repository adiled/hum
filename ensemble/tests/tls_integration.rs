//! End-to-end test of the TLS-pinned TCP transport.
//!
//! Two paths:
//! 1. Happy path — generate a self-signed cert with rcgen, bind a TLS
//!    listener with it, dial with the matching SHA-256 fingerprint,
//!    send a tone across, verify it arrives.
//! 2. Wrong-fingerprint path — dial the same listener with a tampered
//!    fingerprint, verify the TLS handshake fails before any tone
//!    flows. This is the heart of the T2 trust model: the dialer's
//!    pinned verifier rejects any cert it doesn't already know.

use std::sync::Arc;
use std::time::Duration;

use ensemble::{
    cert_fingerprint, Ensemble, HumdAddr, HumdKey, PeerCapabilities, TlsTcpEndpoint,
    TlsTcpListener,
};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::json;

/// Build a fresh self-signed cert with rcgen. Returns the cert + key as
/// the rustls types the listener wants, plus the SHA-256 fingerprint
/// the dialer needs to pin.
fn fresh_self_signed() -> (CertificateDer<'static>, PrivateKeyDer<'static>, [u8; 32]) {
    // rcgen 0.13: generate_simple_self_signed returns a CertifiedKey
    // with `.cert: rcgen::Certificate` and `.key_pair: KeyPair`. The
    // key is serialised to PKCS#8 DER for rustls.
    let subject_alt_names = vec!["localhost".to_string()];
    let ck = generate_simple_self_signed(subject_alt_names).expect("rcgen self-signed");
    let cert_der: CertificateDer<'static> = ck.cert.into();
    let key_der: PrivateKeyDer<'static> =
        PrivatePkcs8KeyDer::from(ck.key_pair.serialize_der()).into();
    let fp = cert_fingerprint(&cert_der);
    (cert_der, key_der, fp)
}

/// Install the default ring-based crypto provider once per test process.
/// rustls 0.23 refuses to build a `ClientConfig` / `ServerConfig` until
/// a provider is selected; we use `ring` (matches the feature flags in
/// the crate's Cargo.toml).
fn install_crypto_provider() {
    // Safe to call repeatedly across tests — `install_default` returns
    // an Err once the slot is filled, which we swallow.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[tokio::test]
async fn tls_endpoint_routes_tone_across_pinned_wire() {
    install_crypto_provider();

    // Random keypairs for the two humds.
    let a_key = HumdKey::generate();
    let b_key = HumdKey::generate();
    let a_id = a_key.hid();
    let b_id = b_key.hid();

    // Generate B's self-signed cert. A pins this fingerprint.
    let (cert, key, fingerprint) = fresh_self_signed();

    // TLS listener on the loopback, OS-assigned port, with B's cert.
    let listener = TlsTcpListener::bind("127.0.0.1:0", cert, key)
        .await
        .expect("bind tls listener");
    let local = listener.local_addr().expect("local_addr");

    // Accept on a background task — dial happens concurrently.
    let accept_task = tokio::spawn(async move { listener.accept().await });

    // Outbound: A dials the listener, pinning B's fingerprint.
    let a_endpoint = TlsTcpEndpoint::connect(
        &local.to_string(),
        HumdAddr::new(b_id),
        PeerCapabilities::default(),
        fingerprint,
    )
    .await
    .expect("tls connect");

    let b_endpoint = accept_task.await.expect("join").expect("accept");

    // Two ensembles, lax auth so plain `install` works.
    let ensemble_a = Ensemble::with_strict_auth(a_id, false);
    let ensemble_b = Ensemble::with_strict_auth(b_id, false);

    ensemble_a.install(a_endpoint.clone(), PeerCapabilities::default(), &a_key);
    ensemble_b.install(b_endpoint.clone(), PeerCapabilities::default(), &b_key);

    // Subscribe on B before traffic flows so the broadcast captures it.
    let mut sub_b = ensemble_b.subscribe();

    // Route a ping from A → B. First tone is A's hello (drained by B);
    // second is this ping which fans out to subscribers.
    let ping = json!({"chi": "ping", "rid": "tls-1", "to": b_id.to_hex()});
    ensemble_a.route(ping).await.expect("route ping");

    let got = tokio::time::timeout(Duration::from_secs(2), sub_b.recv())
        .await
        .expect("recv timed out")
        .expect("subscribe closed");
    assert_eq!(got.get("chi").and_then(|v| v.as_str()), Some("ping"));
    assert_eq!(got.get("rid").and_then(|v| v.as_str()), Some("tls-1"));
}

#[tokio::test]
async fn tls_connect_rejects_wrong_fingerprint() {
    install_crypto_provider();

    let (cert, key, real_fingerprint) = fresh_self_signed();

    let listener = TlsTcpListener::bind("127.0.0.1:0", cert, key)
        .await
        .expect("bind tls listener");
    let local = listener.local_addr().expect("local_addr");

    // Accept task — we expect either the handshake to fail server-side
    // when the client aborts, or the client-side connect to fail first.
    // Either way, the assertion below is that the *client* sees a
    // handshake error, since the pinned verifier rejects the cert.
    let _accept_task = tokio::spawn(async move {
        // Best-effort: may complete with an error once the client tears
        // down. We don't assert on this side; the client-side rejection
        // is the contract we care about.
        let _ = listener.accept().await;
    });

    // Tamper with the fingerprint — flip a bit so the SHA-256 check fails.
    let mut wrong = real_fingerprint;
    wrong[0] ^= 0xFF;

    let b_id = HumdKey::generate().hid();
    let result: anyhow::Result<Arc<TlsTcpEndpoint>> = TlsTcpEndpoint::connect(
        &local.to_string(),
        HumdAddr::new(b_id),
        PeerCapabilities::default(),
        wrong,
    )
    .await;

    let err = match result {
        Ok(_) => panic!("tls connect must reject a wrong fingerprint, got Ok"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("handshake") || err.to_lowercase().contains("certificate"),
        "expected handshake/cert error, got: {err}"
    );
}
