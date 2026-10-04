use std::time::Duration;

use ensemble::{
    Ensemble, HumdKey, InMemoryEndpoint, PeerCapabilities,
    gossip::{GOSSIP_CHI, gossip_tone, mint_msg_id},
};
use serde_json::json;
use tokio::time::timeout;

#[tokio::test]
async fn gossip_percolates_one_hop_and_dedupes_duplicates() {
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

    let (a_to_b, b_to_a) = InMemoryEndpoint::pair(a_id, caps.clone(), b_id, caps.clone());
    let (b_to_c, c_to_b) = InMemoryEndpoint::pair(b_id, caps.clone(), c_id, caps.clone());

    let ens_a = Ensemble::new(a_id);
    let ens_b = Ensemble::new(b_id);
    let ens_c = Ensemble::new(c_id);

    ens_a.install(a_to_b, caps.clone(), &a_key);
    ens_b.install(b_to_a, caps.clone(), &b_key);
    ens_b.install(b_to_c, caps.clone(), &b_key);
    ens_c.install(c_to_b, caps.clone(), &c_key);

    let mut sub_c = ens_c.subscribe_topic("test-topic");

    tokio::time::sleep(Duration::from_millis(20)).await;

    let payload = json!({"event": "hum-relocated", "hum": "atlas", "to_humd": "humd-z"});
    ens_a.publish("test-topic", payload.clone()).await;

    let got = timeout(Duration::from_millis(200), sub_c.recv())
        .await
        .expect("C did not receive gossip within 200ms")
        .expect("C's topic channel closed");
    assert_eq!(got, payload, "C received the wrong payload");

    let x_key = HumdKey::generate();
    let x_id = x_key.hid();
    let (x_to_b, b_to_x) = InMemoryEndpoint::pair(x_id, caps.clone(), b_id, caps.clone());
    ens_b.install(b_to_x, caps.clone(), &b_key);
    let ens_x = Ensemble::new(x_id);
    ens_x.install(x_to_b.clone(), caps.clone(), &x_key);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(ens_b.handshake_done(&x_id), "B must complete X's handshake");

    while let Ok(Ok(_)) = timeout(Duration::from_millis(1), sub_c.recv()).await {}

    let canary_payload = json!({"event": "dedup-canary"});
    let canary_msg_id = mint_msg_id(&x_id);
    let canary_tone = gossip_tone(
        "test-topic",
        "rid-canary",
        &x_id,
        canary_payload.clone(),
        &canary_msg_id,
    );
    assert_eq!(canary_tone.get("chi").unwrap(), GOSSIP_CHI);

    x_to_b
        .send(canary_tone.clone())
        .await
        .expect("send first canary copy");
    let got2 = timeout(Duration::from_millis(200), sub_c.recv())
        .await
        .expect("C did not receive first canary within 200ms")
        .expect("C's topic channel closed");
    assert_eq!(got2, canary_payload);

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
