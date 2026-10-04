
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ensemble::{Ensemble, Hid, HumdKey, Liveness, PeerCapabilities};
use tracing::{debug, info, warn};

use crate::peer_transport::{iroh, tcp};
use crate::peers::PeerConfig;
use crate::redial::Backoff;

#[derive(Debug, Clone)]
pub struct LivenessConfig {
    pub ttl: Duration,
    pub interval: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(30),
            interval: Duration::from_secs(10),
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(300),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub evicted: Vec<Hid>,
    pub redialed: Vec<Hid>,
    pub backed_off: Vec<Hid>,
    pub dial_failed: Vec<Hid>,
}

pub struct Supervisor {
    ens: Arc<Ensemble>,
    key: Arc<HumdKey>,
    peers: Arc<Vec<PeerConfig>>,
    my_caps: PeerCapabilities,
    iroh: Option<Arc<ensemble::IrohTransport>>,
    cfg: LivenessConfig,
    backoff: HashMap<Hid, Backoff>,
    seq: u64,
}

impl Supervisor {
    pub fn new(
        ens: Arc<Ensemble>,
        key: Arc<HumdKey>,
        peers: Arc<Vec<PeerConfig>>,
        my_caps: PeerCapabilities,
        iroh: Option<Arc<ensemble::IrohTransport>>,
        cfg: LivenessConfig,
    ) -> Self {
        Self {
            ens,
            key,
            peers,
            my_caps,
            iroh,
            cfg,
            backoff: HashMap::new(),
            seq: 0,
        }
    }

    fn is_configured(&self, id: &Hid) -> bool {
        self.peers.iter().any(|p| p.humd_id == *id)
    }

    fn absent_configured_peers(&self) -> Vec<PeerConfig> {
        self.peers
            .iter()
            .filter(|p| !self.ens.peers().contains(&p.humd_id))
            .cloned()
            .collect()
    }

    fn backoff_for(&mut self, id: &Hid) -> &mut Backoff {
        self.backoff
            .entry(*id)
            .or_insert_with(|| Backoff::new(self.cfg.backoff_base, self.cfg.backoff_max))
    }

    pub async fn tick(&mut self) -> SweepReport {
        self.seq += 1;
        self.ens.probe_all(self.seq).await;
        let evicted = self.ens.evict_expired(self.cfg.ttl);

        let mut report = SweepReport {
            evicted: evicted.clone(),
            ..Default::default()
        };
        for id in &evicted {
            if !self.is_configured(id) {
                debug!(peer = %id.short(), "liveness.evicted.inbound_only");
            }
        }

        for peer in self.absent_configured_peers() {
            let id = peer.humd_id;
            if !self.backoff_for(&id).ready() {
                report.backed_off.push(id);
                continue;
            }
            if self.dial(&peer).await {
                self.backoff_for(&id).succeed();
                report.redialed.push(id);
                info!(peer = %id.short(), "liveness.redial.ok");
            } else {
                let delay = self.backoff_for(&id).fail();
                report.dial_failed.push(id);
                warn!(peer = %id.short(), ?delay, "liveness.redial.failed");
            }
        }
        report
    }

    async fn dial(&self, peer: &PeerConfig) -> bool {
        let mut attempted = false;
        if let Some(transport) = &self.iroh {
            let iroh_hints = peer
                .hints
                .iter()
                .any(|h| h.starts_with(ensemble::iroh::IROH_HINT));
            if iroh_hints {
                attempted = true;
                if iroh::dial_and_install_peer(transport, &self.ens, &self.key, peer, &self.my_caps).await {
                    return true;
                }
            }
        }
        if peer.hints.iter().any(|h| h.starts_with("tcp:")) {
            attempted = true;
            if tcp::dial_and_install_peer(&self.ens, &self.key, peer, &self.my_caps).await {
                return true;
            }
        }
        if !attempted {
            warn!(peer = %peer.humd_id.short(), "liveness.redial.undialable");
        }
        false
    }

    pub fn peer_liveness(&self, id: &Hid) -> Option<Liveness> {
        self.ens.peer_liveness(id, self.cfg.ttl)
    }

    pub fn consecutive_failures(&self, id: &Hid) -> u32 {
        self.backoff.get(id).map(|b| b.failures()).unwrap_or(0)
    }

    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.cfg.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        drop_immediate_boot_tick(&mut ticker).await;
        loop {
            ticker.tick().await;
            self.tick().await;
        }
    }
}

async fn drop_immediate_boot_tick(ticker: &mut tokio::time::Interval) {
    let _ = ticker.tick().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBES_BEFORE_EXPIRY: u32 = 3;

    #[test]
    fn default_config_probes_far_inside_ttl() {
        let c = LivenessConfig::default();
        assert!(
            c.interval * PROBES_BEFORE_EXPIRY <= c.ttl,
            "a peer must be probed repeatedly before it can expire"
        );
    }

    #[test]
    fn report_defaults_to_empty() {
        assert_eq!(
            SweepReport::default(),
            SweepReport {
                evicted: vec![],
                redialed: vec![],
                backed_off: vec![],
                dial_failed: vec![],
            }
        );
    }
}
