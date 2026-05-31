//! Plain TCP transport — outbound dial + inbound accept.
//!
//! Sibling of [`super::iroh`]. No transport-level crypto — the signed
//! ensemble hello is the only authentication on the wire. Use for
//! loopback, LAN, and trusted-link cases where iroh's QUIC + relay
//! mesh would be unnecessary overhead.
//!
//! Two seams:
//!
//! 1. [`dial_all`] — walk `peers.json`, open one TCP connection per
//!    entry that has a `tcp:` hint, install each via
//!    [`Ensemble::install`] (signed). Best-effort: one dead peer
//!    doesn't sink startup.
//!
//! 2. [`spawn_listener`] — bind `addr` and detach an accept loop.
//!    Returns the bound socket address (so callers running with `:0`
//!    recover the OS-assigned port) and the `peers.json` hint string a
//!    remote peer would paste in to dial back.
//!
//! Unlike iroh, TCP has no implicit endpoint — each connect is a free
//! [`ensemble::TcpEndpoint::connect`] call. The listener side wraps
//! [`ensemble::TcpListener`].

use std::net::SocketAddr;
use std::sync::Arc;

use ensemble::{Ensemble, HumdAddr, HumdKey, PeerCapabilities, PeerConnection, TcpEndpoint, TcpListener};
use tracing::{info, trace, warn};

use crate::peers::PeerConfig;

/// Open one TCP connection per bootstrap peer entry, install signed.
/// Entries without a `tcp:` hint are skipped — those are for other
/// transports.
pub(crate) async fn dial_all(
    ens: &Arc<Ensemble>,
    key: &HumdKey,
    peers: &[PeerConfig],
    my_caps: &PeerCapabilities,
) {
    for peer in peers {
        let Some(addr) = peer.hints.iter().find_map(|h| h.strip_prefix("tcp:")) else {
            trace!(peer = %peer.humd_id.short(), "peer.tcp.skip.no_hint");
            continue;
        };
        let mut peer_addr = HumdAddr::new(peer.humd_id);
        for h in &peer.hints {
            peer_addr.hints.push(h.clone());
        }
        match TcpEndpoint::connect(addr, peer_addr, PeerCapabilities::default()).await {
            Ok(conn) => {
                info!(peer = %peer.humd_id.short(), addr, "peer.tcp.dial.ok");
                ens.install(conn as Arc<dyn PeerConnection>, my_caps.clone(), key);
            }
            Err(e) => {
                warn!(peer = %peer.humd_id.short(), addr, err = %e, "peer.tcp.dial.failed");
            }
        }
    }
}

/// Bind `addr` and detach an accept loop. Each accepted connection is
/// installed via [`Ensemble::install`] (signed hello). Per-accept
/// failures are logged + brief backoff so one bad client can't take the
/// listener down.
///
/// Returns `(bound_addr, hints)` where `hints` is the single
/// `tcp:<host:port>` string a remote peer would paste into its
/// `peers.json`. Uses the supplied `addr` verbatim for that hint when
/// caller passed a fixed host:port; for `:0` callers, the OS-assigned
/// port lands in `bound_addr` and the caller should rewrite the hint.
pub(crate) async fn spawn_listener(
    addr: &str,
    ens: Arc<Ensemble>,
    key: Arc<HumdKey>,
    my_caps: PeerCapabilities,
) -> anyhow::Result<(SocketAddr, Vec<String>)> {
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    info!(addr = %bound, "peer.tcp.bound");
    let hints = vec![format!("tcp:{bound}")];

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok(conn) => {
                    info!("peer.tcp.accept.ok");
                    ens.install(conn as Arc<dyn PeerConnection>, my_caps.clone(), &key);
                }
                Err(e) => {
                    warn!(err = %e, "peer.tcp.accept.failed");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });

    Ok((bound, hints))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ensemble::{Ensemble, HumdKey};
    use std::time::Duration;

    #[tokio::test]
    async fn dial_then_accept_meet_via_signed_hello() {
        let a_key = Arc::new(HumdKey::generate());
        let b_key = Arc::new(HumdKey::generate());
        let a_id = a_key.hid();
        let b_id = b_key.hid();

        let ens_a = Arc::new(Ensemble::new(a_id));
        let ens_b = Arc::new(Ensemble::new(b_id));

        let caps = PeerCapabilities::default();

        let (a_addr, a_hints) = spawn_listener("127.0.0.1:0", ens_a.clone(), a_key.clone(), caps.clone())
            .await
            .expect("bind A");

        let peer_a = PeerConfig {
            humd_id: a_id,
            hints: vec![format!("tcp:{a_addr}")],
            alias: None,
        };
        assert_eq!(a_hints, vec![format!("tcp:{a_addr}")]);

        dial_all(&ens_b, &b_key, std::slice::from_ref(&peer_a), &caps).await;

        for _ in 0..100 {
            if ens_a.peers().contains(&b_id) && ens_b.peers().contains(&a_id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "peers never met: A.peers={:?} B.peers={:?}",
            ens_a.peers(),
            ens_b.peers()
        );
    }
}
