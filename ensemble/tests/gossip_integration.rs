//! End-to-end test of the gossip pub-sub layer.
//!
//! Wire three humds A↔B↔C in a line (no direct A↔C link). Subscribe C
//! to a topic; publish from A. The tone must percolate through B (one
//! gossip hop) and reach C's per-topic broadcast receiver. Then send
//! a tone with a colliding `msg_id` and assert C does NOT see a
//! duplicate — proving the seen-set dedupes across the mesh.
//!
//! Uses `InMemoryEndpoint::pair` for both wires (A↔B and B↔C); B holds
//! two ensemble peers (one per side), so when the gossip arrives from
//! A its drainer re-fans to "every OTHER peer," which is exactly C.

use std::time::Duration;

use ensemble::{
    gossip::{gossip_tone, mint_msg_id, GOSSIP_CHI},
    Ensemble, HumdKey, InMemoryEndpoint, PeerCapabilities,
};
use serde_json::json;
use tokio::time::timeout;

#[tokio::test]
async fn gossip_percolates_one_hop_and_dedupes_duplicates() {
    // Three humds with real keypairs so the signed handshake passes
    // and learned_caps populates (not strictly required for gossip,
    // but exercises the production install() path).
    let a_key = HumdKey::generate();
    let b_key = HumdKey::generate();
    let c_key = HumdKey::generate();
    let a_id = a_key.hid();
    let b_id = b_key.hid();
    let c_id = c_key.hid();

    let caps = PeerCapabilities {
        proto_version: "0.6.0".into(),
        ..Default::default()
    };

    // Wire A↔B.
    let (a_to_b, b_to_a) = InMemoryEndpoint::pair(
        a_id, caps.clone(), // a's transport-view of b
        b_id, caps.clone(), // b's transport-view of a
    );
    // Wire B↔C.
    let (b_to_c, c_to_b) = InMemoryEndpoint::pair(
        b_id, caps.clone(),
        c_id, caps.clone(),
    );

    let ens_a = Ensemble::new(a_id);
    let ens_b = Ensemble::new(b_id);
    let ens_c = Ensemble::new(c_id);

    // A installs its link to B; B installs both (one to A, one to C);
    // C installs its link to B. B is the middle hop.
    ens_a.install(a_to_b, caps.clone(), &a_key);
    ens_b.install(b_to_a, caps.clone(), &b_key);
    ens_b.install(b_to_c, caps.clone(), &b_key);
    ens_c.install(c_to_b, caps.clone(), &c_key);

    // C subscribes BEFORE A publishes — broadcast receivers only catch
    // messages sent after they subscribe.
    let mut sub_c = ens_c.subscribe_topic("test-topic");

    // Give the drainers a beat to chew through the handshake hellos
    // so the gossip tone isn't interleaved with first-hello absorption.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // A publishes. The tone travels A → B → C (one gossip hop).
    let payload = json!({"event": "hum-relocated", "hum": "atlas", "to_humd": "humd-z"});
    ens_a.publish("test-topic", payload.clone()).await;

    // C must see the payload within 200ms.
    let got = timeout(Duration::from_millis(200), sub_c.recv())
        .await
        .expect("C did not receive gossip within 200ms")
        .expect("C's topic channel closed");
    assert_eq!(got, payload, "C received the wrong payload");

    // Dedup check: spin a fresh "X↔B" peer so we can inject a tone
    // with a controlled msg_id, send it TWICE. B's seen-set captures
    // it on first arrival, drops the second copy at the seen check.
    // C should observe the payload exactly once.
    let x_key = HumdKey::generate();
    let x_id = x_key.hid();
    let (x_to_b, b_to_x) = InMemoryEndpoint::pair(
        x_id, caps.clone(),
        b_id, caps.clone(),
    );
    ens_b.install(b_to_x, caps.clone(), &b_key);
    // Give X's hello + B's hello a beat to clear before injecting raw
    // gossip on the X side. X is a "naked" peer — no Ensemble — so it
    // can write whatever it wants directly through its endpoint.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Drain any leftover broadcasts on C before the dedup canary.
    while let Ok(Ok(_)) = timeout(Duration::from_millis(1), sub_c.recv()).await {}

    let canary_payload = json!({"event": "dedup-canary"});
    let canary_msg_id = mint_msg_id("test-topic", "rid-canary", &x_id, &canary_payload);
    let canary_tone = gossip_tone(
        "test-topic",
        "rid-canary",
        &x_id,
        canary_payload.clone(),
        &canary_msg_id,
    );
    assert_eq!(canary_tone.get("chi").unwrap(), GOSSIP_CHI);

    // First copy: should reach C via B.
    x_to_b
        .send(canary_tone.clone())
        .await
        .expect("send first canary copy");
    let got2 = timeout(Duration::from_millis(200), sub_c.recv())
        .await
        .expect("C did not receive first canary within 200ms")
        .expect("C's topic channel closed");
    assert_eq!(got2, canary_payload);

    // Second copy — identical msg_id. B's seen-set has it; should be
    // dropped at B and never reach C.
    x_to_b
        .send(canary_tone)
        .await
        .expect("send second canary copy");
    let dup = timeout(Duration::from_millis(200), sub_c.recv()).await;
    assert!(
        dup.is_err(),
        "C received a duplicate gossip payload (msg_id dedup failed): {:?}",
        dup
    );
}
