//! liveness — a peer that dies is detected, reaped, and comes back.
//!
//! The lossy-link suite proves a broken link loses data. This one proves
//! the system *notices*, which is the property that makes recovery
//! possible at all: routing must stop addressing a dead peer, and a
//! peer that restarts must become reachable without a daemon restart.
//!
//! TTLs here are milliseconds. A liveness test that sleeps for a real
//! TTL is slow, and a slow test is one that gets skipped.

use std::time::Duration;

use ensemble::Liveness;
use sim::Sim;

const TTL: Duration = Duration::from_millis(120);

async fn pair() -> (std::sync::Arc<Sim>, ensemble::Hid, ensemble::Hid) {
    let sim = Sim::new();
    let a = ensemble::Hid::random_humd();
    let b = ensemble::Hid::random_humd();
    sim.spawn_humd(a).await;
    sim.spawn_humd(b).await;
    sim.await_ready(a).await.expect("a ready");
    sim.await_ready(b).await.expect("b ready");
    sim.wire(a, b).expect("wire");
    sim.await_handshake(a, b).await.expect("handshake");
    (std::sync::Arc::new(sim), a, b)
}

fn liveness(sim: &Sim, observer: ensemble::Hid, peer: ensemble::Hid) -> Option<Liveness> {
    sim.peer_liveness(observer, TTL)
        .expect("observer exists")
        .into_iter()
        .find(|(p, _)| *p == peer)
        .map(|(_, l)| l)
}

/// A peer that is answering is Live, and a sweep leaves it alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_talking_peer_is_live_and_survives_a_sweep() {
    let (sim, a, b) = pair().await;
    sim.probe(a, b).await.expect("probe");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![]);
    assert_eq!(sim.peer_count(a), 1, "sweep must not reap a live peer");
}

/// Traffic is what renews the lease. This is the whole point: a busy
/// peer never needs to be probed.
#[tokio::test(flavor = "multi_thread")]
async fn traffic_renews_the_lease_without_probing() {
    let (sim, a, b) = pair().await;
    // Drain off a peer stamps the lease in the drainer, so a real tone
    // is enough.
    for i in 0..3 {
        sim.send_marks(b, a, &format!("keep{i}"), 1)
            .await
            .expect("send");
    }
    // Give the drainer a moment to publish.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![]);
}

/// A killed peer's link stops answering, and the drainer notices the
/// transport is gone rather than merely quiet.
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_peer_goes_dead() {
    let (sim, a, b) = pair().await;
    sim.probe(a, b).await.expect("probe");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));

    sim.kill_peer(b, a).expect("kill b→a");
    // The drainer wakes on its closed receiver.
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Dead));
}

/// A dead peer is still in the registry until a sweep runs. Eviction is
/// the supervisor's call, not the transport's.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_peer_survives_until_swept() {
    let (sim, a, b) = pair().await;
    sim.kill_link(a, b).expect("kill");
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(sim.peer_count(a), 1, "transport death does not self-evict");

    let evicted = sim.evict_expired(a, TTL).expect("sweep");
    assert_eq!(evicted, vec![b]);
    assert_eq!(sim.peer_count(a), 0, "sweep reaps the dead peer");
}

/// Silence past the TTL is staleness — the peer never reported a closed
/// link, it just stopped answering.
#[tokio::test(flavor = "multi_thread")]
async fn silence_past_ttl_goes_stale_and_is_reaped() {
    let (sim, a, b) = pair().await;
    sim.probe(a, b).await.expect("probe");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));

    // Drop every answer without closing the link, so the transport
    // still looks up. This is the wedged-peer case: a connection that
    // accepts writes but delivers nothing.
    sim.impair_dir(b, a, ensemble::LinkFaults::default().drop_pct(100, 3))
        .expect("silence b→a");
    sim.probe(a, b).await.expect("probe");
    // A:→B still delivers, so the link is not killed — but B never
    // answers A's probe.
    tokio::time::sleep(TTL + Duration::from_millis(60)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Stale));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![b]);
}

/// The asymmetry that makes partitions and death different things: a
/// partitioned peer is not reaped, because it is still there. If
/// partitions evicted, every blip would churn the peer set.
#[tokio::test(flavor = "multi_thread")]
async fn a_partitioned_peer_is_not_dead() {
    let (sim, a, b) = pair().await;
    sim.partition(a, b).expect("partition");
    tokio::time::sleep(TTL + Duration::from_millis(60)).await;

    // Silence is silence: from here a partition and a death look the
    // same, and the sweep reaps both. What a partition is *not* is
    // `Dead`, which is reserved for a transport that told us it closed
    // — a fact rather than an inference, and the only case that earns
    // an immediate redial.
    assert_ne!(
        liveness(&sim, a, b),
        Some(Liveness::Dead),
        "no transport reported closure, so this is not a proven death"
    );
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Stale));
    assert_eq!(
        sim.peer_count(a),
        1,
        "a partition does not evict on its own"
    );
}

/// After reaping, a rewired peer is reachable again — the recovery the
/// whole subsystem exists for. The liveness state must not be sticky.
#[tokio::test(flavor = "multi_thread")]
async fn a_rewired_peer_is_live_again() {
    let (sim, a, b) = pair().await;
    sim.kill_link(a, b).expect("kill");
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![b]);
    assert_eq!(sim.peer_count(a), 0);

    // Rewire, as the supervisor's redial would.
    sim.rewire(a, b).expect("rewire");
    sim.await_handshake(a, b).await.expect("handshake again");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));
    assert_eq!(sim.peer_count(a), 1);
}

/// Eviction is per-observer. B dying must not disturb A's other peers.
#[tokio::test(flavor = "multi_thread")]
async fn eviction_only_touches_the_dead_peer() {
    let sim = Sim::new();
    let a = ensemble::Hid::random_humd();
    let b = ensemble::Hid::random_humd();
    let c = ensemble::Hid::random_humd();
    for id in [a, b, c] {
        sim.spawn_humd(id).await;
        sim.await_ready(id).await.expect("ready");
    }
    sim.wire(a, b).expect("wire ab");
    sim.wire(a, c).expect("wire ac");
    sim.await_handshake(a, b).await.expect("ab");
    sim.await_handshake(a, c).await.expect("ac");
    let sim = std::sync::Arc::new(sim);

    sim.probe(a, b).await.expect("probe");
    sim.probe(a, c).await.expect("probe");
    sim.kill_link(b, a).expect("kill b");
    tokio::time::sleep(Duration::from_millis(40)).await;

    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![b]);
    assert_eq!(liveness(&sim, a, c), Some(Liveness::Live), "c is untouched");
    assert_eq!(sim.peer_count(a), 1, "only b was reaped");
}
/// A link that swallows every probe must still be reaped. Without an
/// install timestamp such a peer is unproven forever, and "unproven"
/// quietly becomes "immortal".
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_never_answers_goes_stale() {
    let (sim, a, b) = pair().await;
    // Black-hole both directions: a's probe arrives, b's reply does not.
    sim.impair(a, b, ensemble::LinkFaults::default().drop_pct(100, 7))
        .expect("black hole");
    sim.probe(a, b).await.expect("probe");
    tokio::time::sleep(TTL + Duration::from_millis(60)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Stale));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![b]);
}
