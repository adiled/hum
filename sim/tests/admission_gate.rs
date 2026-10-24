use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn a_hello_with_no_proto_version_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());
    assert!(humd.thrum.is_connected(&cid));

    humd.thrum
        .inject_tone(
            &cid,
            humd.signed_hello(serde_json::json!({
                "chi": "hello",
                "roles": ["worker"],
                "hive": "claude-cli",
                "version": "0.1.0",
                "chis": ["hello", "prompt"],
            })),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "a bee declaring no protoVersion must not stay connected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hello_from_another_major_is_disconnected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());
    assert!(humd.thrum.is_connected(&cid));

    humd.thrum
        .inject_tone(
            &cid,
            humd.signed_hello(serde_json::json!({
                "chi": "hello",
                "roles": ["worker"],
                "hive": "claude-cli",
                "version": "0.1.0",
                "protoVersion": "9.0.0",
                "chis": ["hello", "prompt"],
            })),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !humd.thrum.is_connected(&cid),
        "a bee speaking another major must not stay connected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hello_within_the_same_major_stays_connected() {
    let _ = tracing_subscriber::fmt::try_init();
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());

    humd.thrum
        .inject_tone(
            &cid,
            humd.signed_hello(serde_json::json!({
                "chi": "hello",
                "roles": ["worker"],
                "hive": "claude-cli",
                "version": "0.1.0",
                "protoVersion": "0.1.0",
                "chis": ["hello", "prompt"],
            })),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        humd.thrum.is_connected(&cid),
        "drift inside the major must stay admitted"
    );
}
async fn hello_survives(hello: serde_json::Value) -> bool {
    let sim = sim::Sim::new();
    let humd = sim.spawn_humd(ensemble::Hid::random_humd()).await;
    sim.await_ready(humd.id).await.expect("humd ready");

    let cid = hum_identity::HumId::mint().to_string();
    let _rx = humd.thrum.register_synthetic(cid.clone());
    humd.thrum.inject_tone(&cid, humd.signed_hello(hello)).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    humd.thrum.is_connected(&cid)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_of_a_hive_not_installed_here_is_refused() {
    let _ = tracing_subscriber::fmt::try_init();
    assert!(
        !hello_survives(serde_json::json!({
            "chi": "hello",
            "roles": ["worker"],
            "hive": "never-installed",
            "protoVersion": thrum_core::THRUM_VERSION,
        }))
        .await,
        "a bee whose hive this humd was never taught must not stay connected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_that_names_no_hive_is_refused() {
    let _ = tracing_subscriber::fmt::try_init();
    assert!(
        !hello_survives(serde_json::json!({
            "chi": "hello",
            "roles": ["worker"],
            "protoVersion": thrum_core::THRUM_VERSION,
        }))
        .await,
        "a bee belonging to no hive cannot be understood"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bee_of_an_installed_hive_stays_whatever_its_roles() {
    let _ = tracing_subscriber::fmt::try_init();
    assert!(
        hello_survives(serde_json::json!({
            "chi": "hello",
            "roles": ["worker", "forager"],
            "hive": "humfs",
            "protoVersion": thrum_core::THRUM_VERSION,
        }))
        .await
    );
}
