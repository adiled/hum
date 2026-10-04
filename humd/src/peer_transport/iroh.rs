//! Iroh QUIC transport — outbound dial + inbound accept.
//!
//! humd's [`HumdKey`] is pinned to iroh's `SecretKey` at bind time, so
//! the NodeId and the [`Hid`] derive from the same pubkey: signed
//! hellos verify against the iroh-routed identity without a second
//! keypair to manage.
//!
//! Three seams:
//!
//! 1. [`bind`] — build an [`IrohTransport`] using `humd_key`. Returns
//!    the transport plus the `peers.json` hint strings a remote peer
//!    would paste in to dial this humd back (`iroh:<node>` plus one
//!    `iroh-ip:<sockaddr>` per bound socket).
//!
//! 2. [`dial_all`] — walk `peers.json`, open one iroh connection per
//!    entry that has an `iroh:` hint, install each via
//!    [`Ensemble::install`] (signed). Best-effort: one dead peer
//!    doesn't sink startup.
//!
//! 3. [`spawn_listener`] — detach an accept loop on the transport.
//!    Each accepted connection is installed the same way, so the dial
//!    and accept paths converge on a single registry entry per peer.
//!
//! Relay is disabled — this is the loopback/LAN/direct path. WAN over
//! a relay mesh lands later via a sibling bind path on this module.

use std::net::SocketAddr;
use std::sync::Arc;

use ensemble::{Ensemble, HumdAddr, HumdKey, IrohTransport, PeerCapabilities, PeerConnection};
use tracing::{info, warn};

use crate::peers::PeerConfig;

/// Build the iroh transport with `humd_key` pinned as the iroh
/// SecretKey. Returns the transport plus the `peers.json` hints a
/// remote peer would use to dial this humd back.
pub(crate) async fn bind(humd_key: &HumdKey) -> anyhow::Result<(IrohTransport, Vec<String>)> {
    let transport = IrohTransport::bind_direct_with_key(humd_key).await?;
    let node_id_hex = hex::encode(transport.node_id().as_bytes());
    let discovered: Vec<SocketAddr> = transport
        .endpoint()
        .addr()
        .ip_addrs()
        .copied()
        .collect();
    let hints = dial_hints(&node_id_hex, &discovered);
    info!(
        node_id = %&node_id_hex[..16],
        socket_count = hints.len() - 1,
        "peer.iroh.bound"
    );
    Ok((transport, hints))
}

fn dial_hints(node_id_hex: &str, addrs: &[SocketAddr]) -> Vec<String> {
    let routable: Vec<String> = addrs
        .iter()
        .filter(|addr| !addr.ip().is_unspecified())
        .map(|addr| format!("iroh-ip:{addr}"))
        .collect();
    let mut hints = Vec::with_capacity(1 + routable.len());
    hints.push(format!("iroh:{node_id_hex}"));
    hints.extend(routable);
    hints
}

pub(crate) async fn dial_and_install_peer(
    transport: &IrohTransport,
    ens: &Arc<Ensemble>,
    key: &HumdKey,
    peer: &PeerConfig,
    my_caps: &PeerCapabilities,
) -> bool {
    use ensemble::Transport as _;

    if !peer.hints.iter().any(|h| h.starts_with(ensemble::iroh::IROH_HINT)) {
        return false;
    }
    let mut peer_addr = HumdAddr::new(peer.humd_id);
    for h in &peer.hints {
        peer_addr.hints.push(h.clone());
    }
    match transport.connect(&peer_addr).await {
        Ok(conn) => {
            info!(peer = %peer.humd_id.short(), "peer.iroh.dial.ok");
            ens.install(conn, my_caps.clone(), key);
            true
        }
        Err(e) => {
            warn!(peer = %peer.humd_id.short(), err = %e, "peer.iroh.dial.failed");
            false
        }
    }
}

/// Open one iroh connection per bootstrap peer entry, install signed.
/// Entries without an `iroh:` hint are skipped — those are for other
/// transports.
pub(crate) async fn dial_all(
    transport: &IrohTransport,
    ens: &Arc<Ensemble>,
    key: &HumdKey,
    peers: &[PeerConfig],
    my_caps: &PeerCapabilities,
) {
    for peer in peers {
        dial_and_install_peer(transport, ens, key, peer, my_caps).await;
    }
}

/// Detach the accept loop. Each accepted connection is installed via
/// [`Ensemble::install`] (signed hello, same as the dial path). Per-
/// accept failures are logged and retried after a short backoff so one
/// bad client can't take the listener down.
pub(crate) fn spawn_listener(
    transport: Arc<IrohTransport>,
    ens: Arc<Ensemble>,
    key: Arc<HumdKey>,
    my_caps: PeerCapabilities,
) {
    tokio::spawn(async move {
        loop {
            match transport.accept().await {
                Ok(conn) => {
                    info!("peer.iroh.accept.ok");
                    ens.install(conn as Arc<dyn PeerConnection>, my_caps.clone(), &key);
                }
                Err(e) => {
                    warn!(err = %e, "peer.iroh.accept.failed");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::dial_hints;

    #[test]
    fn wildcard_binds_are_never_advertised_as_dial_targets() {
        let routable: SocketAddr = "10.0.0.5:4556".parse().unwrap();
        let wildcard: SocketAddr = "0.0.0.0:4556".parse().unwrap();
        assert_eq!(
            dial_hints("node", &[routable, wildcard]),
            vec!["iroh:node".to_string(), "iroh-ip:10.0.0.5:4556".to_string()],
        );
    }

    use super::*;
    use ensemble::{Ensemble, HumdKey};
    use std::time::Duration;

    /// Bind two iroh transports, each pinned to its own HumdKey, wire
    /// them with `iroh:` + `iroh-ip:` hints, and verify both sides see
    /// each other in their peer registry after the handshake.
    #[tokio::test]
    async fn dial_then_accept_meet_via_signed_hello() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_test_writer()
            .try_init();

        let a_key = Arc::new(HumdKey::generate());
        let b_key = Arc::new(HumdKey::generate());
        let a_id = a_key.hid();
        let b_id = b_key.hid();

        let ens_a = Arc::new(Ensemble::new(a_id));
        let ens_b = Arc::new(Ensemble::new(b_id));

        let caps = PeerCapabilities::default();

        let (a_tx, a_hints) = match bind(&a_key).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: iroh bind A failed: {e}");
                return;
            }
        };
        let (b_tx, _b_hints) = match bind(&b_key).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: iroh bind B failed: {e}");
                return;
            }
        };

        spawn_listener(Arc::new(a_tx), ens_a.clone(), a_key.clone(), caps.clone());

        let peer_a = PeerConfig {
            humd_id: a_id,
            hints: a_hints,
            alias: None,
        };
        dial_all(&b_tx, &ens_b, &b_key, std::slice::from_ref(&peer_a), &caps).await;

        for _ in 0..200 {
            if ens_a.peers().contains(&b_id) && ens_b.peers().contains(&a_id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "peers never met: A.peers={:?} B.peers={:?}",
            ens_a.peers(),
            ens_b.peers()
        );
    }
}
