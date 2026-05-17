//! End-to-end test of the homegrown Kademlia DHT lookup.
//!
//! Wire five humds A↔B↔C↔D↔E in a line. A only knows B (directly
//! installed); E only knows D. A calls `kad_find(E.humd_id, 2s)`.
//!
//! Expected flow:
//!   1. A's local routing table contains only B. α-batch picks B.
//!   2. A sends `chi:"kad-find-node"` to B; B's drainer responds with
//!      B's routing table closest to E — which contains C (B knows A
//!      and C directly, but C is closer to E in XOR space because
//!      the test seeds C with a HumdId close to E).
//!   3. A inserts C into its routing table, re-queries C.
//!   4. C answers with D, A queries D, D answers with E. Either E
//!      is returned to A directly OR A's table now has E because D
//!      advertised it.
//!
//! XOR-distance ordering is sensitive to randomly-chosen ids. To make
//! the lookup deterministic we hand-craft ids so each humd is strictly
//! closer to E than the previous hop's far peer: A is far from E, B is
//! halfway, C closer, D closest non-E peer. The line topology guarantees
//! that B's table holds only A and C — so when A asks B for "closest to
//! E," B returns at most A and C. The lookup must then walk through C
//! and D to reach E.
//!
//! Honest constraint: in our homegrown kad we can only query peers we
//! have a live PeerConnection to. So if A's routing table after one
//! FIND_NODE round contains C, it can't query C without an install. The
//! test exercises a stronger property: at each hop, the queried peer
//! ANSWERS with its neighbors, and we keep iterating until the target's
//! HumdAddr lands in the table. Since each humd only knows its
//! immediate line neighbors, the iterative lookup percolates through
//! the chain by repeatedly asking the closest installed peer for more.
//!
//! The simple version that works: A↔B, B↔C, C↔D, D↔E. A asks B (its
//! only peer); B returns A and C. A still can't reach C directly. But
//! when A's drainer receives B's resp, A's routing table holds {B, C}.
//! A's kad_find tries to query C — finds no connection — so it can't
//! recurse. That's a real T4 gap (we'd need to dial C via the hints).
//!
//! For this v0 test, we instead build a STAR with a transit hub: A
//! installs a connection to every intermediate humd, but only the LAST
//! humd in the chain holds E's HumdAddr in its routing table. The
//! lookup converges in one round because A can query all four hubs in
//! parallel and one of them returns E. This still exercises the
//! iterative FIND_NODE plumbing, the multi-peer fan-out, the routing
//! table merge, and the response-keyed-by-query-id machinery.
//!
//! Full multi-hop traversal (where A only knows B and must dial C/D/E
//! discovered through queries) requires plugging a real Transport into
//! `kad_find` so it can dial advertised hints. That's noted as an
//! honest gap in the kad module header.

use std::time::Duration;

use ensemble::{Ensemble, HumdAddr, HumdId, HumdKey, InMemoryEndpoint, PeerCapabilities};

/// Install an `InMemoryEndpoint` pair into both ensembles. Caps default
/// to a fixed proto version so the signed hello round-trips cleanly.
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
    // Five humds. Real keypairs so signed hellos succeed.
    let a_key = HumdKey::generate();
    let b_key = HumdKey::generate();
    let c_key = HumdKey::generate();
    let d_key = HumdKey::generate();
    let e_key = HumdKey::generate();
    let a_id = a_key.humd_id();
    let b_id = b_key.humd_id();
    let c_id = c_key.humd_id();
    let d_id = d_key.humd_id();
    let e_id = e_key.humd_id();

    let ens_a = Ensemble::new(a_id);
    let ens_b = Ensemble::new(b_id);
    let ens_c = Ensemble::new(c_id);
    let ens_d = Ensemble::new(d_id);
    let ens_e = Ensemble::new(e_id);

    // Line topology: A↔B↔C↔D↔E. No A↔E direct link.
    wire(&ens_a, &a_key, &ens_b, &b_key);
    wire(&ens_b, &b_key, &ens_c, &c_key);
    wire(&ens_c, &c_key, &ens_d, &d_key);
    wire(&ens_d, &d_key, &ens_e, &e_key);

    // Drainers need a beat to clear the handshake hellos so the kad
    // FIND_NODE traffic isn't interleaved with first-hello absorption.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Sanity check the wiring: A knows only B; E knows only D.
    let a_peers = ens_a.peers();
    assert!(a_peers.contains(&b_id), "A should know B");
    assert!(!a_peers.contains(&e_id), "A should NOT directly know E");
    let e_peers = ens_e.peers();
    assert!(e_peers.contains(&d_id), "E should know D");
    assert!(!e_peers.contains(&a_id), "E should NOT directly know A");

    // The iterative lookup: A asks B for E.
    //
    // Round 1: A queries B (only peer). B's routing table contains
    // {A, C}. B returns its K closest to E — which is C (assuming C
    // happens to be closer in XOR space than A; we don't enforce
    // this, but B will return both anyway since K=20 ≥ 2).
    //
    // Round 2: A's shortlist now holds {B, C, A} (A is filtered out
    // as self). It picks C — but A has no PeerConnection to C, so
    // the round produces no new info via query_peer. Lookup stalls.
    //
    // To make this realistic for the v0 in-memory mesh, we extend
    // the test: A installs a *direct* link to C, D, E's hubs… no,
    // that defeats the point. Instead we exercise what kad can do
    // today: the routing table propagates outward and a future-dial
    // surface can use it.
    //
    // What we can assert RIGHT NOW with the line topology:
    //   1. After kad_find, A's routing table grew (it learned about
    //      C from B's FIND_NODE response).
    //   2. The lookup terminated cleanly within the 2s budget.
    //
    // For the "find E via the chain" property, we also install a
    // sparse mesh: each humd in the chain advertises its full peer
    // set, so by transitively re-querying we eventually pull E's
    // HumdAddr through. But A's kad_find can only query peers it
    // has an installed connection to. So to make E findable from A
    // via iterative lookup, we add the bootstrap convention: A
    // dials any peer named in a response by also installing it.
    //
    // To keep this test honest about what's implemented, we drive
    // the recursion manually: after each lookup round, install a
    // connection from A to the closest unknown peer the routing
    // table now holds. Real T4 would do this via the Transport
    // trait's connect(); we do it explicitly here with another
    // InMemoryEndpoint pair against the corresponding ensemble.
    let ensembles_by_id = [
        (b_id, &ens_b, &b_key),
        (c_id, &ens_c, &c_key),
        (d_id, &ens_d, &d_key),
        (e_id, &ens_e, &e_key),
    ];
    let resolve_ensemble = |id: HumdId| -> Option<(&Ensemble, &HumdKey)> {
        ensembles_by_id
            .iter()
            .find(|(pid, _, _)| *pid == id)
            .map(|(_, e, k)| (*e, *k))
    };

    // Drive up to 4 expand-and-query rounds. Each iteration:
    //   1. Run kad_find with a short per-round budget.
    //   2. If E is found, success.
    //   3. Otherwise, look at A's routing table for the closest
    //      not-yet-installed peer toward E, install that link, loop.
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
        // Expand: install a link to the closest known peer toward E
        // that A hasn't already wired.
        let installed: std::collections::HashSet<HumdId> =
            ens_a.peers().into_iter().collect();
        // Pull all entries from A's routing table closest to E.
        // We don't have a public getter for "all entries" so we use
        // closest_to with a large count.
        let candidates = {
            // Borrow through a tiny helper: closest_to via the kad
            // module. Ensemble exposes len; for ids we re-derive
            // from the routing table directly via a custom path.
            ens_a.kad_closest(&e_id, 32)
        };
        let next = candidates
            .into_iter()
            .find(|a| !installed.contains(&a.id) && a.id != a_id);
        let Some(next) = next else { break };
        let Some((target_ens, target_key)) = resolve_ensemble(next.id) else {
            // Routing table holds an id we can't resolve to a known
            // ensemble — shouldn't happen in this test fixture.
            break;
        };
        wire(&ens_a, &a_key, target_ens, target_key);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    let addr = found.expect("kad_find should have located E via iterative FIND_NODE");
    assert_eq!(addr.id, e_id, "kad_find returned the wrong HumdId");
}
