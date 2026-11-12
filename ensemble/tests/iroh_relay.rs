//! Two humds meet across the public n0 relay mesh, with no IP address
//! of the other side that works. This is the shape of two devices
//! behind separate NATs.
//!
//! These reach the live n0 relay and DNS servers, so they are
//! `#[ignore]`d and CI never depends on them. Run them by hand:
//!
//! ```text
//! cargo test -p ensemble --test iroh_relay -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use ensemble::{
    HumdAddr, HumdKey, IROH_RELAY_HINT, IrohEndpoint, IrohTransport, PeerConnection, Transport,
};
use serde_json::json;

const ONLINE_WAIT: Duration = Duration::from_secs(20);
const DIAL_WAIT: Duration = Duration::from_secs(15);
const DIAL_ATTEMPTS: u32 = 4;

const UNROUTABLE_IP_HINT: &str = "iroh-ip:192.0.2.1:4556";

async fn online_humd() -> (IrohTransport, HumdKey) {
    let key = HumdKey::generate();
    let transport = IrohTransport::bind_with_key(&key)
        .await
        .expect("bind with key");
    assert!(
        transport.online_within(ONLINE_WAIT).await,
        "no home relay within {ONLINE_WAIT:?}: is the n0 relay mesh reachable from here?"
    );
    (transport, key)
}

fn node_hint(transport: &IrohTransport) -> String {
    format!("iroh:{}", hex::encode(transport.node_id().as_bytes()))
}

async fn dial_until_reached(dialer: &IrohTransport, target: &HumdAddr) -> Arc<dyn PeerConnection> {
    let mut last_err = String::new();
    for attempt in 1..=DIAL_ATTEMPTS {
        match tokio::time::timeout(DIAL_WAIT, dialer.connect(target)).await {
            Ok(Ok(conn)) => return conn,
            Ok(Err(e)) => last_err = e.to_string(),
            Err(_) => last_err = format!("no answer within {DIAL_WAIT:?}"),
        }
        eprintln!("iroh_relay: dial attempt {attempt} failed: {last_err}");
    }
    panic!("never reached the peer over the relay: {last_err}");
}

async fn a_tone_crosses(listener: Arc<IrohTransport>, dialer: &IrohTransport, target: HumdAddr) {
    let accepted = tokio::spawn(async move {
        let peer: Arc<IrohEndpoint> = listener.accept().await.expect("accept");
        let mut rx = peer.take_receiver().expect("receiver");
        let tone = tokio::time::timeout(DIAL_WAIT, rx.recv())
            .await
            .expect("tone within the wait")
            .expect("link stayed open");
        (peer, tone)
    });

    let conn = dial_until_reached(dialer, &target).await;
    conn.send_opening(json!({"chi": "ping", "rid": "across-the-relay"}))
        .await
        .expect("send");

    let (_peer, tone) = tokio::time::timeout(DIAL_WAIT * DIAL_ATTEMPTS, accepted)
        .await
        .expect("listener heard the tone in time")
        .expect("listener task");
    assert_eq!(tone["rid"], "across-the-relay");
}

#[tokio::test]
#[ignore = "reaches the live n0 relay mesh"]
async fn a_humd_is_reached_through_the_relay_it_advertises() {
    let (listener, _listener_key) = online_humd().await;
    let (dialer, _dialer_key) = online_humd().await;

    let relay_hints = listener.relay_hints();
    assert!(
        !relay_hints.is_empty(),
        "an online humd must advertise its home relay"
    );
    assert!(relay_hints.iter().all(|h| h.starts_with(IROH_RELAY_HINT)));

    let mut target = HumdAddr::new(ensemble::iroh::humd_id_from_node_id(&listener.node_id()))
        .with_hint(node_hint(&listener))
        .with_hint(UNROUTABLE_IP_HINT);
    for hint in relay_hints {
        target = target.with_hint(hint);
    }

    a_tone_crosses(Arc::new(listener), &dialer, target).await;
}

#[tokio::test]
#[ignore = "reaches the live n0 relay mesh and n0 DNS"]
async fn a_humd_is_reached_by_node_id_alone_through_dns() {
    let (listener, _listener_key) = online_humd().await;
    let (dialer, _dialer_key) = online_humd().await;

    let target = HumdAddr::new(ensemble::iroh::humd_id_from_node_id(&listener.node_id()))
        .with_hint(node_hint(&listener))
        .with_hint(UNROUTABLE_IP_HINT);

    a_tone_crosses(Arc::new(listener), &dialer, target).await;
}
