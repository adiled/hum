//! The redial supervisor: one loop that keeps the peer set matching
//! the bootstrap set.
//!
//! Boot dials every peer once and never looks back, so a peer that
//! dies stays in the registry forever: routing keeps addressing a dead
//! link, and a peer that restarts is unreachable because nobody dials
//! it again. This loop closes that gap.
//!
//! Each tick probes, then sweeps. A peer that stops answering goes
//! `Stale`; after `ttl` it is evicted, and the redial attempt that
//! follows carries its own exponential backoff so a peer that stays
//! down doesn't turn into a dial spin.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ensemble::{Ensemble, Hid, HumdKey, Liveness, PeerCapabilities};
use tracing::{debug, info, warn};

use crate::peer_transport::{iroh, tcp};
use crate::peers::PeerConfig;
use crate::redial::Backoff;

/// Tuning for the supervisor. Timeouts are short by default: a
/// distributed peer is expected to be a LAN hop or a relay, and a slow
/// ping should be a warning, not a half-minute stall.
#[derive(Debug, Clone)]
pub struct LivenessConfig {
    /// Silence after which an un-probed peer is reaped.
    pub ttl: Duration,
    /// How often to probe and sweep.
    pub interval: Duration,
    /// Backoff bounds for a peer that fails to redial.
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

/// What one tick decided, so a caller (or a test) can observe the
/// supervisor without reading logs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Peers evicted this tick, in registry order.
    pub evicted: Vec<Hid>,
    /// Evicted peers that were dialled again this tick.
    pub redialed: Vec<Hid>,
    /// Evicted peers whose redial was still inside its backoff window.
    pub backed_off: Vec<Hid>,
    /// Peers that were evicted and redialled unsuccessfully.
    pub dial_failed: Vec<Hid>,
}

/// The transports a peer may be reachable over. A peer with both an
/// `iroh:` and a `tcp:` hint gets tried over both; the first that
/// connects wins and the other is not attempted.
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

    fn backoff_for(&mut self, id: &Hid) -> &mut Backoff {
        self.backoff
            .entry(*id)
            .or_insert_with(|| Backoff::new(self.cfg.backoff_base, self.cfg.backoff_max))
    }

    /// Probe, sweep, redial. One pass. Exposed so a test can drive the
    /// supervisor deterministically instead of waiting on a timer.
    pub async fn tick(&mut self) -> SweepReport {
        self.seq += 1;
        self.ens.probe_all(self.seq).await;
        let evicted = self.ens.evict_expired(self.cfg.ttl);

        let mut report = SweepReport {
            evicted: evicted.clone(),
            ..Default::default()
        };
        for id in evicted {
            if !self.peers.iter().any(|p| p.humd_id == id) {
                // Evicted a peer we didn't dial (inbound-only). Nothing
                // to redial, but it's no longer in the registry.
                debug!(peer = %id.short(), "liveness.evicted.inbound_only");
            }
        }

        // The redial set is "configured but absent", not "just evicted".
        // A peer whose *first* dial failed was never installed, so no
        // sweep will ever name it — and a peer that is down at boot is
        // exactly the one that must come back.
        let wanted: Vec<PeerConfig> = self
            .peers
            .iter()
            .filter(|p| !self.ens.peers().contains(&p.humd_id))
            .cloned()
            .collect();

        for peer in wanted {
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

    /// Try each transport this peer advertises, in preference order.
    /// A transport this daemon isn't running is skipped rather than
    /// failed — an iroh-only daemon shouldn't count TCP's absence
    /// against the peer.
    ///
    /// A peer that advertises nothing we can dial fails: reporting
    /// success would clear its backoff, and the peer would be
    /// re-evicted and re-"dialled" on every tick forever.
    async fn dial(&self, peer: &PeerConfig) -> bool {
        let mut dialable = false;
        if let Some(transport) = &self.iroh {
            if peer
                .hints
                .iter()
                .any(|h| h.starts_with(ensemble::iroh::IROH_HINT))
            {
                dialable = true;
                if iroh::dial_one(transport, &self.ens, &self.key, peer, &self.my_caps).await {
                    return true;
                }
            }
        }
        if peer.hints.iter().any(|h| h.starts_with("tcp:")) {
            dialable = true;
            if tcp::dial_one(&self.ens, &self.key, peer, &self.my_caps).await {
                return true;
            }
        }
        if !dialable {
            warn!(peer = %peer.humd_id.short(), "liveness.redial.undialable");
        }
        false
    }

    /// Liveness of a single peer, for reporting.
    pub fn peer_liveness(&self, id: &Hid) -> Option<Liveness> {
        self.ens.peer_liveness(id, self.cfg.ttl)
    }

    /// Consecutive failed redials for a peer.
    pub fn attempts(&self, id: &Hid) -> u32 {
        self.backoff.get(id).map(|b| b.attempts()).unwrap_or(0)
    }

    /// Run until cancelled. Probes, sweeps, and redials on every tick.
    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.cfg.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; the boot dial has already
        // happened, so that pass would only duplicate work.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            self.tick().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_probes_far_inside_ttl() {
        let c = LivenessConfig::default();
        assert!(
            c.interval * 3 <= c.ttl,
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
