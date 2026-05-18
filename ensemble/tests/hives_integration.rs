//! Nestling advertise/discover end-to-end over the gossip layer.
//!
//! Three humds in a line (A — B — C). A advertises a `market-maker`
//! bee; C calls `hive_discover("market-maker")` and must
//! receive `(A's HumdId, manifest)` within a short timeout. Then A
//! retracts; via the raw `hive_announcements()` stream, C must
//! see a `Retract` envelope.

use std::time::Duration;

use ensemble::{
    hives::{HiveAnnounce, HiveManifest, Propensity},
    Ensemble, HumdKey, InMemoryEndpoint, PeerCapabilities,
};
use tokio::time::timeout;

#[tokio::test]
async fn advertise_percolates_and_discover_filters_by_name() {
    let a_key = HumdKey::generate();
    let b_key = HumdKey::generate();
    let c_key = HumdKey::generate();
    let a_id = a_key.humd_id();
    let b_id = b_key.humd_id();
    let c_id = c_key.humd_id();

    let caps = PeerCapabilities {
        proto_version: "0.7.0".into(),
        ..Default::default()
    };

    let (a_to_b, b_to_a) = InMemoryEndpoint::pair(a_id, caps.clone(), b_id, caps.clone());
    let (b_to_c, c_to_b) = InMemoryEndpoint::pair(b_id, caps.clone(), c_id, caps.clone());

    let ens_a = Ensemble::new(a_id);
    let ens_b = Ensemble::new(b_id);
    let ens_c = Ensemble::new(c_id);

    ens_a.install(a_to_b, caps.clone(), &a_key);
    ens_b.install(b_to_a, caps.clone(), &b_key);
    ens_b.install(b_to_c, caps.clone(), &b_key);
    ens_c.install(c_to_b, caps.clone(), &c_key);

    let mut discover = ens_c.hive_discover("market-maker");
    let mut raw = ens_c.hive_announcements();

    // Wait for handshakes + topic subscription to wire up before A advertises.
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Noise: an unrelated bee — must not appear in the filtered receiver.
    ens_a
        .hive_advertise(HiveManifest::new("openai-server", "0.2.0", "0.7.0"))
        .await;

    // Real advertise.
    let manifest = HiveManifest::new("market-maker", "0.1.0", "0.7.0")
        .with_propensity(Propensity {
            statefulness: Some("stateless".into()),
            richness: Some("medium".into()),
            wire: Some("custom/mm-v0".into()),
        })
        .with_chis(["hello", "gossip-publish", "tool-call", "tool-result"]);
    ens_a.hive_advertise(manifest.clone()).await;

    let (got_id, got_manifest) = timeout(Duration::from_millis(300), discover.recv())
        .await
        .expect("C did not receive bee-advertise within 300ms")
        .expect("discover channel closed");
    assert_eq!(got_id, a_id, "advertise came from the wrong humd");
    assert_eq!(got_manifest.name, "market-maker");
    assert_eq!(got_manifest.version, "0.1.0");
    assert_eq!(got_manifest.chis.len(), 4);

    // Retract on the raw stream.
    ens_a.hive_retract("market-maker").await;
    let env = timeout(Duration::from_millis(300), async {
        // We may see the two advertises first (one openai-server, one
        // market-maker) before the retract; drain until we hit Retract.
        loop {
            let v = raw.recv().await.expect("raw channel closed");
            if matches!(v, HiveAnnounce::Retract { .. }) {
                return v;
            }
        }
    })
    .await
    .expect("C did not see retract within 300ms");

    match env {
        HiveAnnounce::Retract { humd_id, name } => {
            assert_eq!(humd_id, a_id.to_hex());
            assert_eq!(name, "market-maker");
        }
        _ => panic!("expected retract"),
    }
}
