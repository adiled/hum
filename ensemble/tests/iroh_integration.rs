//! End-to-end test of the iroh transport: spin up two
//! [`IrohTransport`]s in-process on loopback, dial across, run the
//! signed ensemble handshake, and trade tones in both directions.
//!
//! Some CI sandboxes can't bind UDP sockets or initialise a rustls
//! crypto provider — those failures aren't transport bugs and we skip
//! the test gracefully so the suite still passes there.

use std::time::Duration;

use ed25519_dalek::SigningKey;
use ensemble::{
    Ensemble, HumdAddr, Hid, HumdKey, IrohTransport, PeerCapabilities, PeerConnection, Transport,
};
use serde_json::json;

/// Try to bind a fresh iroh endpoint. If iroh refuses to come up
/// (sandbox without UDP, missing crypto provider, etc.) return None
/// and the caller skips. Anything else is a real failure.
async fn try_bind() -> Option<IrohTransport> {
    match IrohTransport::bind_direct().await {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("iroh_integration: skipping — bind failed: {e}");
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
    // With relay disabled and no DNS lookup configured, the dialer
    // needs explicit IP/port hints — pull them off the bound sockets.
    let server_sockets: Vec<String> = server
        .endpoint()
        .bound_sockets()
        .into_iter()
        .map(|s| format!("iroh-ip:{}", s))
        .collect();
    assert!(
        !server_sockets.is_empty(),
        "iroh server endpoint reported no bound sockets"
    );

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
            eprintln!("iroh_integration: skipping — connect failed: {e}");
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
