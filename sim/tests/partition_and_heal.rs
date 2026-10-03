//! partition-and-heal — two humds wired; the link drops; each ticks
//! its local wane; the link heals; wane values reconverge.
//!
//! Scope (v0): wane state convergence only. The full narrative
//! (`scenarios/partition-and-heal.md`) also covers petal replay and
//! drone-quiet semantics; those land in follow-up tests once the
//! petal-replay path exists. Here we prove the Lamport tip
//! reconciliation: each side has its own bumps during the outage, the
//! `chi:"wane-sync"` handshake on heal exchanges snapshots over the real
//! link, and the receivers merge by max so both `WaneTracker`s agree.
//!
//! Every failure mode named below is asserted, not assumed. The two that
//! were previously claimed and not checked:
//!
//!   - *the wire keeps delivering during the partition* — checked by
//!     holding the outage open and re-reading both tips on every poll. A
//!     partition that leaks would reconverge on its own, and the
//!     original test never gave it the chance.
//!   - *the heal flush never fires* — checked by bounding the wait on a
//!     deadline, so a silent flush is a failure with a message rather
//!     than a test that hangs.
//!
//! Still caught by the convergence assertion:
//!
//!   - the merge picks min instead of max (one side regresses).

use std::time::Duration;

use ensemble::Hid;
use sim::Sim;

const SIGIL: &str = "test-sigil";
/// How long the outage is held open while we insist nothing crosses.
/// Long enough that a leaky link would have to be leaking very slowly to
/// escape, short enough not to dominate the suite.
const OUTAGE: Duration = Duration::from_millis(600);
/// Heal must converge well inside this. Bounded so a broken flush fails
/// with a diagnosis instead of hanging until the CI timeout.
const CONVERGE_BY: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread")]
async fn partition_then_heal_converges_wane() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = Sim::new();
    let a_id = Hid::random_humd();
    let b_id = Hid::random_humd();
    let a = sim.spawn_humd(a_id).await;
    let b = sim.spawn_humd(b_id).await;

    sim.await_ready(a_id).await.expect("a ready");
    sim.await_ready(b_id).await.expect("b ready");
    sim.wire(a_id, b_id).expect("wire a-b");

    // Both healthy: tick wane on each side in lockstep (pretending the
    // petal source fed both before the outage). Wane is per-(sigil,humd),
    // so each side advances its own tracker.
    for _ in 0..3 {
        a.waneman.tick(SIGIL);
        b.waneman.tick(SIGIL);
    }
    assert_eq!(a.waneman.get(SIGIL), 3);
    assert_eq!(b.waneman.get(SIGIL), 3);

    sim.partition(a_id, b_id).expect("partition");

    // During the outage each side keeps producing locally. A advances by
    // 5 (it owns the live petal source); B advances by 1 (a heartbeat
    // tick or a local-only event). The tips diverge.
    for _ in 0..5 {
        a.waneman.tick(SIGIL);
    }
    b.waneman.tick(SIGIL);

    assert_eq!(a.waneman.get(SIGIL), 8, "a kept advancing locally");
    assert_eq!(b.waneman.get(SIGIL), 4, "b ticked once during outage");

    // Prove the link is actually down by trying to use it. Merely
    // watching two numbers stay apart proves nothing while no traffic
    // exists to leak: a partition that delivered everything perfectly
    // would leave these tips just as divergent, because nothing is sent
    // until the heal. So send a real wane-sync into the outage. If the
    // partition is not a partition, B adopts A's 8 immediately and this
    // is caught here rather than being mistaken for a working heal
    // later.
    sim.wane_sync(&a, &b).await.expect("a emits into the outage");
    sim.wane_sync(&b, &a).await.expect("b emits into the outage");

    let until = std::time::Instant::now() + OUTAGE;
    while std::time::Instant::now() < until {
        assert_eq!(
            (a.waneman.get(SIGIL), b.waneman.get(SIGIL)),
            (8, 4),
            "nothing may cross a partitioned link: the tips changed before the heal"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Heal — flushes the buffered link AND exchanges wane snapshots.
    sim.heal(a_id, b_id).await.expect("heal");

    let target = 8;
    let mut converged_at = None;
    let deadline = std::time::Instant::now() + CONVERGE_BY;
    while std::time::Instant::now() < deadline {
        if a.waneman.get(SIGIL) == target && b.waneman.get(SIGIL) == target {
            converged_at = Some(std::time::Instant::now());
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(
        converged_at.is_some(),
        "wane should converge within {CONVERGE_BY:?} of heal: a={}, b={}",
        a.waneman.get(SIGIL),
        b.waneman.get(SIGIL),
    );
    // B advanced from its own local 4 to A's 8. That number could only
    // have come off the wire, so this is the positive proof that a
    // wane-sync tone actually crossed rather than the two trackers
    // agreeing by accident.
    assert_eq!(a.waneman.get(SIGIL), 8, "a kept the higher tip");
    assert_eq!(b.waneman.get(SIGIL), 8, "b adopted a's tip over the wire");
    assert_eq!(a.waneman.get(SIGIL), b.waneman.get(SIGIL));
    assert_eq!(a.ensemble.inbox_dropped(), 0, "a dropped a drained tone");
    assert_eq!(b.ensemble.inbox_dropped(), 0, "b dropped a drained tone");

    sim.shutdown().await;
}

/// The negative control. An outage that is never healed must not
/// reconcile on its own, no matter how long it is held.
///
/// Without this, a test suite that always heals first cannot tell the
/// difference between "the heal worked" and "these two numbers converge
/// anyway" — which is the same confusion the mid-outage assertion above
/// exists to prevent, one dimension up.
#[tokio::test(flavor = "multi_thread")]
async fn an_unhealed_partition_never_reconverges() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = Sim::new();
    let a_id = Hid::random_humd();
    let b_id = Hid::random_humd();
    let a = sim.spawn_humd(a_id).await;
    let b = sim.spawn_humd(b_id).await;

    sim.await_ready(a_id).await.expect("a ready");
    sim.await_ready(b_id).await.expect("b ready");
    sim.wire(a_id, b_id).expect("wire a-b");
    sim.partition(a_id, b_id).expect("partition");

    a.waneman.tick(SIGIL);
    for _ in 0..7 {
        b.waneman.tick(SIGIL);
    }

    // Same point as above, in the negative control: an unhealed partition
    // has to refuse real traffic, not merely sit there holding two
    // numbers apart.
    sim.wane_sync(&b, &a).await.expect("b emits into the outage");

    let until = std::time::Instant::now() + OUTAGE * 2;
    while std::time::Instant::now() < until {
        assert_eq!(
            (a.waneman.get(SIGIL), b.waneman.get(SIGIL)),
            (1, 7),
            "an unhealed partition must stay divergent, whatever the wall clock says"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    sim.shutdown().await;
}

/// The merge is a max, so the *lower* side must adopt the higher tip
/// without dragging the other one down. A min-merge satisfies
/// "they are equal" and passes a naive convergence assertion while
/// silently rewinding wane — which for a Lamport clock means re-running
/// history.
#[tokio::test(flavor = "multi_thread")]
async fn heal_takes_the_higher_tip_and_never_rewinds() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = Sim::new();
    let a_id = Hid::random_humd();
    let b_id = Hid::random_humd();
    let a = sim.spawn_humd(a_id).await;
    let b = sim.spawn_humd(b_id).await;

    sim.await_ready(a_id).await.expect("a ready");
    sim.await_ready(b_id).await.expect("b ready");
    sim.wire(a_id, b_id).expect("wire a-b");

    // B is far ahead. A holds a second sigil that B has never heard of,
    // so the join has to be per-sigil rather than a wholesale overwrite.
    for _ in 0..9 {
        b.waneman.tick(SIGIL);
    }
    b.waneman.tick("sigil-only-b");
    for _ in 0..2 {
        a.waneman.tick("sigil-only-a");
    }
    a.waneman.tick(SIGIL);
    sim.partition(a_id, b_id).expect("partition");
    sim.heal(a_id, b_id).await.expect("heal");

    let deadline = std::time::Instant::now() + CONVERGE_BY;
    while std::time::Instant::now() < deadline {
        if a.waneman.get(SIGIL) == 9 && b.waneman.get(SIGIL) == 9 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(a.waneman.get(SIGIL), 9, "A must take B's higher tip");
    assert_eq!(b.waneman.get(SIGIL), 9, "B must not rewind to A's lower tip");
    assert_eq!(
        (a.waneman.get("sigil-only-a"), b.waneman.get("sigil-only-a")),
        (2, 2),
        "a sigil only A has must survive the join"
    );
    assert_eq!(
        (a.waneman.get("sigil-only-b"), b.waneman.get("sigil-only-b")),
        (1, 1),
        "a sigil only B has must reach A"
    );

    sim.shutdown().await;
}
