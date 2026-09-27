//! delivery — at-most-once, expiry, and the line between them.
//!
//! At-most-once is a property of a *message identity*, and getting the
//! identity wrong breaks the feature it is meant to provide. Two cases
//! look alike on the wire and must not be conflated:
//!
//!   - the network delivered one message twice → deliver once
//!   - the originator sent the same content twice → deliver twice
//!
//! `rid` cannot tell them apart, so it is not the identity. See
//! `mint_msg_id`: the originator assigns a fresh id per publish.

use std::time::Duration;

use serde_json::json;
use sim::Sim;
use tokio::time::timeout;

const WINDOW: Duration = Duration::from_millis(750);

async fn trio() -> (Sim, ensemble::Hid, ensemble::Hid, ensemble::Hid) {
    // A publishes, B is the middle hop, C subscribes. The repeat and
    // the duplicate must both survive one hop of re-fan to be judged at
    // all — a bug that drops them at the originator would look correct.
    let sim = Sim::new();
    let a = ensemble::Hid::random_humd();
    let b = ensemble::Hid::random_humd();
    let c = ensemble::Hid::random_humd();
    for id in [a, b, c] {
        sim.spawn_humd(id).await;
        sim.await_ready(id).await.expect("ready");
    }
    sim.wire(a, b).expect("wire a-b");
    sim.wire(b, c).expect("wire b-c");
    sim.await_handshake(a, b).await.expect("a-b");
    sim.await_handshake(b, c).await.expect("b-c");
    (sim, a, b, c)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_publish_is_delivered_twice() {
    let (sim, a, _b, c) = trio().await;
    let mut sub = sim.subscribe_topic(c, "alerts").expect("subscribe");

    // A heartbeat, or a standing alert. The second one is not a
    // duplicate of the first — the sender meant to send it.
    let payload = json!({"event": "overloaded", "level": 3});
    sim.publish(a, "alerts", payload.clone()).await.expect("publish");
    sim.publish(a, "alerts", payload.clone()).await.expect("publish");

    for i in 0..2 {
        let got = timeout(WINDOW, sub.recv())
            .await
            .unwrap_or_else(|_| panic!("publish {i} never arrived"))
            .expect("topic channel closed");
        assert_eq!(got, payload);
    }
}

/// The other half, and the one at-most-once actually promises: one
/// publish, duplicated by the link, delivered once. The repeat above
/// and this duplicate are the same bytes on the wire — only the sender
/// knows which one it meant.
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicated_publish_is_delivered_once() {
    let (sim, a, b, c) = trio().await;
    let mut sub = sim.subscribe_topic(c, "alerts").expect("subscribe");

    sim.impair_dir(b, c, ensemble::LinkFaults::default().dup_next(1))
        .expect("duplicate the next b->c tone");

    sim.publish(a, "alerts", json!({"event": "overloaded", "level": 3}))
        .await
        .expect("publish");

    let got = timeout(WINDOW, sub.recv()).await.expect("arrived").expect("open");
    assert_eq!(got["event"], "overloaded");

    // The link really did send it twice — otherwise this test proves
    // nothing about dedup.
    let counters = sim.link_counters(b, c).expect("counters").0;
    assert!(counters.duplicated >= 1, "the link did not duplicate: {counters:?}");

    let extra = timeout(Duration::from_millis(200), sub.recv()).await;
    assert!(extra.is_err(), "the duplicate was delivered too: {extra:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_tone_is_dropped_on_arrival() {
    let sim = Sim::new();
    let a = ensemble::Hid::random_humd();
    let b = ensemble::Hid::random_humd();
    for id in [a, b] {
        sim.spawn_humd(id).await;
        sim.await_ready(id).await.expect("ready");
    }
    sim.wire(a, b).expect("wire");
    sim.await_handshake(a, b).await.expect("a-b");

    let mut rx = sim.humd_peer_sub(b).expect("subscribe");
    let past = now_ms() - 60_000;
    sim.nestler_send_ordered(
        a,
        json!({"chi": "perf-mark", "rid": "stale-1", "to": b.to_hex(), "dusk": past}),
    )
    .await
    .expect("send");

    // Nothing should arrive, and the drop must be counted rather than
    // inferred from absence.
    let got = timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(got.is_err(), "a tone past its dusk was delivered: {got:?}");
    assert_eq!(
        sim.expired_dusk(b),
        1,
        "the expiry rule should have counted it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tone_within_its_dusk_is_delivered() {
    let sim = Sim::new();
    let a = ensemble::Hid::random_humd();
    let b = ensemble::Hid::random_humd();
    for id in [a, b] {
        sim.spawn_humd(id).await;
        sim.await_ready(id).await.expect("ready");
    }
    sim.wire(a, b).expect("wire");
    sim.await_handshake(a, b).await.expect("a-b");

    let mut rx = sim.humd_peer_sub(b).expect("subscribe");
    sim.nestler_send_ordered(
        a,
        json!({
            "chi": "perf-mark", "rid": "live-1", "to": b.to_hex(),
            "dusk": now_ms() + 60_000
        }),
    )
    .await
    .expect("send");

    let got = timeout(WINDOW, rx.recv()).await.expect("arrived").expect("open");
    assert_eq!(got["rid"], "live-1");
    assert_eq!(sim.expired_dusk(b), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tone_with_no_dusk_never_expires() {
    let sim = Sim::new();
    let a = ensemble::Hid::random_humd();
    let b = ensemble::Hid::random_humd();
    for id in [a, b] {
        sim.spawn_humd(id).await;
        sim.await_ready(id).await.expect("ready");
    }
    sim.wire(a, b).expect("wire");
    sim.await_handshake(a, b).await.expect("a-b");

    let mut rx = sim.humd_peer_sub(b).expect("subscribe");
    sim.nestler_send_ordered(a, json!({"chi": "perf-mark", "rid": "forever-1", "to": b.to_hex()}))
        .await
        .expect("send");

    let got = timeout(WINDOW, rx.recv()).await.expect("arrived").expect("open");
    assert_eq!(got["rid"], "forever-1");
    assert_eq!(sim.expired_dusk(b), 0);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}
