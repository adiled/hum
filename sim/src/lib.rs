//! sim — in-process ensemble simulator.
//!
//! Spin up N `humd` instances in one process, wire them with
//! `ensemble::InMemoryEndpoint::pair`, fake-network them, and run
//! narrative tests against the result. No sockets, no real subprocesses,
//! no I/O — every humd's `Thrum` is in-memory and every nest uses
//! `nest::MockPerch`.
//!
//! This crate is the foundation for the narrative test suite. It owns
//! lifecycle (spawn/wire/shutdown) and the synthetic-nestler hooks
//! (`nestler_send`, `nestler_recv`). The tests themselves live in the
//! caller (smoke test here is just a vital-signs check).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ensemble::{Ensemble, HumdId, InMemoryEndpoint, PeerCapabilities};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use thrumd::Thrum;
use tokio::sync::{mpsc, oneshot};

/// One in-process humd inside the sim: its identity, its Thrum (so the
/// sim can drive tones), its Ensemble (so the sim can wire peers), and
/// the shutdown handle for the spawned task.
pub struct SimHumd {
    pub id: HumdId,
    pub thrum: Thrum,
    pub ensemble: Arc<Ensemble>,
    pub shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub join: Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
    /// Per-synthetic-client outbound queues. `nestler_recv` pops from
    /// these. Keyed by the synthetic client_id we minted on send.
    /// Each entry is the receiver Thrum hands back from
    /// `register_synthetic`.
    out_queues: Mutex<HashMap<String, mpsc::Receiver<Value>>>,
    /// Per-sid mailbox: any tone whose `sid` matches drops in here.
    /// Built lazily by `nestler_recv`; fed by a fanout task per
    /// synthetic client.
    sid_mailboxes: Mutex<HashMap<String, mpsc::UnboundedReceiver<Value>>>,
    sid_senders: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
}

pub struct Sim {
    humds: RwLock<HashMap<HumdId, Arc<SimHumd>>>,
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

impl Sim {
    pub fn new() -> Self {
        Self {
            humds: RwLock::new(HashMap::new()),
        }
    }

    /// Spawn an in-process humd with the given id. Builds a fresh
    /// `Thrum` and `Ensemble`, plugs both into a `humd::DaemonConfig`,
    /// and launches `humd::run` on a task. Returns immediately.
    pub async fn spawn_humd(&self, id: HumdId) -> Arc<SimHumd> {
        let thrum = Thrum::new();
        let ensemble = Arc::new(Ensemble::new(id));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Dummy paths — they're inert when bind_mcp=false and thrum_override
        // is supplied, but DaemonConfig wants something to hold.
        let tmp = std::env::temp_dir().join(format!("sim-humd-{}", id.short()));
        let _ = std::fs::create_dir_all(&tmp);
        let penny_path = tmp.join("penny.json");

        let cfg = humd::DaemonConfig {
            thrum_path: tmp.join("thrum.sock"),
            http_path: tmp.join("http.sock"),
            mcp_addr: ([127, 0, 0, 1], 0).into(),
            penny_path,
            hum_cfg: config::HumConfig::default(),
            cli_path: "noop".into(),
            penny_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ensemble.clone()),
            bind_mcp: false,
            perches: humd::PerchSet {
                pipe: Arc::new(nest::MockPerch::default()),
                pty: Arc::new(nest::MockPerch::default()),
            },
        };

        let shutdown_fut = async move {
            let _ = shutdown_rx.await;
        };
        let join = tokio::spawn(humd::run(cfg, shutdown_fut));

        let sim_humd = Arc::new(SimHumd {
            id,
            thrum,
            ensemble,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            join: Mutex::new(Some(join)),
            out_queues: Mutex::new(HashMap::new()),
            sid_mailboxes: Mutex::new(HashMap::new()),
            sid_senders: Mutex::new(HashMap::new()),
        });

        self.humds.write().insert(id, sim_humd.clone());
        sim_humd
    }

    /// Wire two humds with an in-memory channel pair. Both ensembles
    /// pick up a `PeerConnection` to the other; capabilities default
    /// to the current proto version with no nests / hosts advertised.
    pub fn wire(&self, a: HumdId, b: HumdId) -> Result<()> {
        let humds = self.humds.read();
        let ha = humds
            .get(&a)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        let hb = humds
            .get(&b)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", b.short()))?;
        drop(humds);

        let caps = PeerCapabilities {
            proto_version: thrum_core::THRUM_VERSION.to_string(),
            ..Default::default()
        };
        let (a_view, b_view) = InMemoryEndpoint::pair(
            ha.id,
            caps.clone(),
            hb.id,
            caps,
        );
        // a's ensemble learns about b via `a_view` (whose peer is b).
        ha.ensemble.add_peer(a_view);
        hb.ensemble.add_peer(b_view);
        Ok(())
    }

    /// Mock-send a tone into humd's Thrum as if from a connected
    /// nestler. Registers a synthetic client on first call per humd,
    /// spawns a fanout task that immediately routes incoming tones
    /// into per-sid mailboxes, then injects the tone through the sink.
    /// Returns the synthetic client_id so callers can correlate
    /// replies.
    pub fn nestler_send(&self, humd: HumdId, tone: Value) -> Result<String> {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        let client_id = format!("sim-{}", uuid::Uuid::new_v4());
        let mut rx = h.thrum.register_synthetic(client_id.clone());

        // Fanout task: drain this synthetic's outbound queue and route
        // each tone into its sid's mailbox (lazily creating mailboxes
        // for new sids). Without this, tones sit in the mpsc receiver
        // forever and nestler_recv only drains on entry — racing the
        // detached inject_tone.
        let h_for_pump = h.clone();
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                let sid = tone
                    .get("sid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() { continue; }
                let tx = {
                    let mut senders = h_for_pump.sid_senders.lock();
                    if let Some(tx) = senders.get(&sid) {
                        tx.clone()
                    } else {
                        let (tx, rx2) = mpsc::unbounded_channel::<Value>();
                        senders.insert(sid.clone(), tx.clone());
                        h_for_pump.sid_mailboxes.lock().insert(sid.clone(), rx2);
                        tx
                    }
                };
                let _ = tx.send(tone);
            }
        });

        // Inject on a detached task so the sync API stays sync. The
        // sink is async; we don't want to block the caller while a
        // nest spawns.
        let thrum = h.thrum.clone();
        let cid = client_id.clone();
        tokio::spawn(async move {
            thrum.inject_tone(&cid, tone).await;
        });
        Ok(client_id)
    }

    /// Take the next tone broadcast on `humd` whose `sid` matches.
    /// Best-effort: drains every synthetic out_queue into per-sid
    /// mailboxes, then waits up to `timeout` for the named sid.
    pub async fn nestler_recv(
        &self,
        humd: HumdId,
        sid: &str,
        timeout: Duration,
    ) -> Option<Value> {
        let h = self.humds.read().get(&humd).cloned()?;

        // Bind a mailbox sender for this sid if absent. We pump every
        // synthetic out_queue's items in here, keyed by their `sid`
        // field. Tones without a sid get dropped.
        {
            let mut senders = h.sid_senders.lock();
            let mut mailboxes = h.sid_mailboxes.lock();
            if !senders.contains_key(sid) {
                let (tx, rx) = mpsc::unbounded_channel::<Value>();
                senders.insert(sid.to_string(), tx);
                mailboxes.insert(sid.to_string(), rx);
            }
        }

        // Drain all out_queues into the per-sid mailboxes. We move
        // each receiver out, drain non-blockingly, then put it back.
        // This keeps the per-humd state simple — no long-lived fanout
        // task per synthetic client.
        let mut queues = h.out_queues.lock();
        for (_cid, rx) in queues.iter_mut() {
            while let Ok(tone) = rx.try_recv() {
                let tone_sid = tone
                    .get("sid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if tone_sid.is_empty() {
                    continue;
                }
                let senders = h.sid_senders.lock();
                if let Some(tx) = senders.get(&tone_sid) {
                    let _ = tx.send(tone);
                } else {
                    drop(senders);
                    // Lazily mint a mailbox so future recv calls for
                    // this sid see the message.
                    let (tx, rx2) = mpsc::unbounded_channel::<Value>();
                    let _ = tx.send(tone);
                    h.sid_senders.lock().insert(tone_sid.clone(), tx);
                    h.sid_mailboxes.lock().insert(tone_sid, rx2);
                }
            }
        }
        drop(queues);

        // Now await the named sid's mailbox.
        let mut rx_opt = h.sid_mailboxes.lock().remove(sid)?;
        let result = tokio::time::timeout(timeout, rx_opt.recv()).await.ok().flatten();
        // Put the mailbox back so subsequent recvs can use it.
        h.sid_mailboxes.lock().insert(sid.to_string(), rx_opt);
        result
    }

    /// Shutdown all humds and drain their join handles.
    pub async fn shutdown(self) {
        let humds: Vec<Arc<SimHumd>> = self.humds.read().values().cloned().collect();
        for h in &humds {
            if let Some(tx) = h.shutdown_tx.lock().take() {
                let _ = tx.send(());
            }
        }
        for h in &humds {
            let join = h.join.lock().take();
            if let Some(j) = join {
                let _ = j.await;
            }
        }
    }
}

// Silence the "field never read" warning for penny_path placeholder.
#[allow(dead_code)]
fn _keep_path_alive(_p: PathBuf) {}
