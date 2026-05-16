//! Federation handoff: two humds owned by different operators (T3 tier)
//! announce themselves with Ed25519-signed hellos. The good case proves
//! both sides verify the other's signature and learn its caps; the bad
//! case proves a tampered hello (pubkey doesn't hash to the claimed
//! humd_id) is rejected — the peer never enters the registry.
//!
//! Narrative: org-A runs humd-A, org-B runs humd-B. On peer-add each
//! ships its identity, signed under its own key. Each side verifies the
//! signature against the claimed pubkey and rejects mismatched /
//! unsigned hellos.
//!
//! This validates the cryptographic seam at the bottom of the
//! federation-handoff scenario (`scenarios/federation-handoff.md`).
//! Grant tokens, scoped capabilities, and revocation live one layer up
//! and are exercised by separate tests — this one is *only* about
//! "does the peer prove it owns its pubkey?"

use std::time::Duration;

use ensemble::HumdKey;

/// Poll up to ~500ms for `cond` to hold. Avoids racy `sleep(50ms)`
/// fixed waits — the handshake is async (spawned drainer tasks) but
/// usually completes in <5ms.
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

    // Caps actually came from the hello (not the transport stub) — the
    // learned caps include `claude-cli` in nests because that's what
    // `wire_signed` advertises.
    let b_caps_on_a = a.ensemble.peer_caps(&b.id).expect("b in A's registry");
    assert!(
        b_caps_on_a.nests.iter().any(|n| n == "claude-cli"),
        "verified hello must surface nests in learned_caps, got {:?}",
        b_caps_on_a.nests
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

    // C announces a tampered hello — pubkey doesn't hash to c.id. A is
    // strict-auth and must eject C from its registry once the drainer
    // sees the bad hello.
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
