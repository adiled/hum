//! Smoke test — spawn 2 humds, wire them, verify each ensemble sees
//! the other. No prompts, no tones — just the vital signs.

use ensemble::Hid;
use sim::Sim;

#[tokio::test]
async fn two_humds_wired_see_each_other() {
    let sim = Sim::new();
    let a = Hid::random_humd();
    let b = Hid::random_humd();

    let ha = sim.spawn_humd(a).await;
    let hb = sim.spawn_humd(b).await;

    sim.wire(a, b).expect("wire");

    let peers_a = ha.ensemble.peers();
    let peers_b = hb.ensemble.peers();
    assert!(peers_a.contains(&b), "a should see b in its ensemble");
    assert!(peers_b.contains(&a), "b should see a in its ensemble");

    sim.shutdown().await;
}
