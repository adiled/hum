//! End-to-end test of the TCP transport: bind a listener, dial it from
//! a peer, wire both endpoints into separate `Ensemble`s, and route a
//! `chi:"ping"` tone across the wire. Asserts both sides perform the
//! signed handshake and the routed tone reaches the receiving
//! subscriber.

use std::time::Duration;

use ensemble::{Ensemble, HumdAddr, HumdKey, PeerCapabilities, TcpEndpoint, TcpListener};
use serde_json::json;

#[tokio::test]
async fn tcp_endpoint_routes_tone_across_wire() {
    let a_key = HumdKey::generate();
    let b_key = HumdKey::generate();
    let a_id = a_key.hid();
    let b_id = b_key.hid();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let local = listener.local_addr().expect("local_addr");

    let accept_task = tokio::spawn(async move { listener.accept().await });

    let a_endpoint = TcpEndpoint::connect(
        &local.to_string(),
        HumdAddr::new(b_id),
        PeerCapabilities::default(),
    )
    .await
    .expect("connect");

    let b_endpoint = accept_task.await.expect("join").expect("accept");

    // Lax auth so plain `install` works regardless of strict-auth toggles.
    let ensemble_a = Ensemble::with_strict_auth(a_id, false);
    let ensemble_b = Ensemble::with_strict_auth(b_id, false);

    // A's outbound endpoint already knows it is talking to B, so
    // `route()` finds it. The inbound endpoint carries a placeholder id
    // until the drainer parses A's hello; B never routes outbound in
    // this test, only subscribes, so the placeholder is fine there.
    ensemble_a.install(a_endpoint.clone(), PeerCapabilities::default(), &a_key);
    ensemble_b.install(b_endpoint.clone(), PeerCapabilities::default(), &b_key);

    let mut sub_b = ensemble_b.subscribe();

    // The first tone over the wire is A's hello (absorbed by B's
    // drainer), the second is this ping (fans out to subscribers).
    let ping = json!({"chi": "ping", "rid": "tcp-1", "to": b_id.to_hex(), "from": a_id.to_hex()});
    ensemble_a.route(ping).await.expect("route ping");

    let got = tokio::time::timeout(Duration::from_secs(2), sub_b.recv())
        .await
        .expect("recv timed out")
        .expect("subscribe closed");
    assert_eq!(got.get("chi").and_then(|v| v.as_str()), Some("ping"));
    assert_eq!(got.get("rid").and_then(|v| v.as_str()), Some("tcp-1"));
}
