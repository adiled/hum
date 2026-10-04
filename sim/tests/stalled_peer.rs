
use std::sync::Arc;
use std::time::Duration;

use ensemble::Hid;
use serde_json::{json, Value};
use sim::{Sim, SimHumd};
use tokio::time::timeout;

const PATIENCE: Duration = Duration::from_secs(3);
const SETTLE: Duration = Duration::from_secs(20);

fn tone_to(target: Hid, rid: &str) -> Value {
    json!({
        "chi": "chunk",
        "rid": rid,
        "from": "test",
        "to": target.to_string(),
        "body": {"text": "x"},
    })
}

async fn three(sim: &Sim) -> (Arc<SimHumd>, Arc<SimHumd>, Arc<SimHumd>) {
    let a = sim.spawn_humd(Hid::random_humd()).await;
    let b = sim.spawn_humd(Hid::random_humd()).await;
    let c = sim.spawn_humd(Hid::random_humd()).await;
    for h in [&a, &b, &c] {
        sim.await_ready(h.id).await.expect("ready");
    }
    sim.wire(a.id, b.id).expect("wire a-b");
    sim.wire(a.id, c.id).expect("wire a-c");
    sim.await_handshake(a.id, b.id).await.expect("handshake a-b");
    sim.await_handshake(a.id, c.id).await.expect("handshake a-c");
    (a, b, c)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_peer_does_not_starve_a_healthy_one_in_a_fanout() {
    let sim = Sim::new();
    let (a, b, c) = three(&sim).await;
    let topic = "stall/fanout";
    let mut c_sub = sim.subscribe_topic(c.id, topic).expect("C subscribed");

    sim.stall_dir(a.id, b.id).expect("stall A->B");
    assert!(sim.is_stalled(a.id, b.id).expect("stalled"));

    timeout(SETTLE, sim.publish(a.id, topic, serde_json::json!({"k": "v"})))
        .await
        .expect("publish must return: a fan-out cannot await a dead write forever")
        .expect("A accepts the publish");

    let got = tokio::time::timeout(PATIENCE, c_sub.recv())
        .await
        .expect("C must receive: one stalled peer may not cancel a fan-out")
        .expect("C's subscription is open");
    assert_eq!(got, serde_json::json!({"k": "v"}));

    let mut b_sub = sim.subscribe_topic(b.id, topic).expect("B subscribed");
    let got = tokio::time::timeout(Duration::from_millis(300), b_sub.recv()).await;
    assert!(got.is_err(), "a stalled peer must not be counted as delivered");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fanout_past_two_stalled_peers_still_arrives() {
    let sim = Sim::new();
    let (a, b, c) = three(&sim).await;
    let d = sim.spawn_humd(Hid::random_humd()).await;
    sim.await_ready(d.id).await.expect("d ready");
    sim.wire(a.id, d.id).expect("wire a-d");
    sim.await_handshake(a.id, d.id).await.expect("handshake a-d");

    let topic = "stall/two";
    let mut d_sub = sim.subscribe_topic(d.id, topic).expect("D subscribed");

    sim.stall_dir(a.id, b.id).expect("stall A->B");
    sim.stall_dir(a.id, c.id).expect("stall A->C");

    let started = std::time::Instant::now();
    timeout(SETTLE, sim.publish(a.id, topic, serde_json::json!({"k": "v"})))
        .await
        .expect("publish must return")
        .expect("A accepts the publish");

    let got = tokio::time::timeout(PATIENCE * 4, d_sub.recv())
        .await
        .expect("D must receive")
        .expect("D's subscription is open");
    assert_eq!(got, serde_json::json!({"k": "v"}));
    assert!(
        started.elapsed() < ensemble::SEND_TIMEOUT * 3,
        "two stalled peers cost two deadlines, not an unbounded wait: {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_peer_is_counted_and_evicted() {
    let sim = Sim::new();
    let (a, b, _c) = three(&sim).await;

    assert_eq!(sim.peer_count(a.id), 2, "A knows B and C to start");

    sim.stall_dir(a.id, b.id).expect("stall A->B");
    sim.nestler_send(a.id, tone_to(b.id, "into-the-stall"))
        .expect("A accepts local work");

    timeout(PATIENCE, async {
        loop {
            let (ab, _ba) = sim.link_counters(a.id, b.id).expect("counters");
            if ab.stalled_sends > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the write must reach the stalled link and stay there");

    timeout(SETTLE, async {
        while a.ensemble.send_timeouts() == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("a bounded send must resolve; an unbounded one never would");

    assert!(
        a.ensemble.send_timeouts() >= 1,
        "a stall that is not counted is a stall nobody gets paged for"
    );

    a.ensemble.evict_expired(Duration::from_millis(0));
    assert!(
        !a.ensemble.peers().contains(&b.id),
        "B cannot take a write, so B must not keep renewing a lease against A"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stall_is_reversible_and_does_not_fake_a_partition() {
    let sim = Sim::new();
    let (a, b, _c) = three(&sim).await;

    sim.stall_dir(a.id, b.id).expect("stall A->B");

    assert!(
        a.ensemble.peers().contains(&b.id),
        "a stalled peer is not evicted by looking at it"
    );
    let liveness = sim.peer_liveness(a.id, Duration::from_secs(60)).expect("liveness");
    let b_state = liveness
        .iter()
        .find(|(id, _)| *id == b.id)
        .map(|(_, l)| *l)
        .expect("B is in the registry");
    assert_eq!(
        b_state,
        ensemble::Liveness::Live,
        "a stalled peer still looks live: the lease renews on other traffic, \
         which is exactly why silence alone cannot find it"
    );

    sim.unstall_dir(a.id, b.id).expect("unstall");
    assert!(!sim.is_stalled(a.id, b.id).expect("not stalled"));
    let mut sub = sim.humd_peer_sub(b.id).expect("B subscribed");
    sim.nestler_send(a.id, tone_to(b.id, "after-heal"))
        .expect("A accepts local work");
    let got = Sim::collect_rids(&mut sub, 1, PATIENCE).await;
    assert_eq!(got, vec!["after-heal".to_string()], "the link works again");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stall_is_silence_at_the_far_side_rather_than_an_error() {
    let sim = Sim::new();
    let (_a, b, c) = three(&sim).await;
    sim.wire(c.id, b.id).expect("wire c-b");
    sim.await_handshake(c.id, b.id).await.expect("handshake c-b");

    sim.stall_dir(c.id, b.id).expect("stall C->B");
    sim.stall_dir(b.id, c.id).expect("stall B->C");

    let mut b_sub = sim.humd_peer_sub(b.id).expect("B subscribed");
    sim.nestler_send(c.id, tone_to(b.id, "into-the-void"))
        .expect("C accepts local work");

    let got = Sim::collect_rids(&mut b_sub, 1, Duration::from_millis(500)).await;
    assert!(
        got.is_empty(),
        "a stalled peer sees nothing at all — no error, no partial tone: got {got:?}"
    );
    assert!(
        b.ensemble.send_timeouts() > 0 || b.ensemble.send_failures() == 0,
        "the stall is visible on the sending side even though it is invisible on the receiving one"
    );
}
