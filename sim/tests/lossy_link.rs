
use std::sync::Arc;
use std::time::Duration;

use ensemble::{LinkFaults, Script};
use sim::Sim;

const WINDOW: Duration = Duration::from_millis(750);

async fn pair() -> (Arc<Sim>, ensemble::Hid, ensemble::Hid) {
    let sim = Sim::new();
    let a = ensemble::Hid::random_humd();
    let b = ensemble::Hid::random_humd();
    sim.spawn_humd(a).await;
    sim.spawn_humd(b).await;
    sim.await_ready(a).await.expect("a ready");
    sim.await_ready(b).await.expect("b ready");
    sim.wire(a, b).unwrap();
    sim.await_handshake(a, b).await.unwrap();
    (Arc::new(sim), a, b)
}

fn base(sim: &Sim, a: ensemble::Hid, b: ensemble::Hid) -> ensemble::LinkCounters {
    sim.link_counters(a, b).unwrap().0
}

#[tokio::test(flavor = "multi_thread")]
async fn intact_delivers_all_in_order() {
    let (sim, a, b) = pair().await;
    let b0 = base(&sim, a, b);
    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.send_marks(a, b, "intact", 6).await.unwrap();

    let rid = Sim::collect_rids(&mut rx, 6, WINDOW).await;
    assert_eq!(rid, ["intact-0","intact-1","intact-2","intact-3","intact-4","intact-5"]);
    let ab = sim.link_counters_since(a, b, &b0).unwrap();
    assert_eq!(ab.offered, 6);
    assert_eq!(ab.dropped, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn dropped_tones_never_arrive() {
    let (sim, a, b) = pair().await;
    let b0 = base(&sim, a, b);
    sim.impair_dir(a, b, LinkFaults::default().drop_next(4)).unwrap();
    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.send_marks(a, b, "lossy", 6).await.unwrap();

    let rid = Sim::collect_rids(&mut rx, 2, WINDOW).await;
    let ab = sim.link_counters_since(a, b, &b0).unwrap();
    assert_eq!(ab.dropped, 4);
    assert_eq!(ab.delivered, 2);
    assert_eq!(rid, ["lossy-4","lossy-5"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicated_tones_arrive_twice() {
    let (sim, a, b) = pair().await;
    let b0 = base(&sim, a, b);
    sim.impair_dir(a, b, LinkFaults::default().dup_next(3)).unwrap();
    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.send_marks(a, b, "dup", 6).await.unwrap();

    let rid = Sim::collect_rids(&mut rx, 9, WINDOW).await;
    let ab = sim.link_counters_since(a, b, &b0).unwrap();
    assert_eq!(ab.duplicated, 3);
    assert_eq!(rid, [
        "dup-0","dup-0","dup-1","dup-1","dup-2","dup-2",
        "dup-3","dup-4","dup-5"
    ]);
}

#[tokio::test(flavor = "multi_thread")]
async fn reordered_tones_swap_adjacent_pairs() {
    let (sim, a, b) = pair().await;
    sim.impair_dir(a, b, LinkFaults::default().reorder_next(2)).unwrap();
    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.send_marks(a, b, "swap", 4).await.unwrap();

    let rid = Sim::collect_rids(&mut rx, 4, WINDOW).await;
    assert_eq!(rid, ["swap-1","swap-0","swap-3","swap-2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn partition_stops_delivery() {
    let (sim, a, b) = pair().await;
    sim.partition(a, b).unwrap();
    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.send_marks(a, b, "cut", 5).await.unwrap();

    assert!(Sim::collect_rids(&mut rx, 5, WINDOW).await.is_empty());
    assert_eq!(sim.buffered(a, b).unwrap(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn heal_flushes_buffer_through_link_faults() {
    let (sim, a, b) = pair().await;
    let b0 = base(&sim, a, b);
    sim.partition(a, b).unwrap();
    sim.send_marks(a, b, "buf", 8).await.unwrap();
    assert_eq!(sim.buffered(a, b).unwrap(), 8);

    sim.impair_dir(a, b, LinkFaults::default().drop_next(3)).unwrap();
    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.heal(a, b).await.unwrap();

    let got = Sim::collect_rids(&mut rx, 5, WINDOW).await;
    let ab = sim.link_counters_since(a, b, &b0).unwrap();
    assert_eq!(ab.lost_on_heal, 3, "the first 3 buffered tones are lost");
    assert_eq!(got, ["buf-3", "buf-4", "buf-5", "buf-6", "buf-7"]);
    assert_eq!(sim.buffered(a, b).unwrap(), 0, "buffer fully drained");
}

#[tokio::test(flavor = "multi_thread")]
async fn link_recovers_after_lossy_heal() {
    let (sim, a, b) = pair().await;
    sim.partition(a, b).unwrap();
    sim.send_marks(a, b, "cut", 4).await.unwrap();
    sim.impair_dir(a, b, LinkFaults::default().drop_next(2)).unwrap();

    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.heal(a, b).await.unwrap();
    sim.heal_link_faults(a, b).unwrap();

    sim.send_marks(a, b, "ok", 3).await.unwrap();
    let got = Sim::collect_rids(&mut rx, 6, WINDOW).await;
    assert_eq!(&got[..2], ["cut-2", "cut-3"], "heal dropped the first two");
    assert_eq!(&got[3..], ["ok-0", "ok-1", "ok-2"], "and the link carries traffic again");
}

#[tokio::test(flavor = "multi_thread")]
async fn impairment_is_directional() {
    let (sim, a, b) = pair().await;
    sim.impair_dir(a, b, LinkFaults::default().drop_next(10)).unwrap();
    let mut rx = sim.humd_peer_sub(a).unwrap();
    sim.send_marks(b, a, "reply", 3).await.unwrap();
    let rid = Sim::collect_rids(&mut rx, 3, WINDOW).await;
    assert_eq!(rid, ["reply-0","reply-1","reply-2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn losses_are_link_level_not_inbox_level() {
    let (sim, a, b) = pair().await;
    let b0 = base(&sim, a, b);
    sim.impair_dir(a, b, LinkFaults::default()
        .script(Script { drop_every: 3, ..Script::default() })
        .seed(7))
     .unwrap();

    let mut rx = sim.humd_peer_sub(b).unwrap();
    sim.send_marks(a, b, "thirds", 30).await.unwrap();
    let rid = Sim::collect_rids(&mut rx, 20, WINDOW).await;

    let ab = sim.link_counters_since(a, b, &b0).unwrap();
    assert_eq!(rid.len() as u64, ab.delivered);
    assert_eq!(ab.offered, 30);
    assert_eq!(ab.dropped, 10);
    assert_eq!(sim.ensemble_dropped(b), 0);
}
