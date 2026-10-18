//! Revocation at the socket.
//!
//! A revoked hid is refused the socket, not merely hidden from discovery.
//! The interesting case is the one where revocation would be easy to
//! evade: a bee that drops its signature to slip past the check.

use std::time::Duration;

use ed25519_dalek::SigningKey;
use hum_identity::{BeeKey, Hid, HidPrefix};

fn fresh_bee() -> BeeKey {
    let signing = SigningKey::generate(&mut rand::thread_rng());
    let hid = Hid::from_pubkey(HidPrefix::Wbee, &signing.verifying_key().to_bytes());
    BeeKey { signing, hid }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hello_with_identity(bee: &BeeKey) -> serde_json::Value {
    let (pubkey, signed_at, signature) = bee.sign_socket_hello(now_ms());
    serde_json::json!({
        "chi": "hello",
        "bee": ["worker"],
        "hive": "claude-cli",
        "version": "0.1.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "chis": ["hello", "prompt"],
        "hid": bee.hid.to_hex(),
        "pubkey": pubkey,
        "signedAt": signed_at,
        "signature": signature,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_bee_is_refused_the_socket() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();

    let bee = fresh_bee();
    let id = ensemble::Hid::random_humd();
    let humd = sim.spawn_humd_revoking(id, &[&bee.hid.to_hex()]).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());
    humd.thrum
        .inject_tone(&cid, hello_with_identity(&bee))
        .await;

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "a correctly signed hello from a revoked bee must still be refused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_signature_does_not_evade_revocation() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();

    let bee = fresh_bee();
    let id = ensemble::Hid::random_humd();
    let humd = sim.spawn_humd_revoking(id, &[&bee.hid.to_hex()]).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());

    let unsigned = serde_json::json!({
        "chi": "hello",
        "bee": ["worker"],
        "hive": "claude-cli",
        "version": "0.1.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "hid": bee.hid.to_hex(),
    });
    humd.thrum.inject_tone(&cid, unsigned).await;

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "an unsigned hello claiming a revoked hid must be refused too"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unrevoked_bee_is_unaffected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();

    let bee = fresh_bee();
    let other = fresh_bee();
    let id = ensemble::Hid::random_humd();
    let humd = sim.spawn_humd_revoking(id, &[&other.hid.to_hex()]).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());
    humd.thrum
        .inject_tone(&cid, hello_with_identity(&bee))
        .await;

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        humd.thrum.is_connected(&cid),
        "a bee absent from the revocation list must be admitted"
    );
}
