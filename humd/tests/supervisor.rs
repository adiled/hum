
use std::sync::Arc;
use std::time::Duration;

use ensemble::{Ensemble, Hid, HumdKey, PeerCapabilities};
use humd::peers::PeerConfig;
use humd::redial::Backoff;
use humd::supervisor::{LivenessConfig, Supervisor};

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
    assert_eq!(sup.consecutive_failures(&peer), 1);

    let second = sup.tick().await;
    assert!(second.evicted.is_empty());
    assert!(
        second.dial_failed.is_empty() && second.redialed.is_empty(),
        "must not redial while inside the backoff window"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tick_probes_before_sweeping() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    let (mine, _theirs) = endpoint(peer);
    ens.add_peer(mine);
    let mut sup = supervisor_for(ens.clone(), peer);

    sup.tick().await;
    assert_eq!(
        ens.peer_liveness(&peer, LivenessConfig::default().ttl),
        Some(ensemble::Liveness::Live)
    );
}

#[test]
fn a_recovered_peer_resets_its_backoff() {
    let mut b = Backoff::new(Duration::from_millis(10), Duration::from_secs(60));
    for _ in 0..5 {
        b.fail();
    }
    assert_eq!(b.failures(), 5);
    b.succeed();
    assert_eq!(b.failures(), 0, "a reachable peer starts from the bottom");
    assert!(b.ready(), "and may be redialled immediately");
}

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

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_never_installed_is_still_retried() {
    let me = Hid::random_humd();
    let peer = Hid::random_humd();
    let ens = Arc::new(Ensemble::new(me));
    let mut sup = supervisor_for(ens.clone(), peer);

    let report = sup.tick().await;
    assert!(report.evicted.is_empty(), "nothing was installed to reap");
    assert_eq!(report.dial_failed, vec![peer], "but it is still dialled");
    assert_eq!(sup.consecutive_failures(&peer), 1);
}
