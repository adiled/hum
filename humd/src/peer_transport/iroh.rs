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
//!    would paste in to dial this humd back (`iroh:<node>`, one
//!    `iroh-ip:<sockaddr>` per local address, and `iroh-relay:<url>`
//!    for the home relay when one is reached within [`RELAY_WAIT`]).
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
//! The endpoint runs iroh's n0 preset: the public n0 relay mesh plus
//! n0's DNS address lookup. A humd behind NAT keeps a home relay and
//! publishes it under its node id, so a peer can dial it anywhere in
//! the world. With only an `iroh:<node>` hint the dialer resolves the
//! current relay by DNS. An `iroh-relay:` hint skips that lookup. iroh
//! opens on whichever path answers first, then hole-punches to a
//! direct path when the NATs allow it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ensemble::{Ensemble, HumdAddr, HumdKey, IrohTransport, PeerCapabilities, PeerConnection};
use tracing::{info, warn};

use crate::peers::PeerConfig;

pub(crate) async fn bind(
    humd_key: &HumdKey,
    port: u16,
) -> anyhow::Result<(IrohTransport, Vec<String>)> {
    let transport = match IrohTransport::bind_with_key_on(humd_key, port_nonzero(port)).await {
        Ok(t) => t,
        Err(e) => {
            warn!(
                port,
                err = %e,
                "peer.iroh.fixed_port.bind_failed — taking an ephemeral port; the address peers hold goes stale on restart"
            );
            IrohTransport::bind_with_key(humd_key).await?
        }
    };
    let node_id_hex = hex::encode(transport.node_id().as_bytes());
    if !transport.online_within(RELAY_WAIT).await {
        warn!(
            wait_ms = RELAY_WAIT.as_millis() as u64,
            "peer.iroh.relay.not_yet — no home relay; peers outside this network can dial once it connects, by node id"
        );
    }
    let addr = transport.endpoint().addr();
    let discovered: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
    let relays: Vec<String> = addr.relay_urls().map(|u| u.to_string()).collect();
    let hints = dial_hints(&node_id_hex, &discovered, &relays);
    info!(
        node_id = %&node_id_hex[..16],
        socket_count = discovered.len(),
        relay = relays.first().map(String::as_str).unwrap_or("none"),
        "peer.iroh.bound"
    );
    Ok((transport, hints))
}

const RELAY_WAIT: Duration = Duration::from_secs(5);

fn port_nonzero(port: u16) -> Option<u16> {
    if port == 0 { None } else { Some(port) }
}

fn dial_hints(node_id_hex: &str, addrs: &[SocketAddr], relays: &[String]) -> Vec<String> {
    let routable = addrs
        .iter()
        .filter(|addr| !addr.ip().is_unspecified())
        .map(|addr| format!("{}{addr}", ensemble::IROH_IP_HINT));
    let relayed = relays
        .iter()
        .map(|url| format!("{}{url}", ensemble::IROH_RELAY_HINT));
    std::iter::once(format!("{}{node_id_hex}", ensemble::iroh::IROH_HINT))
        .chain(routable)
        .chain(relayed)
        .collect()
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

pub(crate) async fn dial_all(
    transport: &IrohTransport,
    ens: &Arc<Ensemble>,
    key: &HumdKey,
    peers: &[PeerConfig],
    my_caps: &PeerCapabilities,
) -> Vec<String> {
    let mut reached = Vec::new();
    for peer in peers {
        if dial_and_install_peer(transport, ens, key, peer, my_caps).await {
            reached.push(peer.humd_id.to_hex());
        }
    }
    reached
}

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
            dial_hints("node", &[routable, wildcard], &[]),
            vec!["iroh:node".to_string(), "iroh-ip:10.0.0.5:4556".to_string()],
        );
    }

    #[test]
    fn the_home_relay_is_advertised_after_the_addresses() {
        let routable: SocketAddr = "10.0.0.5:4556".parse().unwrap();
        let relay = "https://use1-1.relay.n0.iroh-canary.iroh.link./".to_string();
        assert_eq!(
            dial_hints("node", &[routable], std::slice::from_ref(&relay)),
            vec![
                "iroh:node".to_string(),
                "iroh-ip:10.0.0.5:4556".to_string(),
                format!("iroh-relay:{relay}"),
            ],
        );
    }

    use super::*;
    use ensemble::{Ensemble, HumdKey};
    use std::time::Duration;

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

        let (a_tx, a_hints) = match bind(&a_key, 0).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipping: iroh bind A failed: {e}");
                return;
            }
        };
        let (b_tx, _b_hints) = match bind(&b_key, 0).await {
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
