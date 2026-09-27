//! A peer that stops reading must not take the mesh with it.
//!
//! The failure this file pins down: one humd whose socket to peer B has
//! filled because B stopped reading. The write to B never completes. If
//! the send that dispatches to *all* peers awaits that write, the single
//! dispatch task is parked forever and every peer after B in the
//! iteration gets nothing — not slowly, but never. The only symptom is a
//! `Lagged(n)` on a broadcast receiver, which reports a count and no
//! cause, so the mesh looks like it lost messages for no reason.
//!
//! Two properties, both of which the old code failed:
//!   1. containment — a healthy peer on the same dispatcher keeps
//!      receiving while another peer is stalled;
//!   2. detection — the stalled peer is eventually counted and evicted,
//!      because a lease that keeps renewing against a peer which cannot
//!      take a write is a stall that hides forever.

use std::sync::Arc;
use std::time::Duration;

use ensemble::Hid;
use serde_json::{json, Value};
use sim::{Sim, SimHumd};
use tokio::time::timeout;

/// Longer than the production send deadline, so a delivery that arrives
/// within this window arrived on its own merits.
const PATIENCE: Duration = Duration::from_secs(3);
/// Enough headroom for the send deadline to expire, the connection to
/// close, and the lease to be reaped. Generous on purpose: the
/// alternative is a test that fails on a slow machine.
const SETTLE: Duration = Duration::from_secs(20);

/// A tone for `target`, named so the receiver can identify it.
fn tone_to(target: Hid, rid: &str) -> Value {
    json!({
        "chi": "chunk",
        "rid": rid,
        "from": "test",
        "to": target.to_string(),
        "body": {"text": "x"},
    })
}

/// Three humds, wired, ready.
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

/// The headline property, on the one code path that really is a single
/// loop over every peer: a gossip publish.
///
/// `publish` walks its peers and awaits each write in turn, so before the
/// deadline existed a stalled peer parked the whole fan-out. C's
/// subscription then never saw a message that A had already accepted.
/// This is not a hypothetical ordering: B and C are consecutive entries
/// in the same `HashMap` walk, and which comes first is arbitrary — so
/// the loss is intermittent and looks like a flaky mesh.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_peer_does_not_starve_a_healthy_one_in_a_fanout() {
    let sim = Sim::new();
    let (a, b, c) = three(&sim).await;
    let topic = "stall/fanout";
    let mut c_sub = sim.subscribe_topic(c.id, topic).expect("C subscribed");

    sim.stall_dir(a.id, b.id).expect("stall A->B");
    assert!(sim.is_stalled(a.id, b.id).expect("stalled"));

    // The publish is itself bounded. Without the send deadline it never
    // returns, and a test that hangs tells you far less than one that
    // fails — it just eats the CI timeout and names no cause.
    timeout(SETTLE, sim.publish(a.id, topic, serde_json::json!({"k": "v"})))
        .await
        .expect("publish must return: a fan-out cannot await a dead write forever")
        .expect("A accepts the publish");

    let got = tokio::time::timeout(PATIENCE, c_sub.recv())
        .await
        .expect("C must receive: one stalled peer may not cancel a fan-out")
        .expect("C's subscription is open");
    assert_eq!(got, serde_json::json!({"k": "v"}));

    // A must not have delivered to the peer it cannot write to. Silence
    // there is correct, and is asserted so this cannot be "fixed" later
    // by making the fan-out lie.
    let mut b_sub = sim.subscribe_topic(b.id, topic).expect("B subscribed");
    let got = tokio::time::timeout(Duration::from_millis(300), b_sub.recv()).await;
    assert!(got.is_err(), "a stalled peer must not be counted as delivered");
}

/// Two stalled peers must not delay the healthy one for longer than one
/// deadline each. A fan-out that bounds each send sequentially is bounded
/// but still multiplied by the peer count; this pins the current cost so
/// a change that reintroduces unbounded waiting shows up as a failure
/// rather than as a latency graph nobody reads.
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

/// The other half: a bounded send that does not evict leaves the lease
/// renewing against a peer that cannot take a write.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_peer_is_counted_and_evicted() {
    let sim = Sim::new();
    let (a, b, _c) = three(&sim).await;

    assert_eq!(sim.peer_count(a.id), 2, "A knows B and C to start");

    sim.stall_dir(a.id, b.id).expect("stall A->B");
    sim.nestler_send(a.id, tone_to(b.id, "into-the-stall"))
        .expect("A accepts local work");

    // The write is handed to the stalled link and never comes back.
    // `nestler_send` injects on a detached task, so this has to be
    // awaited rather than read straight away.
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

    // The send deadline has to expire, and it has to be visible.
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

    // And the peer it cost its connection is reaped, not left live.
    a.ensemble.evict_expired(Duration::from_millis(0));
    assert!(
        !a.ensemble.peers().contains(&b.id),
        "B cannot take a write, so B must not keep renewing a lease against A"
    );
}

/// A stall is not a kill, and must not be confused with one: it is a
/// broken writer, not a broken link. Asserted so a future "fix" that
/// simply kills every slow peer is caught.
#[tokio::test(flavor = "multi_thread")]
async fn a_stall_is_reversible_and_does_not_fake_a_partition() {
    let sim = Sim::new();
    let (a, b, _c) = three(&sim).await;

    sim.stall_dir(a.id, b.id).expect("stall A->B");

    // Nothing about the stall is visible to a lease. This is the reason a
    // send deadline is needed and silence alone is not enough — asserted
    // rather than assumed, because it is the whole justification.
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

    // Reversible, so the fault does not leak into neighbouring tests.
    sim.unstall_dir(a.id, b.id).expect("unstall");
    assert!(!sim.is_stalled(a.id, b.id).expect("not stalled"));
    let mut sub = sim.humd_peer_sub(b.id).expect("B subscribed");
    sim.nestler_send(a.id, tone_to(b.id, "after-heal"))
        .expect("A accepts local work");
    let got = Sim::collect_rids(&mut sub, 1, PATIENCE).await;
    assert_eq!(got, vec!["after-heal".to_string()], "the link works again");
}

/// What a stall actually looks like from the far side: silence, not an
/// error. Nothing is logged at the receiver, nothing is counted as lost,
/// and the sender is still perfectly willing to try again.
///
/// Kept as a separate test because it is easy to write a "fix" that
/// makes a stall loud — close the connection, retry, surface an error —
/// and this pins the property that the *receiver* cannot tell the
/// difference. It is also honest about its limits: the two peers here are
/// dispatched independently, so this is not a serialisation test. The
/// fan-out tests above are the ones that cover ordering.
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
