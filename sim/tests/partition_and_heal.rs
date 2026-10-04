
use std::time::Duration;

use ensemble::Hid;
use sim::Sim;

const SIGIL: &str = "test-sigil";
const OUTAGE: Duration = Duration::from_millis(600);
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

    for _ in 0..3 {
        a.waneman.tick(SIGIL);
        b.waneman.tick(SIGIL);
    }
    assert_eq!(a.waneman.get(SIGIL), 3);
    assert_eq!(b.waneman.get(SIGIL), 3);

    sim.partition(a_id, b_id).expect("partition");

    for _ in 0..5 {
        a.waneman.tick(SIGIL);
    }
    b.waneman.tick(SIGIL);

    assert_eq!(a.waneman.get(SIGIL), 8, "a kept advancing locally");
    assert_eq!(b.waneman.get(SIGIL), 4, "b ticked once during outage");

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
    assert_eq!(a.waneman.get(SIGIL), 8, "a kept the higher tip");
    assert_eq!(b.waneman.get(SIGIL), 8, "b adopted a's tip over the wire");
    assert_eq!(a.waneman.get(SIGIL), b.waneman.get(SIGIL));
    assert_eq!(a.ensemble.inbox_dropped(), 0, "a dropped a drained tone");
    assert_eq!(b.ensemble.inbox_dropped(), 0, "b dropped a drained tone");

    sim.shutdown().await;
}

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
