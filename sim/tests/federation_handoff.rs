//! Federation handoff.
//!
//! The prose spec is [`scenarios/federation-handoff.md`](../../../scenarios/federation-handoff.md).

use std::time::Duration;

use ensemble::HumdKey;

async fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn good_signature_admits_peer() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();

    let a = sim.spawn_humd_with_identity(HumdKey::generate()).await;
    let b = sim.spawn_humd_with_identity(HumdKey::generate()).await;

    sim.wire_signed(a.id, b.id).expect("signed wire A↔B");

    let a_id = a.id;
    let b_id = b.id;
    let a_ens = a.ensemble.clone();
    let b_ens = b.ensemble.clone();

    let admitted = wait_for(|| {
        a_ens.peer_caps(&b_id).is_some_and(|c| !c.proto_version.is_empty())
            && b_ens.peer_caps(&a_id).is_some_and(|c| !c.proto_version.is_empty())
    })
    .await;
    assert!(admitted, "both ensembles must learn each other's caps from verified hellos");

    let b_caps_on_a = a.ensemble.peer_caps(&b.id).expect("b in A's registry");
    assert_eq!(
        b_caps_on_a.proto_version,
        thrum_core::THRUM_VERSION,
        "a verified hello must surface its claims in learned_caps, got {b_caps_on_a:?}"
    );

    assert!(a.ensemble.peers().contains(&b.id), "A's peers includes B");
    assert!(b.ensemble.peers().contains(&a.id), "B's peers includes A");
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_signature_rejects_peer() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();

    let a = sim.spawn_humd_with_identity(HumdKey::generate()).await;
    let c = sim.spawn_humd_with_identity(HumdKey::generate()).await;

    sim.wire_signed_tampered(a.id, c.id).expect("tampered wire A↔C");

    let a_ens = a.ensemble.clone();
    let c_id = c.id;
    let ejected = wait_for(|| !a_ens.peers().contains(&c_id)).await;

    assert!(
        ejected,
        "A must reject C's tampered hello — peers={:?}",
        a.ensemble.peers().iter().map(|p| p.short()).collect::<Vec<_>>()
    );
    assert!(
        a.ensemble.peer_caps(&c.id).is_none(),
        "A learns no caps for C — tampered hello"
    );
}
