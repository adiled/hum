//! End-to-end test of the iroh transport: spin up two
//! [`IrohTransport`]s in-process on loopback, dial across, run the
//! signed ensemble handshake, and trade tones in both directions.
//!
//! Some CI sandboxes cannot bind UDP sockets or initialise a rustls
//! crypto provider. Those are environmental, not transport bugs — but
//! "the test returned early" is indistinguishable from "the test
//! passed", and a suite that can report green without asserting
//! anything is worse than no suite: it manufactures confidence.
//!
//! So a skip here has to be earned. [`Skip::classify`] only forgives a
//! closed list of environmental signatures and fails everything else,
//! including a bind error nobody has seen before — an unrecognised
//! failure is a bug until proven otherwise. Set
//! `HUM_REQUIRE_TRANSPORT_TESTS=1` to turn even the forgivable cases
//! into hard failures, which is how the job that has UDP should run.

use std::time::Duration;

use ed25519_dalek::SigningKey;
use ensemble::{
    Ensemble, HumdAddr, Hid, HumdKey, IrohTransport, PeerCapabilities, PeerConnection, Transport,
};
use serde_json::json;

/// Set this to make an environmental skip a hard failure. Use it in the
/// job that is supposed to have real UDP, so the suite cannot quietly
/// lose its transport coverage.
const REQUIRE_ENV: &str = "HUM_REQUIRE_TRANSPORT_TESTS";

/// Whether a failure means "this machine can't run it" or "it is broken".
enum Skip {
    /// Environmental. `reason` goes in the test output.
    Environmental(String),
    /// Not environmental. Always a failure.
    Real(String),
}

impl Skip {
    /// Decide, from the error text, whether this is the environment
    /// speaking or the transport breaking.
    ///
    /// Matching on text is unpleasant, but `bind_direct` collapses
    /// everything to `anyhow!("iroh bind: {e}")`, so the chain is gone by
    /// the time it reaches here. The list is deliberately short: every
    /// entry is a reason we have actually seen on a sandbox, and the
    /// fallthrough is a failure. When iroh stops wrapping its errors we
    /// should match on variants instead — [`classify`]'s own tests are
    /// what will tell us this went stale.
    fn classify(what: &str, err: impl std::fmt::Display) -> Self {
        let text = err.to_string().to_lowercase();
        // Permission: a sandbox without network namespace access.
        if text.contains("permission denied") {
            return Skip::Environmental(format!("{what}: permission denied ({err})"));
        }
        // No usable interface at all: no loopback, no network device.
        if text.contains("network is unreachable")
            || text.contains("no such device")
            || text.contains("cannot assign requested address")
            || text.contains("no route to host")
        {
            return Skip::Environmental(format!("{what}: no usable network ({err})"));
        }
        // The port is taken by something else, or a leaked endpoint from a
        // previous run is still bound. Ambiguous: can be environmental
        // and can be our own leak, so it is reported loudly.
        if text.contains("address in use") {
            return Skip::Environmental(format!("{what}: address in use ({err})"));
        }
        // A crypto provider was never installed. Purely environmental —
        // but it means the test asserted nothing, so it is still only
        // allowed when the caller opts out of skips.
        if text.contains("no process-level CryptoProvider available")
            || text.contains("cryptoprovider")
        {
            return Skip::Environmental(format!("{what}: no crypto provider ({err})"));
        }
        Skip::Real(format!("{what}: {err}"))
    }

    /// Turn a skip into either a loud `eprintln!` and `None`, or a panic.
    /// Never a silent `None` with nothing said.
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

/// Try to bind a fresh iroh endpoint, or explain why we cannot.
///
/// Returns `None` only when a skip was forgiven, and `resolve` has
/// already said so on stderr. Anything else panics here.
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

    // Server's NodeId — the client needs this to dial. Iroh-side
    // addressing is the public key; Hid is sha256(pubkey).
    let server_node_id = server.node_id();
    let server_humd_id = Hid::from_pubkey(ensemble::HidPrefix::Humd, server_node_id.as_bytes());
    let client_node_id = client.node_id();
    let client_humd_id = Hid::from_pubkey(ensemble::HidPrefix::Humd, client_node_id.as_bytes());
    // With relay disabled and no DNS lookup configured, the dialer needs
    // explicit IP/port hints. `dial_hints` is not a convenience wrapper
    // around `bound_sockets` — it maps the wildcard bind address to
    // loopback, because a dial to `0.0.0.0` hangs instead of failing and
    // that is what this test was silently skipping over.
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

    // Pin the ensemble's HumdKey to the iroh SecretKey so the signed
    // hello's pubkey hashes back to the iroh-derived Hid — the
    // ensemble drainer checks `sha256(pubkey) == claimed_id` and ejects
    // peers on mismatch. iroh's `SecretKey` is an Ed25519 SigningKey
    // under the hood; just reuse the bytes.
    let server_key = HumdKey(SigningKey::from_bytes(&server.endpoint().secret_key().to_bytes()));
    let client_key = HumdKey(SigningKey::from_bytes(&client.endpoint().secret_key().to_bytes()));
    // Sanity: the derived Hid from HumdKey must match the
    // iroh-derived Hid, otherwise the handshake check below fails.
    assert_eq!(server_key.hid(), server_humd_id);
    assert_eq!(client_key.hid(), client_humd_id);

    // Spawn the server-side accept on a task. It pulls one inbound
    // connection, installs it into a fresh ensemble, and watches for
    // the client's perf-mark tone.
    let server_humd_for_task = server_humd_id;
    let client_humd_for_task = client_humd_id;
    // Wrap server in Arc so the spawned task can hold a clone while the
    // outer test scope retains its own — dropping the IrohTransport
    // closes the iroh::Endpoint and tears the connection down, which
    // would lose any tones still buffered in the QUIC send queue.
    let server = std::sync::Arc::new(server);
    let server_clone = server.clone();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let endpoint = match server_clone.accept().await {
            Ok(ep) => ep,
            Err(e) => return Err(format!("server accept: {e}")),
        };

        // Sanity check: the iroh-derived Hid on the inbound endpoint
        // must match what the outer test thinks the client's id is. If
        // it doesn't, the ensemble drainer will eject the peer when the
        // client's hello arrives and the test will deadlock confusingly.
        assert_eq!(endpoint.peer().id, client_humd_for_task);

        let ensemble = Ensemble::new(server_humd_for_task);
        let mut sub = ensemble.subscribe();
        ensemble.install(endpoint.clone(), PeerCapabilities::default(), &server_key);

        // Receive the client's `perf-mark` tone.
        let got = tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .map_err(|_| "server: recv timed out".to_string())?
            .map_err(|e| format!("server: recv: {e}"))?;
        if got.get("chi").and_then(|v| v.as_str()) != Some("perf-mark") {
            return Err(format!("server: expected perf-mark, got {got:?}"));
        }

        // Reply with a ping addressed to the client's Hid.
        let pong = json!({
            "chi": "ping",
            "rid": "iroh-pong-1",
            "to": client_humd_for_task.to_hex(),
        });
        ensemble
            .route(pong)
            .await
            .map_err(|e| format!("server: route: {e}"))?;

        // Wait for the client to confirm it received the pong before
        // returning — dropping `endpoint` / `ensemble` early would tear
        // the QUIC stream down and the pong could get lost in flight.
        let _ = done_rx.await;
        Ok::<(), String>(())
    });

    // Client side: dial the server by NodeId, install the connection
    // into a fresh ensemble, send a perf-mark tone, await the server's
    // ping on our subscribe channel.
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
            // Same rule as bind: a loopback QUIC handshake that fails is
            // almost always our bug, not the machine's.
            Skip::classify("connect", e).resolve();
            return;
        }
    };

    assert_eq!(conn.peer().id, server_humd_id);
    let ensemble = Ensemble::new(client_humd_id);
    let mut sub = ensemble.subscribe();
    ensemble.install(conn, PeerCapabilities::default(), &client_key);

    // Route a perf-mark tone to the server (addressed by its Hid).
    let mark = json!({
        "chi": "perf-mark",
        "rid": "iroh-mark-1",
        "to": server_humd_id.to_hex(),
    });
    ensemble
        .route(mark)
        .await
        .expect("route perf-mark");

    // Race the client's recv against the server task — if the server
    // fails (e.g. accept errored) we want its real message, not a
    // generic "client recv timed out" that hides the cause.
    let mut server_task = server_task;
    let got = tokio::select! {
        biased;
        srv = &mut server_task => {
            srv.expect("server task panicked")
                .expect("server task error");
            // Server exited before client received — surface whatever
            // is still in the subscribe queue (or fail with a timeout).
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

    // Now release the server task so it can finish + drop cleanly.
    let _ = done_tx.send(());
    let server_result = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server task timed out");
    server_result
        .expect("server task panicked")
        .expect("server task error");

    // Keep the outer `server` Arc alive until here so the underlying
    // iroh endpoint isn't dropped mid-test.
    drop(server);
}

/// The classifier is the thing standing between this file and a suite
/// that forgives everything, so it gets tested like anything else. A
/// blanket `Skip::Environmental` would make every other test in the repo
/// pass, and the only defence is to check the classification directly.
mod classify {
    use super::{Skip, REQUIRE_ENV};

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
        // The important one. An unfamiliar error must not be forgiven
        // just because skipping is convenient.
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
        // SAFETY: single-threaded, and the env is read immediately.
        unsafe { std::env::set_var(REQUIRE_ENV, "1") };
        let r = std::panic::catch_unwind(|| {
            Skip::Environmental("bind: permission denied".into()).resolve()
        });
        unsafe { std::env::remove_var(REQUIRE_ENV) };
        assert!(
            r.is_err(),
            "{REQUIRE_ENV} must make even an environmental skip fail"
        );
    }
}
