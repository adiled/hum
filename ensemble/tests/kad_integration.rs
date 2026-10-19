//! End-to-end test of the homegrown Kademlia DHT lookup.
//!
//! Five humds in a line, A↔B↔C↔D↔E. A knows only B; E knows only D.
//! A calls `kad_find(E.hid, 2s)` and must end up with E's HumdAddr.
//!
//! Homegrown kad can only query peers it has a live `PeerConnection`
//! to, so the test drives the expansion explicitly: each round, if the
//! lookup did not converge, A installs a link to the closest peer its
//! routing table now knows. Real T4 would do this through the
//! `Transport` trait's `connect()`. That is the honest gap noted in the
//! kad module header.
//!
//! The line topology guarantees B's routing table holds only A and C,
//! so when A asks B for "closest to E", B returns at most those two and
//! the lookup has to percolate through the chain.

use std::time::Duration;

use ensemble::{Ensemble, HumdAddr, Hid, HumdKey, InMemoryEndpoint, PeerCapabilities};

/// Install an `InMemoryEndpoint` pair into both ensembles.
fn wire(ens_a: &Ensemble, a_key: &HumdKey, ens_b: &Ensemble, b_key: &HumdKey) {
    let caps = PeerCapabilities {
        proto_version: "0.7.0".into(),
        ..Default::default()
    };
    let (a_side, b_side) = InMemoryEndpoint::pair(
        ens_a.me(),
        caps.clone(),
        ens_b.me(),
        caps.clone(),
    );
    ens_a.install(a_side, caps.clone(), a_key);
    ens_b.install(b_side, caps, b_key);
}

#[tokio::test]
async fn kad_find_locates_distant_humd_via_intermediaries() {
    let a_key = HumdKey::generate();
    let b_key = HumdKey::generate();
    let c_key = HumdKey::generate();
    let d_key = HumdKey::generate();
    let e_key = HumdKey::generate();
    let a_id = a_key.hid();
    let b_id = b_key.hid();
    let c_id = c_key.hid();
    let d_id = d_key.hid();
    let e_id = e_key.hid();

    let ens_a = Ensemble::new(a_id);
    let ens_b = Ensemble::new(b_id);
    let ens_c = Ensemble::new(c_id);
    let ens_d = Ensemble::new(d_id);
    let ens_e = Ensemble::new(e_id);

    wire(&ens_a, &a_key, &ens_b, &b_key);
    wire(&ens_b, &b_key, &ens_c, &c_key);
    wire(&ens_c, &c_key, &ens_d, &d_key);
    wire(&ens_d, &d_key, &ens_e, &e_key);

    // The drainers need a beat to clear the handshake hellos, so the kad
    // FIND_NODE traffic isn't interleaved with first-hello absorption.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let a_peers = ens_a.peers();
    assert!(a_peers.contains(&b_id), "A should know B");
    assert!(!a_peers.contains(&e_id), "A should NOT directly know E");
    let e_peers = ens_e.peers();
    assert!(e_peers.contains(&d_id), "E should know D");
    assert!(!e_peers.contains(&a_id), "E should NOT directly know A");

    // A asks B for E. B's routing table holds {A, C}, so B returns its K
    // closest to E — at most those two. A's shortlist grows, but A has no
    // PeerConnection to C yet, so the round yields no new information
    // and the lookup stalls until the expansion below installs a link.
    let ensembles_by_id = [
        (b_id, &ens_b, &b_key),
        (c_id, &ens_c, &c_key),
        (d_id, &ens_d, &d_key),
        (e_id, &ens_e, &e_key),
    ];
    let resolve_ensemble = |id: Hid| -> Option<(&Ensemble, &HumdKey)> {
        ensembles_by_id
            .iter()
            .find(|(pid, _, _)| *pid == id)
            .map(|(_, e, k)| (*e, *k))
    };

    // Drive up to 6 expand-and-query rounds: run kad_find, and if it did
    // not converge, install a link to the closest known peer toward E that
    // A has not already wired.
    let mut found: Option<HumdAddr> = None;
    for _round in 0..6 {
        let outcome = tokio::time::timeout(
            Duration::from_millis(500),
            ens_a.kad_find(e_id, Duration::from_millis(400)),
        )
        .await
        .expect("kad_find timed out at the wrapper level");
        if let Some(addr) = outcome {
            found = Some(addr);
            break;
        }
        let installed: std::collections::HashSet<Hid> =
            ens_a.peers().into_iter().collect();
        let candidates = ens_a.kad_closest(&e_id, 32);
        let next = candidates
            .into_iter()
            .find(|a| !installed.contains(&a.id) && a.id != a_id);
        let Some(next) = next else { break };
        let Some((target_ens, target_key)) = resolve_ensemble(next.id) else {
            break;
        };
        wire(&ens_a, &a_key, target_ens, target_key);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    let addr = found.expect("kad_find should have located E via iterative FIND_NODE");
    assert_eq!(addr.id, e_id, "kad_find returned the wrong Hid");
}
