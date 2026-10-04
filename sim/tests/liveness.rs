
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

#[tokio::test(flavor = "multi_thread")]
async fn a_talking_peer_is_live_and_survives_a_sweep() {
    let (sim, a, b) = pair().await;
    sim.probe(a, b).await.expect("probe");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![]);
    assert_eq!(sim.peer_count(a), 1, "sweep must not reap a live peer");
}

#[tokio::test(flavor = "multi_thread")]
async fn traffic_renews_the_lease_without_probing() {
    let (sim, a, b) = pair().await;
    for i in 0..3 {
        sim.send_marks(b, a, &format!("keep{i}"), 1)
            .await
            .expect("send");
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_killed_peer_goes_dead() {
    let (sim, a, b) = pair().await;
    sim.probe(a, b).await.expect("probe");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));

    sim.kill_peer(b, a).expect("kill b→a");
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Dead));
}

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

#[tokio::test(flavor = "multi_thread")]
async fn silence_past_ttl_goes_stale_and_is_reaped() {
    let (sim, a, b) = pair().await;
    sim.probe(a, b).await.expect("probe");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));

    sim.impair_dir(b, a, ensemble::LinkFaults::default().drop_pct(100, 3))
        .expect("silence b→a");
    sim.probe(a, b).await.expect("probe");
    tokio::time::sleep(TTL + Duration::from_millis(60)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Stale));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![b]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_partitioned_peer_is_not_dead() {
    let (sim, a, b) = pair().await;
    sim.partition(a, b).expect("partition");
    tokio::time::sleep(TTL + Duration::from_millis(60)).await;

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

#[tokio::test(flavor = "multi_thread")]
async fn a_rewired_peer_is_live_again() {
    let (sim, a, b) = pair().await;
    sim.kill_link(a, b).expect("kill");
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![b]);
    assert_eq!(sim.peer_count(a), 0);

    sim.rewire(a, b).expect("rewire");
    sim.await_handshake(a, b).await.expect("handshake again");
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Live));
    assert_eq!(sim.peer_count(a), 1);
}

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
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_never_answers_goes_stale() {
    let (sim, a, b) = pair().await;
    sim.impair(a, b, ensemble::LinkFaults::default().drop_pct(100, 7))
        .expect("black hole");
    sim.probe(a, b).await.expect("probe");
    tokio::time::sleep(TTL + Duration::from_millis(60)).await;
    assert_eq!(liveness(&sim, a, b), Some(Liveness::Stale));
    assert_eq!(sim.evict_expired(a, TTL).expect("sweep"), vec![b]);
}
