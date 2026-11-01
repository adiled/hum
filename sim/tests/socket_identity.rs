//! Socket hello identity binding.
//!
//! A bee nestles only by signing its hello with a key its hive entered.
//! These tests drive the Hello arm through the installed sink, so they
//! cover the decision humd actually makes, not a re-implementation of it.

use std::time::Duration;

use ed25519_dalek::Signer;
use hum_identity::BeeKey;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn fresh_bee() -> BeeKey {
    BeeKey::generate()
}

fn hello_with_identity(tone: &mut serde_json::Value, bee: &BeeKey) {
    let sig = bee.sign_socket_hello(now_ms());
    let (pubkey, signed_at, signature) = (sig.pubkey, sig.signed_at, sig.signature);
    let obj = tone.as_object_mut().expect("object");
    obj.insert("hid".into(), serde_json::json!(bee.hid.to_hex()));
    obj.insert("pubkey".into(), serde_json::json!(pubkey));
    obj.insert("signedAt".into(), serde_json::json!(signed_at));
    obj.insert("signature".into(), serde_json::json!(signature));
}

fn base_hello() -> serde_json::Value {
    serde_json::json!({
        "chi": "hello",
        "role": "nurse",
        "hive": "claude-cli",
        "version": "0.1.0",
        "protoVersion": thrum_core::THRUM_VERSION,
        "chis": ["hello", "prompt"],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_that_signs_its_hid_stays_connected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());

    let bee = fresh_bee();
    humd.bees.enter("claude-cli", thrum_core::Role::Nurse, &bee);
    let mut hello = base_hello();
    hello_with_identity(&mut hello, &bee);
    humd.thrum.inject_tone(&cid, hello).await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        humd.thrum.is_connected(&cid),
        "a correctly signed hello from an entered bee must be admitted"
    );
}

async fn signed_hello_survives(enter_under: Option<&str>, exit_first: bool) -> bool {
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");
    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());
    let bee = fresh_bee();
    if let Some(hive) = enter_under {
        humd.bees.enter(hive, thrum_core::Role::Nurse, &bee);
    }
    if exit_first {
        humd.bees.exit(&bee.id());
    }
    let mut hello = base_hello();
    hello_with_identity(&mut hello, &bee);
    humd.thrum.inject_tone(&cid, hello).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    humd.thrum.is_connected(&cid)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signed_bee_that_never_entered_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    assert!(!signed_hello_survives(None, false).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_entered_under_another_hive_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    assert!(
        !signed_hello_survives(Some("bash"), false).await,
        "a bee speaks for the hive it entered under, not the one it claims"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_that_exited_is_disconnected_at_its_next_hello() {
    let _ = tracing_subscriber::fmt::try_init();
    assert!(!signed_hello_survives(Some("claude-cli"), true).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entered_bee_that_does_not_sign_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());

    let bee = fresh_bee();
    humd.bees.enter("claude-cli", thrum_core::Role::Nurse, &bee);
    let mut hello = base_hello();
    hello["hid"] = serde_json::json!(bee.hid.to_hex());
    humd.thrum.inject_tone(&cid, hello).await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "claiming an entered hid without proving the key must not be enough"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_with_a_forged_signature_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());

    let bee = fresh_bee();
    humd.bees.enter("claude-cli", thrum_core::Role::Nurse, &bee);
    let mut hello = base_hello();
    hello_with_identity(&mut hello, &bee);
    hello["signature"] = serde_json::json!(hex::encode([0u8; 64]));

    humd.thrum.inject_tone(&cid, hello).await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "a hello whose signature does not verify must not stay connected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_claiming_anothers_hid_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());

    let mallory = fresh_bee();
    let victim = fresh_bee();
    humd.bees.enter("claude-cli", thrum_core::Role::Nurse, &victim);
    let signed_at = now_ms();
    let msg = hum_identity::socket_hello_message(&victim.hid, signed_at);
    let sig = mallory.signing.sign(&msg);

    let mut hello = base_hello();
    hello["hid"] = serde_json::json!(victim.hid.to_hex());
    hello["pubkey"] = serde_json::json!(hex::encode(mallory.pubkey_bytes()));
    hello["signedAt"] = serde_json::json!(signed_at);
    hello["signature"] = serde_json::json!(hex::encode(sig.to_bytes()));

    humd.thrum.inject_tone(&cid, hello).await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "a valid signature over a hid the key does not own must be refused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_hello_outside_the_skew_window_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());

    let bee = fresh_bee();
    humd.bees.enter("claude-cli", thrum_core::Role::Nurse, &bee);
    let stale = now_ms() - 3_600_000;
    let sig = bee.sign_socket_hello(stale);
    let (pubkey, signature) = (sig.pubkey, sig.signature);
    let mut hello = base_hello();
    hello["hid"] = serde_json::json!(bee.hid.to_hex());
    hello["pubkey"] = serde_json::json!(pubkey);
    hello["signedAt"] = serde_json::json!(stale);
    hello["signature"] = serde_json::json!(signature);

    humd.thrum.inject_tone(&cid, hello).await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "a hello recorded a hour ago must not be replayable now"
    );
}
