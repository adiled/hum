
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use ensemble::{
    hello_tone, Ensemble, Hid, HumdKey, InMemoryEndpoint, LinkCounters, LinkFaults,
    PeerCapabilities,
};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use thrum_core::WaneTracker;
use humd::thrumd::Thrum;
use tokio::sync::{mpsc, oneshot};

const CAPACITY_UNLIMITED: usize = usize::MAX;

const SIM_NEST_KIND: &str = "claude-repl";

fn sim_caps(humd: &SimHumd) -> PeerCapabilities {
    let cap = humd.capacity.load(Ordering::SeqCst);
    PeerCapabilities {
        proto_version: thrum_core::THRUM_VERSION.to_string(),
        nests: vec![SIM_NEST_KIND.to_string()],
        free_slots: (cap != CAPACITY_UNLIMITED).then_some(cap),
        ..Default::default()
    }
}

fn local_capacity(max_concurrent: usize) -> humd::LocalCapacity {
    match max_concurrent {
        0 => humd::LocalCapacity::OverflowAlways,
        CAPACITY_UNLIMITED => humd::LocalCapacity::Unlimited,
        n => humd::LocalCapacity::Slots(n),
    }
}

pub struct SimHumd {
    pub id: Hid,
    pub thrum: Thrum,
    pub ensemble: Arc<Ensemble>,
    pub shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub join: Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
    out_queues: Mutex<HashMap<String, mpsc::Receiver<Value>>>,
    sid_mailboxes: Mutex<HashMap<String, mpsc::UnboundedReceiver<Value>>>,
    sid_senders: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
    capacity: AtomicUsize,
    pub waneman: Arc<WaneTracker>,
    pub key: Option<Arc<HumdKey>>,
}

pub struct Sim {
    humds: RwLock<HashMap<Hid, Arc<SimHumd>>>,
    pending_capacities: RwLock<HashMap<Hid, usize>>,
    links: RwLock<HashMap<(Hid, Hid), Link>>,
}

#[derive(Clone)]
struct Link {
    a: Hid,
    b: Hid,
    a_end: Arc<InMemoryEndpoint>,
    b_end: Arc<InMemoryEndpoint>,
}

fn link_key(x: Hid, y: Hid) -> (Hid, Hid) {
    if x.to_hex() <= y.to_hex() { (x, y) } else { (y, x) }
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
            pending_capacities: RwLock::new(HashMap::new()),
            links: RwLock::new(HashMap::new()),
        }
    }

    pub fn set_capacity(&self, humd: Hid, max_concurrent: usize) {
        if let Some(h) = self.humds.read().get(&humd).cloned() {
            h.capacity.store(max_concurrent, Ordering::SeqCst);
            return;
        }
        self.pending_capacities.write().insert(humd, max_concurrent);
    }

    pub async fn spawn_humd(&self, id: Hid) -> Arc<SimHumd> {
        let thrum = Thrum::new();
        let ensemble = Arc::new(Ensemble::with_strict_auth(id, false));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let tmp = std::env::temp_dir().join(format!("sim-humd-{}", id.short()));
        let _ = std::fs::create_dir_all(&tmp);
        let penny_path = tmp.join(hum_paths::PENNY_BASENAME);

        let initial_capacity = self
            .pending_capacities
            .write()
            .remove(&id)
            .unwrap_or(CAPACITY_UNLIMITED);
        let capacity = local_capacity(initial_capacity);

        let waneman = Arc::new(WaneTracker::new());
        let cfg = humd::DaemonConfig {
            thrum_path: tmp.join(hum_paths::THRUM_SOCK_BASENAME),
            http_path: tmp.join(hum_paths::HTTP_SOCK_BASENAME),
            mcp_addr: ([127, 0, 0, 1], 0).into(),
            penny_path,
            routing_path: tmp.join(hum_paths::ROUTING_JSON_BASENAME),
            routing_persist_interval: Duration::from_secs(3600),
            hum_cfg: hum_paths::config::HumConfig::default(),
            cli_path: "noop".into(),
            penny_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ensemble.clone()),
            bind_mcp: false,
            capacity,
            waneman: Some(waneman.clone()),
            humd_key: None,
            bootstrap_peers: Vec::new(),
            thehum_cfg: None,
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
            capacity: AtomicUsize::new(initial_capacity),
            waneman,
            key: None,
        });

        self.humds.write().insert(id, sim_humd.clone());
        sim_humd
      }

    pub async fn await_ready(&self, humd: Hid) -> Result<()> {
        for _ in 0..200 {
            let ready = {
                let Some(h) = self.humds.read().get(&humd).cloned() else {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    continue;
                };
                h.thrum.has_sink() && h.ensemble.has_subscribers()
            };
            if ready {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        bail!("humd {} never became ready", humd.short())
    }

    pub fn rewire(&self, a: Hid, b: Hid) -> Result<()> {
        self.wire(a, b)
    }

    /// Unauthenticated mesh link. Use [`Sim::wire_signed`] when the test is
    /// about identity.
    pub fn wire(&self, a: Hid, b: Hid) -> Result<()> {
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

        let a_caps = sim_caps(&ha);
        let b_caps = sim_caps(&hb);
        let (a_view, b_view) = InMemoryEndpoint::pair_concrete(
            ha.id,
            b_caps.clone(),
            hb.id,
            a_caps.clone(),
        );
        let key = link_key(ha.id, hb.id);
        self.links.write().insert(
            key,
            Link {
                a: ha.id,
                b: hb.id,
                a_end: a_view.clone(),
                b_end: b_view.clone(),
            },
        );
        ha.ensemble
            .add_peer_with_caps(a_view as Arc<dyn ensemble::PeerConnection>, a_caps);
        hb.ensemble
            .add_peer_with_caps(b_view as Arc<dyn ensemble::PeerConnection>, b_caps);
        Ok(())
    }

    pub async fn spawn_humd_with_identity(&self, key: HumdKey) -> Arc<SimHumd> {
        let id = key.hid();
        let thrum = Thrum::new();
        let ensemble = Arc::new(Ensemble::with_strict_auth(id, true));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let tmp = std::env::temp_dir().join(format!("sim-humd-{}", id.short()));
        let _ = std::fs::create_dir_all(&tmp);
        let penny_path = tmp.join(hum_paths::PENNY_BASENAME);

        let initial_capacity = self
            .pending_capacities
            .write()
            .remove(&id)
            .unwrap_or(CAPACITY_UNLIMITED);
        let capacity = local_capacity(initial_capacity);

        let waneman = Arc::new(WaneTracker::new());
        let cfg = humd::DaemonConfig {
            thrum_path: tmp.join(hum_paths::THRUM_SOCK_BASENAME),
            http_path: tmp.join(hum_paths::HTTP_SOCK_BASENAME),
            mcp_addr: ([127, 0, 0, 1], 0).into(),
            penny_path,
            routing_path: tmp.join(hum_paths::ROUTING_JSON_BASENAME),
            routing_persist_interval: Duration::from_secs(3600),
            hum_cfg: hum_paths::config::HumConfig::default(),
            cli_path: "noop".into(),
            penny_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ensemble.clone()),
            bind_mcp: false,
            capacity,
            waneman: Some(waneman.clone()),
            humd_key: None,
            bootstrap_peers: Vec::new(),
            thehum_cfg: None,
        };

        let shutdown_fut = async move { let _ = shutdown_rx.await; };
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
            capacity: AtomicUsize::new(initial_capacity),
            waneman,
            key: Some(Arc::new(key)),
        });

        self.humds.write().insert(id, sim_humd.clone());
        sim_humd
    }

    pub fn wire_signed(&self, a: Hid, b: Hid) -> Result<()> {
        let humds = self.humds.read();
        let ha = humds.get(&a).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        let hb = humds.get(&b).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", b.short()))?;
        drop(humds);
        let a_key = ha.key.clone().ok_or_else(|| anyhow::anyhow!(
            "humd {} has no signing key — spawn via spawn_humd_with_identity",
            a.short()
        ))?;
        let b_key = hb.key.clone().ok_or_else(|| anyhow::anyhow!(
            "humd {} has no signing key — spawn via spawn_humd_with_identity",
            b.short()
        ))?;

        let a_caps = sim_caps(&ha);
        let b_caps = sim_caps(&hb);
        let (a_view, b_view) = InMemoryEndpoint::pair(
            ha.id, b_caps.clone(),
            hb.id, a_caps.clone(),
        );
        ha.ensemble.install(a_view, a_caps, &a_key);
        hb.ensemble.install(b_view, b_caps, &b_key);
        Ok(())
    }

    pub fn wire_signed_tampered(&self, a: Hid, c: Hid) -> Result<()> {
        let humds = self.humds.read();
        let ha = humds.get(&a).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        let hc = humds.get(&c).cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", c.short()))?;
        drop(humds);
        let a_key = ha.key.clone().ok_or_else(|| anyhow::anyhow!(
            "humd {} has no signing key", a.short()
        ))?;

        let a_caps = sim_caps(&ha);
        let c_caps = sim_caps(&hc);
        let (a_view, c_view) = InMemoryEndpoint::pair(
            ha.id, c_caps.clone(),
            hc.id, a_caps.clone(),
        );

        ha.ensemble.install(a_view, a_caps, &a_key);

        let attacker_key = HumdKey::generate();
        let tampered = hello_tone(&hc.id, &attacker_key, &c_caps);
        let c_for_send = c_view.clone();
        tokio::spawn(async move {
            let _ = c_for_send.send(tampered).await;
        });
        hc.ensemble.install_unsigned(c_view, c_caps);
        Ok(())
    }

    fn link(&self, a: Hid, b: Hid) -> Result<Link> {
        self.links
            .read()
            .get(&link_key(a, b))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no link {}-{}", a.short(), b.short()))
    }

    fn end_for(&self, a: Hid, b: Hid) -> Result<Arc<InMemoryEndpoint>> {
        let link = self.link(a, b)?;
        Ok(if link.a == a { link.a_end } else { link.b_end })
    }

    pub fn impair_dir(&self, a: Hid, b: Hid, faults: LinkFaults) -> Result<()> {
        self.end_for(a, b)?.set_faults(faults);
        Ok(())
    }

    pub fn stall_dir(&self, a: Hid, b: Hid) -> Result<()> {
        self.end_for(a, b)?.stall();
        Ok(())
    }

    pub fn unstall_dir(&self, a: Hid, b: Hid) -> Result<()> {
        self.end_for(a, b)?.unstall();
        Ok(())
    }

    pub fn is_stalled(&self, a: Hid, b: Hid) -> Result<bool> {
        Ok(self.end_for(a, b)?.is_stalled())
    }

    pub fn impair(&self, a: Hid, b: Hid, faults: LinkFaults) -> Result<()> {
        self.impair_dir(a, b, faults.clone())?;
        self.impair_dir(b, a, faults)
    }

    pub fn link_counters(&self, a: Hid, b: Hid) -> Result<(LinkCounters, LinkCounters)> {
        let link = self.link(a, b)?;
        Ok(if link.a == a {
            (link.a_end.counters(), link.b_end.counters())
        } else {
            (link.b_end.counters(), link.a_end.counters())
        })
    }

    pub fn link_counters_since(
        &self,
        a: Hid,
        b: Hid,
        base: &LinkCounters,
    ) -> Result<LinkCounters> {
        let (ab, _) = self.link_counters(a, b)?;
        Ok(ab.since(base))
    }

    pub fn buffered(&self, a: Hid, b: Hid) -> Result<usize> {
        Ok(self.end_for(a, b)?.buffered())
    }

    pub fn heal_link_faults(&self, a: Hid, b: Hid) -> Result<()> {
        self.end_for(a, b)?.set_faults(LinkFaults::default());
        self.end_for(b, a)?.set_faults(LinkFaults::default());
        Ok(())
    }

    pub fn kill_peer(&self, a: Hid, b: Hid) -> Result<()> {
        self.end_for(a, b)?.kill();
        Ok(())
    }

    pub fn kill_link(&self, a: Hid, b: Hid) -> Result<()> {
        self.kill_peer(a, b)?;
        self.kill_peer(b, a)
    }

    pub fn link_killed(&self, a: Hid, b: Hid) -> Result<bool> {
        Ok(self.end_for(a, b)?.is_killed())
    }

    pub async fn probe(&self, a: Hid, b: Hid) -> Result<()> {
        let ens = self
            .humds
            .read()
            .get(&a)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        ens.ensemble.probe_one(&b, 0).await;
        Ok(())
    }

    pub fn peer_liveness(
        &self,
        observer: Hid,
        ttl: std::time::Duration,
    ) -> Result<Vec<(ensemble::Hid, ensemble::Liveness)>> {
        let ens = self
            .humds
            .read()
            .get(&observer)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", observer.short()))?;
        Ok(ens
            .ensemble
            .peers()
            .into_iter()
            .filter_map(|p| ens.ensemble.peer_liveness(&p, ttl).map(|l| (p, l)))
            .collect())
    }

    pub fn evict_expired(&self, observer: Hid, ttl: std::time::Duration) -> Result<Vec<ensemble::Hid>> {
        let ens = self
            .humds
            .read()
            .get(&observer)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", observer.short()))?;
        Ok(ens.ensemble.evict_expired(ttl))
    }

    pub fn peer_count(&self, observer: Hid) -> usize {
        self.humds.read().get(&observer).map(|h| h.ensemble.peers().len()).unwrap_or(0)
    }

    pub fn partition(&self, a: Hid, b: Hid) -> Result<()> {
        let links = self.links.read();
        let link = links
            .get(&link_key(a, b))
            .ok_or_else(|| anyhow::anyhow!("no link {}-{}", a.short(), b.short()))?;
        link.a_end.set_partitioned(true);
        link.b_end.set_partitioned(true);
        Ok(())
    }

    pub async fn heal(&self, a: Hid, b: Hid) -> Result<()> {
        let (link_a, link_b) = {
            let links = self.links.read();
            let link = links
                .get(&link_key(a, b))
                .ok_or_else(|| anyhow::anyhow!("no link {}-{}", a.short(), b.short()))?;
            (link.a, link.b)
        };
        {
            let links = self.links.read();
            let link = links.get(&link_key(a, b)).unwrap();
            link.a_end.set_partitioned(false);
            link.b_end.set_partitioned(false);
        }

        let (ha, hb) = {
            let humds = self.humds.read();
            (humds.get(&link_a).cloned(), humds.get(&link_b).cloned())
        };
        let ha = ha.ok_or_else(|| anyhow::anyhow!("no humd {}", link_a.short()))?;
        let hb = hb.ok_or_else(|| anyhow::anyhow!("no humd {}", link_b.short()))?;

        for (from, to) in [(&ha, &hb), (&hb, &ha)] {
            if let Err(e) = self.wane_sync(from, to).await {
                tracing::warn!(err = %e, "wane-sync.route.failed");
            }
        }
        Ok(())
    }

    pub async fn wane_sync(&self, from: &SimHumd, to: &SimHumd) -> Result<()> {
        let mut snapshot_json = serde_json::Map::new();
        for (sigil, n) in from.waneman.snapshot() {
            snapshot_json.insert(sigil, Value::from(n));
        }
        let tone = serde_json::json!({
            "chi": "wane-sync",
            "rid": hum_identity::HumId::mint().to_string(),
            "from": from.id.to_hex(),
            "to": to.id.to_hex(),
            "snapshot": Value::Object(snapshot_json),
        });
        from.ensemble.route(tone).await?;
        Ok(())
    }

    pub async fn attach_mock_worker(&self, humd: Hid, models: Vec<String>) -> Result<String> {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        for _ in 0..200 {
            if h.thrum.has_sink() { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let client_id = hum_identity::HumId::mint().to_string();
        let mut rx = h.thrum.register_synthetic(client_id.clone());
        let hello = serde_json::json!({
            "chi": "hello",
            "bee": ["worker"],
            "hive": "claude-repl",
            "version": "0.0.0",
            "protoVersion": thrum_core::THRUM_VERSION,
            "models": models,
            "chis": ["hello", "prompt", "chunk", "finish"],
        });
        h.thrum.inject_tone(&client_id, hello).await;
        let thrum = h.thrum.clone();
        let cid_for_pump = client_id.clone();
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if chi != "prompt" { continue; }
                let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if sid.is_empty() { continue; }
                let frames = vec![
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_start","id":0}),
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"text_delta","delta":"HELLO"}),
                    serde_json::json!({"chi":"chunk","sid":&sid,"chunkType":"content_block_stop","blockIdx":0}),
                    serde_json::json!({"chi":"finish","sid":&sid,"finishReason":"end_turn","usage":{}}),
                ];
                for f in frames {
                    thrum.inject_tone(&cid_for_pump, f).await;
                }
            }
        });
        Ok(client_id)
    }

    pub fn nestler_send(&self, humd: Hid, tone: Value) -> Result<String> {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        let client_id = hum_identity::HumId::mint().to_string();
        let mut rx = h.thrum.register_synthetic(client_id.clone());

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

        let thrum = h.thrum.clone();
        let cid = client_id.clone();
        tokio::spawn(async move {
            thrum.inject_tone(&cid, tone).await;
        });
        Ok(client_id)
    }

    pub async fn nestler_recv(
        &self,
        humd: Hid,
        sid: &str,
        timeout: Duration,
    ) -> Option<Value> {
        let h = self.humds.read().get(&humd).cloned()?;

        {
            let mut senders = h.sid_senders.lock();
            let mut mailboxes = h.sid_mailboxes.lock();
            if !senders.contains_key(sid) {
                let (tx, rx) = mpsc::unbounded_channel::<Value>();
                senders.insert(sid.to_string(), tx);
                mailboxes.insert(sid.to_string(), rx);
            }
        }

        {
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
                    let (tx, rx2) = mpsc::unbounded_channel::<Value>();
                    let _ = tx.send(tone);
                    h.sid_senders.lock().insert(tone_sid.clone(), tx);
                    h.sid_mailboxes.lock().insert(tone_sid, rx2);
                }
            }
        }
        }

        let mut rx_opt = h.sid_mailboxes.lock().remove(sid)?;
        let result = tokio::time::timeout(timeout, rx_opt.recv()).await.ok().flatten();
        h.sid_mailboxes.lock().insert(sid.to_string(), rx_opt);
        result
    }

    pub async fn attach_mock_forager<F>(
        &self,
        humd: Hid,
        hive: &str,
        tool_names: Vec<String>,
        responder: F,
    ) -> Result<String>
    where
        F: Fn(&str, Value) -> String + Send + Sync + 'static,
    {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        for _ in 0..200 {
            if h.thrum.has_sink() { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let client_id = hum_identity::HumId::mint().to_string();
        let mut rx = h.thrum.register_synthetic(client_id.clone());
        let tools: Vec<Value> = tool_names.iter().map(|name| serde_json::json!({
            "name": name,
            "description": format!("mock {name}"),
            "inputSchema": { "type": "object", "properties": {}, "required": [] }
        })).collect();
        let hello = serde_json::json!({
            "chi": "hello",
            "bee": ["forager"],
            "hive": hive,
            "version": "0.0.0",
            "protoVersion": thrum_core::THRUM_VERSION,
            "tools": tools,
            "chis": ["hello", "tool-call", "tool-result", "cancel"],
        });
        h.thrum.inject_tone(&client_id, hello).await;
        let thrum = h.thrum.clone();
        let cid_for_pump = client_id.clone();
        let responder = std::sync::Arc::new(responder);
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                let chi = tone.get("chi").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if chi != "tool-call" { continue; }
                let sid = tone.get("sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let call_id = tone.get("callId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let tool_name = tone.get("toolName").and_then(|v| v.as_str())
                    .or_else(|| tone.get("name").and_then(|v| v.as_str()))
                    .unwrap_or("").to_string();
                let args = tone.get("args").cloned().unwrap_or(Value::Null);
                let result = responder(&tool_name, args);
                let reply = serde_json::json!({
                    "chi": "tool-result",
                    "sid": sid,
                    "callId": call_id,
                    "toolName": tool_name,
                    "result": result,
                });
                thrum.inject_tone(&cid_for_pump, reply).await;
            }
        });
        Ok(client_id)
    }

    pub fn attach_observer(
        &self,
        observer_humd: Hid,
        host_humd: Hid,
        sid: &str,
    ) -> Result<String> {
        self.nestler_send(
            observer_humd,
            serde_json::json!({
                "chi": "attach",
                "rid": hum_identity::HumId::mint().to_string(),
                "sid": sid,
                "to": host_humd.to_hex(),
                "from": observer_humd.to_hex(),
                "hearOnly": true,
            }),
        )
    }

    pub async fn humd_peer_tap(&self, humd: Hid, timeout: Duration) -> Option<Value> {
        let h = self.humds.read().get(&humd).cloned()?;
        let mut rx = h.ensemble.subscribe();
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Ok(tone)) => Some(tone),
            _ => None,
        }
    }

    pub async fn await_handshake(&self, a: Hid, b: Hid) -> Result<()> {
        for _ in 0..200 {
            let done = {
                let (ha, hb) = {
                    let humds = self.humds.read();
                    (humds.get(&a).cloned(), humds.get(&b).cloned())
                };
                match (ha, hb) {
                    (Some(ha), Some(hb)) => {
                        ha.ensemble.handshake_done(&b) && hb.ensemble.handshake_done(&a)
                    }
                    _ => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    }
                }
            };
            if done {
                let hb = self.humds.read().get(&b).cloned().expect("checked above");
                let mut rx = hb.ensemble.subscribe();
                while rx.try_recv().is_ok() {}
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        bail!("{}-{} handshake timed out", a.short(), b.short())
    }

    pub fn humd_peer_sub(&self, humd: Hid) -> Option<ensemble::InboxSub> {
        Some(self.humds.read().get(&humd)?.ensemble.subscribe())
    }

    pub async fn collect_rids(
        rx: &mut ensemble::InboxSub,
        want: usize,
        window: Duration,
    ) -> Vec<String> {
        let mut out = Vec::with_capacity(want);
        let deadline = tokio::time::Instant::now() + window;
        while out.len() < want {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(tone)) => {
                    if let Some(rid) = tone["rid"].as_str() {
                        out.push(rid.to_string());
                    }
                }
                _ => break,
            }
        }
        out
    }

    pub async fn nestler_send_ordered(&self, humd: Hid, tone: Value) -> Result<String> {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        let cid = hum_identity::HumId::mint().to_string();
        let _ = h.thrum.register_synthetic(cid.clone());
        h.thrum.inject_tone(&cid, tone).await;
        Ok(cid)
    }

    pub async fn publish(
        &self,
        from: Hid,
        topic: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        let ens = self
            .humds
            .read()
            .get(&from)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", from.short()))?;
        ens.ensemble.publish(topic, payload).await;
        Ok(())
    }

    pub async fn publish_with_dusk(
        &self,
        from: Hid,
        topic: &str,
        payload: serde_json::Value,
        dusk_ms: i64,
    ) -> Result<()> {
        let ens = self
            .humds
            .read()
            .get(&from)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", from.short()))?;
        ens.ensemble.publish_with_dusk(topic, payload, Some(dusk_ms)).await;
        Ok(())
    }

    pub fn subscribe_topic(
        &self,
        humd: Hid,
        topic: &str,
    ) -> Result<tokio::sync::broadcast::Receiver<serde_json::Value>> {
        let ens = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        Ok(ens.ensemble.subscribe_topic(topic))
    }

    pub fn expired_dusk(&self, humd: Hid) -> u64 {
        self.humds.read().get(&humd).map_or(0, |h| h.ensemble.expired_dusk())
    }

    pub async fn send_marks(&self, from: Hid, to: Hid, tag: &str, n: usize) -> Result<()> {
        for i in 0..n {
            let tone = serde_json::json!({
                "chi": "perf-mark",
                "rid": format!("{tag}-{i}"),
                "to": to.to_hex(),
                "from": from.to_hex(),
                "mark": tag,
            });
            self.nestler_send_ordered(from, tone).await?;
        }
        Ok(())
    }

    pub fn rids(tones: &[Value]) -> Vec<String> {
        tones.iter().filter_map(|t| t["rid"].as_str().map(String::from)).collect()
    }

    pub fn ensemble_dropped(&self, humd: Hid) -> u64 {
        self.humds.read().get(&humd).map(|h| h.ensemble.inbox_dropped()).unwrap_or(0)
    }

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

#[allow(dead_code)]
fn _keep_path_alive(_p: PathBuf) {}
