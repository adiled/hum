
use std::time::Duration;

use ed25519_dalek::SigningKey;
use ensemble::{
    Ensemble, HumdAddr, Hid, HumdKey, IrohTransport, PeerCapabilities, PeerConnection, Transport,
};
use serde_json::json;

const REQUIRE_ENV: &str = "HUM_REQUIRE_TRANSPORT_TESTS";

enum Skip {
    Environmental(String),
    Real(String),
}

impl Skip {
    fn classify(what: &str, err: impl std::fmt::Display) -> Self {
        let text = err.to_string().to_lowercase();
        if text.contains("permission denied") {
            return Skip::Environmental(format!("{what}: permission denied ({err})"));
        }
        if text.contains("network is unreachable")
            || text.contains("no such device")
            || text.contains("cannot assign requested address")
            || text.contains("no route to host")
        {
            return Skip::Environmental(format!("{what}: no usable network ({err})"));
        }
        if text.contains("address in use") {
            return Skip::Environmental(format!("{what}: address in use ({err})"));
        }
        if text.contains("no process-level CryptoProvider available")
            || text.contains("cryptoprovider")
        {
            return Skip::Environmental(format!("{what}: no crypto provider ({err})"));
        }
        Skip::Real(format!("{what}: {err}"))
    }

    fn resolve(self) -> Option<()> {
        let forced = std::env::var(REQUIRE_ENV).is_ok_and(|v| v != "0" && !v.is_empty());
        match self {
            Skip::Environmental(reason) if forced => panic!(
                "{REQUIRE_ENV} is set, so this failure cannot be skipped: {reason}"
            ),
            Skip::Environmental(reason) => {
                eprintln!("iroh_integration: SKIPPED — {reason}");
                eprintln!(
                    "iroh_integration: this run proved nothing about the iroh \
                     transport. Set {REQUIRE_ENV}=1 in a job with UDP to enforce it."
                );
                None
            }
            Skip::Real(reason) => panic!("iroh_integration: {reason}"),
        }
    }
}

fn force_transport_tests(required: bool) {
    if required {
        unsafe { std::env::set_var(REQUIRE_ENV, "1") };
    } else {
        unsafe { std::env::remove_var(REQUIRE_ENV) };
    }
}

async fn try_bind() -> Option<IrohTransport> {
    match IrohTransport::bind_direct().await {
        Ok(t) => Some(t),
        Err(e) => {
            Skip::classify("bind", e).resolve();
            None
        }
    }
}

#[tokio::test]
async fn iroh_endpoint_routes_tones_both_ways() {
    let Some(server) = try_bind().await else {
        return;
    };
    let Some(client) = try_bind().await else {
        return;
    };

    let server_node_id = server.node_id();
    let server_humd_id = Hid::from_pubkey(ensemble::HidPrefix::Humd, server_node_id.as_bytes());
    let client_node_id = client.node_id();
    let client_humd_id = Hid::from_pubkey(ensemble::HidPrefix::Humd, client_node_id.as_bytes());
    let server_sockets: Vec<String> = server.dial_hints();
    assert!(
        !server_sockets.is_empty(),
        "iroh server endpoint reported no bound sockets"
    );
    for hint in &server_sockets {
        let addr = hint
            .strip_prefix(ensemble::IROH_IP_HINT)
            .unwrap_or_else(|| panic!("hint {hint} lost its prefix"));
        let parsed: std::net::SocketAddr = addr
            .parse()
            .unwrap_or_else(|e| panic!("hint {hint} is not a SocketAddr: {e}"));
        assert!(
            !parsed.ip().is_unspecified(),
            "dial hint {hint} is a wildcard address and can never be dialled"
        );
    }

    let server_key = HumdKey(SigningKey::from_bytes(&server.endpoint().secret_key().to_bytes()));
    let client_key = HumdKey(SigningKey::from_bytes(&client.endpoint().secret_key().to_bytes()));
    assert_eq!(server_key.hid(), server_humd_id);
    assert_eq!(client_key.hid(), client_humd_id);

    let server_humd_for_task = server_humd_id;
    let client_humd_for_task = client_humd_id;
    let server = std::sync::Arc::new(server);
    let server_clone = server.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let endpoint = match server_clone.accept().await {
            Ok(ep) => ep,
            Err(e) => return Err(format!("server accept: {e}")),
        };

        assert_eq!(endpoint.peer().id, client_humd_for_task);

        let ensemble = Ensemble::new(server_humd_for_task);
        let mut sub = ensemble.subscribe();
        ensemble.install(endpoint.clone(), PeerCapabilities::default(), &server_key);

        let got = tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .map_err(|_| "server: recv timed out".to_string())?
            .map_err(|e| format!("server: recv: {e}"))?;
        if got.get("chi").and_then(|v| v.as_str()) != Some("perf-mark") {
            return Err(format!("server: expected perf-mark, got {got:?}"));
        }

        let pong = json!({
            "chi": "ping",
            "rid": "iroh-pong-1",
            "to": client_humd_for_task.to_hex(),
        });
        ensemble
            .route(pong)
            .await
            .map_err(|e| format!("server: route: {e}"))?;

        let _ = done_rx.await;
        Ok::<(), String>(())
    });

    let mut server_humd_addr = HumdAddr::new(server_humd_id).with_hint(format!(
        "iroh:{}",
        hex::encode(server_node_id.as_bytes())
    ));
    for hint in server_sockets {
        server_humd_addr = server_humd_addr.with_hint(hint);
    }
    let conn = match client.connect(&server_humd_addr).await {
        Ok(c) => c,
        Err(e) => {
            Skip::classify("connect", e).resolve();
            return;
        }
    };

    assert_eq!(conn.peer().id, server_humd_id);
    let ensemble = Ensemble::new(client_humd_id);
    let mut sub = ensemble.subscribe();
    ensemble.install(conn, PeerCapabilities::default(), &client_key);

    let mark = json!({
        "chi": "perf-mark",
        "rid": "iroh-mark-1",
        "to": server_humd_id.to_hex(),
    });
    ensemble
        .route(mark)
        .await
        .expect("route perf-mark");

    let mut server_task = server_task;
    let got = tokio::select! {
        biased;
        srv = &mut server_task => {
            srv.expect("server task panicked")
                .expect("server task error");
            tokio::time::timeout(Duration::from_secs(2), sub.recv())
                .await
                .expect("client recv timed out after server done")
                .expect("client recv closed")
        }
        got = tokio::time::timeout(Duration::from_secs(10), sub.recv()) => {
            got.expect("client recv timed out")
                .expect("client recv closed")
        }
    };
    assert_eq!(got.get("chi").and_then(|v| v.as_str()), Some("ping"));
    assert_eq!(got.get("rid").and_then(|v| v.as_str()), Some("iroh-pong-1"));

    let _ = done_tx.send(());
    let server_result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task timed out");
    server_result
        .expect("server task panicked")
        .expect("server task error");

    drop(server);
}

mod classify {
    use super::{force_transport_tests, Skip, REQUIRE_ENV};

    #[test]
    fn sandbox_signatures_are_environmental() {
        for msg in [
            "iroh bind: bind: Permission denied (os error 13)",
            "iroh bind: Address in use (os error 48)",
            "connect: network is unreachable",
            "bind: no such device",
            "bind: cannot assign requested address",
        ] {
            assert!(
                matches!(Skip::classify("bind", msg), Skip::Environmental(_)),
                "{msg:?} should be environmental"
            );
        }
    }

    #[test]
    fn anything_unrecognised_is_a_real_failure() {
        for msg in [
            "iroh bind: quic handshake failed",
            "server accept: connection reset by peer",
            "no secret found for the relay",
            "",
        ] {
            assert!(
                matches!(Skip::classify("bind", msg), Skip::Real(_)),
                "{msg:?} must not be forgiven"
            );
        }
    }

    #[test]
    fn a_real_failure_panics_even_without_the_env_var() {
        let r = std::panic::catch_unwind(|| {
            Skip::Real("bind: quic handshake failed".into()).resolve()
        });
        assert!(r.is_err(), "a real failure must never resolve to a skip");
    }

    #[test]
    fn requiring_transport_tests_turns_a_skip_into_a_panic() {
        force_transport_tests(true);
        let r = std::panic::catch_unwind(|| {
            Skip::Environmental("bind: permission denied".into()).resolve()
        });
        force_transport_tests(false);
        assert!(
            r.is_err(),
            "{REQUIRE_ENV} must make even an environmental skip fail"
        );
    }
}
