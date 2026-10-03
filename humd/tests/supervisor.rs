//! supervisor — eviction and redial, as one decision.
//!
//! `ensemble::liveness` proves the ensemble detects a dead peer. This
//! proves the daemon *acts* on it: evicts, then dials again, and
//! remembers a peer's failure so a peer that stays down doesn't turn
//! into a dial spin.
//!
//! Driving `Supervisor::tick` directly is deliberate. The production
//! loop runs on a 10s interval, and a test that waits on a real
//! interval is a test that either takes ten seconds or gets skipped.

use std::sync::Arc;
use std::time::Duration;

use ensemble::{Ensemble, Hid, HumdKey, PeerCapabilities};
use humd::peers::PeerConfig;
use humd::redial::Backoff;
use humd::supervisor::{LivenessConfig, Supervisor};

/// No transports configured, so a dial cannot succeed — which is the
/// case that matters here: a peer that stays down must back off rather
/// than spin.
fn supervisor_for(ens: Arc<Ensemble>, peer: Hid) -> Supervisor {
    Supervisor::new(
        ens,
        Arc::new(HumdKey::generate()),
        Arc::new(vec![PeerConfig {
            humd_id: peer,
            hints: vec![],
            alias: None,
        }]),
        PeerCapabilities::default(),
        None,
        LivenessConfig::default(),
    )
}

/// An endpoint claiming `peer` as its peer. The far side is returned
/// too and must be held: dropping it closes the channel, which is
/// exactly the death these tests are about.
fn endpoint(
    peer: Hid,
) -> (
    Arc<dyn ensemble::PeerConnection>,
    Arc<dyn ensemble::PeerConnection>,
) {
    ensemble::InMemoryEndpoint::pair(
        Hid::random_humd(),
        PeerCapabilities::default(),
        peer,
        PeerCapabilities::default(),
    )
}

/// An unproven peer is Live: no traffic and no reported close is not
/// evidence of death, and reaping it would evict every peer at boot.
#[tokio::test(flavor = "multi_thread")]
async fn an_unproven_peer_is_not_reaped() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    let (mine, _theirs) = endpoint(peer);
    ens.add_peer(mine);
    let mut sup = supervisor_for(ens.clone(), peer);

    let report = sup.tick().await;
    assert!(
        report.evicted.is_empty(),
        "unproven peer must survive a sweep"
    );
    assert!(ens.peers().contains(&peer));
}

/// The core decision: a dead peer is evicted and its redial is
/// attempted in the same pass.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_peer_is_evicted_and_redial_attempted() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    let (mine, _theirs) = endpoint(peer);
    ens.add_peer(mine);
    ens.expire_peer(&peer);
    let mut sup = supervisor_for(ens.clone(), peer);

    let report = sup.tick().await;
    assert_eq!(report.evicted, vec![peer], "the dead peer is reaped");
    assert_eq!(report.dial_failed, vec![peer], "and redialled this pass");
    assert!(!ens.peers().contains(&peer), "and gone from the registry");
}

/// Backoff is the difference between a peer being down and a peer being
/// a busy loop.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_stays_down_backs_off() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    let (mine, _theirs) = endpoint(peer);
    ens.add_peer(mine);
    ens.expire_peer(&peer);
    let mut sup = supervisor_for(ens.clone(), peer);

    let first = sup.tick().await;
    assert_eq!(first.dial_failed, vec![peer], "first attempt is made");
    assert_eq!(sup.attempts(&peer), 1);

    // The peer is already evicted, so the next pass has nothing to reap —
    // and must not dial again inside the backoff window.
    let second = sup.tick().await;
    assert!(second.evicted.is_empty());
    assert!(
        second.dial_failed.is_empty() && second.redialed.is_empty(),
        "must not redial while inside the backoff window"
    );
}

/// The probe must reach peers that are still installed, and must not
/// resurrect an evicted one.
#[tokio::test(flavor = "multi_thread")]
async fn tick_probes_before_sweeping() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    let (mine, _theirs) = endpoint(peer);
    ens.add_peer(mine);
    let mut sup = supervisor_for(ens.clone(), peer);

    sup.tick().await;
    // The probe landed, so the peer's lease now has traffic on it.
    assert_eq!(
        ens.peer_liveness(&peer, LivenessConfig::default().ttl),
        Some(ensemble::Liveness::Live)
    );
}

/// Backoff resets on success, so a peer that flaps doesn't accumulate
/// delay forever.
#[test]
fn a_recovered_peer_resets_its_backoff() {
    let mut b = Backoff::new(Duration::from_millis(10), Duration::from_secs(60));
    for _ in 0..5 {
        b.fail();
    }
    assert_eq!(b.attempts(), 5);
    b.succeed();
    assert_eq!(b.attempts(), 0, "a reachable peer starts from the bottom");
    assert!(b.ready(), "and may be redialled immediately");
}

/// A healthy peer must survive several sweeps. This is the guard
/// against a config that probes slower than its TTL, which would evict
/// live peers on a timer.
#[tokio::test(flavor = "multi_thread")]
async fn a_healthy_peer_survives_repeated_sweeps() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    let (mine, _theirs) = endpoint(peer);
    ens.add_peer(mine);
    let mut sup = supervisor_for(ens.clone(), peer);

    for _ in 0..5 {
        sup.tick().await;
    }
    assert!(
        ens.peers().contains(&peer),
        "a live peer must never be reaped"
    );
}

#[test]
fn config_probes_far_inside_ttl() {
    let c = LivenessConfig::default();
    assert!(
        c.interval * 3 <= c.ttl,
        "a peer must be probed repeatedly before a sweep can reap it"
    );
}

/// A peer that was down at boot never entered the registry, so no sweep
/// will ever name it. If the redial set were "just evicted", it would
/// never be retried — the boot-time outage would be permanent.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_never_installed_is_still_retried() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    // Note: no add_peer. The peer is configured and simply absent.
    let mut sup = supervisor_for(ens.clone(), peer);

    let report = sup.tick().await;
    assert!(report.evicted.is_empty(), "nothing was installed to reap");
    assert_eq!(report.dial_failed, vec![peer], "but it is still dialled");
    assert_eq!(sup.attempts(&peer), 1);
}
