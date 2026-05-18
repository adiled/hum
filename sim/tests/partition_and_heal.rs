//! partition-and-heal — two humds wired; the link drops; each ticks
//! its local wane; the link heals; wane values reconverge.
//!
//! Scope (v0): wane state convergence only. The full narrative
//! (`scenarios/partition-and-heal.md`) also covers petal replay and
//! drone-quiet semantics; those land in follow-up tests once the
//! petal-replay path exists. Here we prove the Lamport tip
//! reconciliation: each side has its own bumps during the outage, the
//! `chi:"wane-sync"` handshake on heal exchanges snapshots, and the
//! receivers merge by max so both `WaneTracker`s agree.
//!
//! Failure modes this catches:
//!   - The wire silently keeps delivering during `partition` (test
//!     would converge during the outage, not after — assertions check
//!     divergence mid-window first).
//!   - The heal flush never fires (wane stays divergent forever; the
//!     post-heal poll times out).
//!   - The merge picks min instead of max (one side regresses).

use std::time::Duration;

use ensemble::Hid;
use sim::Sim;

const SIGIL: &str = "test-sigil";

#[tokio::test(flavor = "multi_thread")]
async fn partition_then_heal_converges_wane() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = Sim::new();
    let a_id = Hid::random_humd();
    let b_id = Hid::random_humd();
    let a = sim.spawn_humd(a_id).await;
    let b = sim.spawn_humd(b_id).await;

    sim.wire(a_id, b_id).expect("wire a-b");

    // Both healthy: tick wane on each side a few times in lockstep
    // (pretending the petal source fed both before the outage). Each
    // side advances its own tracker — wane is per-(sigil,humd).
    for _ in 0..3 {
        a.waneman.tick(SIGIL);
        b.waneman.tick(SIGIL);
    }
    assert_eq!(a.waneman.get(SIGIL), 3);
    assert_eq!(b.waneman.get(SIGIL), 3);

    // Partition. The link buffers / drops; neither side hears the
    // other for the duration of the outage.
    sim.partition(a_id, b_id).expect("partition");

    // During the partition each side keeps producing locally. A
    // advances by 5 (it owns the live petal source); B advances by 1
    // (a heartbeat tick or a local-only event). The tips diverge.
    for _ in 0..5 {
        a.waneman.tick(SIGIL);
    }
    b.waneman.tick(SIGIL);

    assert_eq!(a.waneman.get(SIGIL), 8, "a kept advancing locally");
    assert_eq!(b.waneman.get(SIGIL), 4, "b ticked once during outage");

    // Heal — flushes the buffered link AND exchanges wane snapshots.
    sim.heal(a_id, b_id).await.expect("heal");

    // Poll up to 1s for both sides' wane to reach the joined max (8).
    // The handshake is async (route → ensemble pump → HumdSink merge),
    // so we give it a window rather than asserting immediately.
    let target = 8;
    let mut converged = false;
    for _ in 0..100 {
        if a.waneman.get(SIGIL) == target && b.waneman.get(SIGIL) == target {
            converged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        converged,
        "wane should converge within 1s of heal: a={}, b={}",
        a.waneman.get(SIGIL),
        b.waneman.get(SIGIL),
    );
    assert_eq!(a.waneman.get(SIGIL), b.waneman.get(SIGIL));

    sim.shutdown().await;
}
