use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use std::time::Duration;

use anyhow::{Result, bail};
use ensemble::{
    Ensemble, Hid, HumdKey, InMemoryEndpoint, LinkCounters, LinkFaults, PeerCapabilities,
    hello_tone,
};
use humd::thrumd::Thrum;
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use thrum_core::WaneTracker;
use tokio::sync::{mpsc, oneshot};

pub const SIM_HIVES: [&str; 4] = ["claude-cli", "claude-repl", "bash", "openai-server"];

fn sim_caps(_humd: &SimHumd) -> PeerCapabilities {
    PeerCapabilities {
        proto_version: thrum_core::THRUM_VERSION.to_string(),
        ..Default::default()
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
    pub waneman: Arc<WaneTracker>,
    pub key: Option<Arc<HumdKey>>,
    pub hives: humd::InstalledHives,
    pub bees: humd::EnteredBees,
}

impl SimHumd {
    pub fn enter(&self, hive: &str, role: thrum_core::Role) -> hum_identity::BeeKey {
        let key = hum_identity::BeeKey::generate();
        self.bees.enter(hive, role, &key);
        key
    }

    pub fn signed_hello(&self, mut hello: Value) -> Value {
        let hive = hello.get("hive").and_then(Value::as_str).unwrap_or("").to_string();
        let role = hello
            .get("role")
            .and_then(Value::as_str)
            .and_then(thrum_core::Role::parse)
            .unwrap_or(thrum_core::Role::Nurse);
        self.enter(&hive, role).sign_hello(&mut hello);
        hello
    }
}

pub struct Sim {
    humds: RwLock<HashMap<Hid, Arc<SimHumd>>>,
    links: RwLock<HashMap<(Hid, Hid), Link>>,
}

impl Drop for Sim {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let strays: Vec<String> = self
            .humds
            .read()
            .values()
            .flat_map(|h| h.thrum.wire_strays())
            .collect();
        assert!(
            strays.is_empty(),
            "tones carried keys the thrum does not declare:\n  {}",
            strays.join("\n  ")
        );
    }
}

#[derive(Clone)]
struct Link {
    a: Hid,
    b: Hid,
    a_end: Arc<InMemoryEndpoint>,
    b_end: Arc<InMemoryEndpoint>,
}

fn link_key(x: Hid, y: Hid) -> (Hid, Hid) {
    if x.to_hex() <= y.to_hex() {
        (x, y)
    } else {
        (y, x)
    }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Trust {
    pub nurses: bool,
    pub attendants: bool,
}

impl Trust {
    pub const ALL: Trust = Trust { nurses: true, attendants: true };
}

impl Sim {
    pub fn new() -> Self {
        Self {
            humds: RwLock::new(HashMap::new()),
            links: RwLock::new(HashMap::new()),
        }
    }

    pub async fn spawn_humd(&self, id: Hid) -> Arc<SimHumd> {
        self.spawn_humd_trusting(id, Trust::ALL).await
    }

    pub async fn spawn_humd_not_trusting_remote_nurses(&self, id: Hid) -> Arc<SimHumd> {
        self.spawn_humd_trusting(id, Trust { nurses: false, ..Trust::ALL }).await
    }

    pub async fn spawn_humd_not_trusting_remote_attendants(&self, id: Hid) -> Arc<SimHumd> {
        self.spawn_humd_trusting(id, Trust { attendants: false, ..Trust::ALL }).await
    }

    pub async fn spawn_humd_revoking(&self, id: Hid, revoked: &[&str]) -> Arc<SimHumd> {
        self.spawn_humd_inner_with_revocations(id, Trust::ALL, revoked)
            .await
    }

    pub async fn spawn_humd_trusting(&self, id: Hid, trust: Trust) -> Arc<SimHumd> {
        self.spawn_humd_inner_with_revocations(id, trust, &[])
            .await
    }

    async fn spawn_humd_inner_with_revocations(
        &self,
        id: Hid,
        trust: Trust,
        revoked: &[&str],
    ) -> Arc<SimHumd> {
        let thrum = Thrum::new();
        thrum.set_wire_check(humd::thrumd::WireCheck::Strict);
        let ensemble = Arc::new(Ensemble::with_strict_auth(id, false));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let tmp = std::env::temp_dir().join(format!("sim-humd-{}", id.short()));
        let _ = std::fs::create_dir_all(&tmp);
        let comb_path = tmp.join(hum_paths::COMB_BASENAME);
        let _ = std::fs::remove_file(tmp.join(hum_paths::BEES_SNAPSHOT_BASENAME));
        let _ = std::fs::remove_file(tmp.join(hum_paths::ADVISORIES_BASENAME));
        let revoked_path = tmp.join(hum_paths::REVOKED_BEES_BASENAME);
        let _ = std::fs::remove_file(&revoked_path);
        if !revoked.is_empty() {
            let list: Vec<String> = revoked.iter().map(|s| s.to_string()).collect();
            hum_paths::Revocations {
                version: 1,
                revoked: list,
            }
            .write_to(&revoked_path)
            .expect("write revocation list");
        }

        let waneman = Arc::new(WaneTracker::new());
        let hives = humd::InstalledHives::in_memory(SIM_HIVES);
        let bees = humd::EnteredBees::in_memory();
        let cfg = humd::DaemonConfig {
            thrum_path: tmp.join(hum_paths::THRUM_SOCK_BASENAME),
            http_path: tmp.join(hum_paths::HTTP_SOCK_BASENAME),
            comb_path,
            routing_path: tmp.join(hum_paths::ROUTING_JSON_BASENAME),
            routing_persist_interval: Duration::from_secs(3600),
            hum_cfg: hum_paths::config::HumConfig::default(),
            cli_path: "noop".into(),
            comb_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ensemble.clone()),
            revoked_path: revoked_path.clone(),
            bees_snapshot_path: tmp.join(hum_paths::BEES_SNAPSHOT_BASENAME),
            advisories_path: tmp.join(hum_paths::ADVISORIES_BASENAME),
            waneman: Some(waneman.clone()),
            humd_key: None,
            bootstrap_peers: Vec::new(),
            thehum_cfg: None,
            trust_remote_nurses: trust.nurses,
            trust_remote_attendants: trust.attendants,
            hives: hives.clone(),
            bees: bees.clone(),
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
            waneman,
            key: None,
            hives,
            bees,
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
        self.wire_unauthenticated(a, b)
    }

    pub fn wire_unauthenticated(&self, a: Hid, b: Hid) -> Result<()> {
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
        let (a_view, b_view) =
            InMemoryEndpoint::pair_concrete(ha.id, b_caps.clone(), hb.id, a_caps.clone());
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
        thrum.set_wire_check(humd::thrumd::WireCheck::Strict);
        let ensemble = Arc::new(Ensemble::with_strict_auth(id, true));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let tmp = std::env::temp_dir().join(format!("sim-humd-{}", id.short()));
        let _ = std::fs::create_dir_all(&tmp);
        let comb_path = tmp.join(hum_paths::COMB_BASENAME);
        let _ = std::fs::remove_file(tmp.join(hum_paths::BEES_SNAPSHOT_BASENAME));
        let _ = std::fs::remove_file(tmp.join(hum_paths::ADVISORIES_BASENAME));

        let waneman = Arc::new(WaneTracker::new());
        let hives = humd::InstalledHives::in_memory(SIM_HIVES);
        let bees = humd::EnteredBees::in_memory();
        let cfg = humd::DaemonConfig {
            thrum_path: tmp.join(hum_paths::THRUM_SOCK_BASENAME),
            http_path: tmp.join(hum_paths::HTTP_SOCK_BASENAME),
            comb_path,
            routing_path: tmp.join(hum_paths::ROUTING_JSON_BASENAME),
            routing_persist_interval: Duration::from_secs(3600),
            hum_cfg: hum_paths::config::HumConfig::default(),
            cli_path: "noop".into(),
            comb_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ensemble.clone()),
            revoked_path: tmp.join(hum_paths::REVOKED_BEES_BASENAME),
            bees_snapshot_path: tmp.join(hum_paths::BEES_SNAPSHOT_BASENAME),
            advisories_path: tmp.join(hum_paths::ADVISORIES_BASENAME),
            waneman: Some(waneman.clone()),
            humd_key: None,
            bootstrap_peers: Vec::new(),
            thehum_cfg: None,
            trust_remote_nurses: true,
            trust_remote_attendants: true,
            hives: hives.clone(),
            bees: bees.clone(),
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
            waneman,
            key: Some(Arc::new(key)),
            hives,
            bees,
        });

        self.humds.write().insert(id, sim_humd.clone());
        sim_humd
    }

    pub fn wire_signed(&self, a: Hid, b: Hid) -> Result<()> {
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
        let a_key = ha.key.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "humd {} has no signing key — spawn via spawn_humd_with_identity",
                a.short()
            )
        })?;
        let b_key = hb.key.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "humd {} has no signing key — spawn via spawn_humd_with_identity",
                b.short()
            )
        })?;

        let a_caps = sim_caps(&ha);
        let b_caps = sim_caps(&hb);
        let (a_view, b_view) = InMemoryEndpoint::pair(ha.id, b_caps.clone(), hb.id, a_caps.clone());
        ha.ensemble.install(a_view, a_caps, &a_key);
        hb.ensemble.install(b_view, b_caps, &b_key);
        Ok(())
    }

    pub fn wire_signed_tampered(&self, a: Hid, c: Hid) -> Result<()> {
        let humds = self.humds.read();
        let ha = humds
            .get(&a)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", a.short()))?;
        let hc = humds
            .get(&c)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", c.short()))?;
        drop(humds);
        let a_key = ha
            .key
            .clone()
            .ok_or_else(|| anyhow::anyhow!("humd {} has no signing key", a.short()))?;

        let a_caps = sim_caps(&ha);
        let c_caps = sim_caps(&hc);
        let (a_view, c_view) = InMemoryEndpoint::pair(ha.id, c_caps.clone(), hc.id, a_caps.clone());

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

    pub fn link_counters_since(&self, a: Hid, b: Hid, base: &LinkCounters) -> Result<LinkCounters> {
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

    pub fn evict_expired(
        &self,
        observer: Hid,
        ttl: std::time::Duration,
    ) -> Result<Vec<ensemble::Hid>> {
        let ens = self
            .humds
            .read()
            .get(&observer)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", observer.short()))?;
        Ok(ens.ensemble.evict_expired(ttl))
    }

    pub fn peer_count(&self, observer: Hid) -> usize {
        self.humds
            .read()
            .get(&observer)
            .map(|h| h.ensemble.peers().len())
            .unwrap_or(0)
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

    pub async fn attach_mock_nurse(&self, humd: Hid, models: Vec<String>) -> Result<String> {
        self.attach_mock_nurse_claiming(humd, models, None, "HELLO").await
    }

    pub async fn attach_mock_nurse_claiming(
        &self,
        humd: Hid,
        models: Vec<String>,
        headroom: Option<(u32, u32)>,
        reply: &str,
    ) -> Result<String> {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        for _ in 0..200 {
            if h.thrum.has_sink() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let client_id = hum_identity::HumId::mint().to_string();
        let mut rx = h.thrum.register_synthetic(client_id.clone());
        let mut hello = serde_json::json!({
            "chi": "hello",
            "role": "nurse",
            "hive": "claude-repl",
            "version": "0.0.0",
            "protoVersion": thrum_core::THRUM_VERSION,
            "models": models,
            "chis": ["hello", "prompt", "unfurl", "wilt"],
        });
        if let Some((free, total)) = headroom {
            hello["headroom"] = serde_json::json!({"freeSlots": free, "totalSlots": total});
        }
        let hello = h.signed_hello(hello);
        h.thrum.inject_tone(&client_id, hello).await;
        let thrum = h.thrum.clone();
        let cid_for_pump = client_id.clone();
        let reply = reply.to_string();
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                let chi = tone
                    .get("chi")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if chi != "prompt" {
                    continue;
                }
                let sid = tone
                    .get("sid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() {
                    continue;
                }
                let frames = vec![
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text"}),
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","text":&reply}),
                    serde_json::json!({"chi":"unfurl","sid":&sid,"petal":0,"kind":"text","closed":true}),
                    serde_json::json!({"chi":"wilt","sid":&sid,"reason":"end_turn","nectar":{}}),
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
                if sid.is_empty() {
                    continue;
                }
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

    pub async fn nestler_recv(&self, humd: Hid, sid: &str, timeout: Duration) -> Option<Value> {
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
            for rx in queues.values_mut() {
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
        let result = tokio::time::timeout(timeout, rx_opt.recv())
            .await
            .ok()
            .flatten();
        h.sid_mailboxes.lock().insert(sid.to_string(), rx_opt);
        result
    }

    pub async fn attach_mock_tools<F>(
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
            if h.thrum.has_sink() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        h.hives.install(hive);
        let client_id = hum_identity::HumId::mint().to_string();
        let mut rx = h.thrum.register_synthetic(client_id.clone());
        let tools: Vec<Value> = tool_names
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "description": format!("mock {name}"),
                    "inputSchema": { "type": "object", "properties": {}, "required": [] }
                })
            })
            .collect();
        let hello = h.signed_hello(serde_json::json!({
            "chi": "hello",
            "role": "attendant",
            "hive": hive,
            "version": "0.0.0",
            "protoVersion": thrum_core::THRUM_VERSION,
            "tools": tools,
            "chis": ["hello", "tendril-reach", "tendril-result"],
        }));
        h.thrum.inject_tone(&client_id, hello).await;
        let thrum = h.thrum.clone();
        let cid_for_pump = client_id.clone();
        let responder = std::sync::Arc::new(responder);
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                let chi = tone
                    .get("chi")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if chi != "tendril-reach" {
                    continue;
                }
                let sid = tone
                    .get("sid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let call_id = tone
                    .get("callId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = tone
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = tone.get("args").cloned().unwrap_or(Value::Null);
                let result = responder(&name, args);
                let reply = serde_json::json!({
                    "chi": "tendril-result",
                    "sid": sid,
                    "callId": call_id,
                    "result": result,
                });
                thrum.inject_tone(&cid_for_pump, reply).await;
            }
        });
        Ok(client_id)
    }

    pub async fn attach_mock_builder<F>(&self, humd: Hid, run: F) -> Result<String>
    where
        F: Fn(&str, &str) -> (String, i64) + Send + Sync + 'static,
    {
        let h = self
            .humds
            .read()
            .get(&humd)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no humd {}", humd.short()))?;
        for _ in 0..200 {
            if h.thrum.has_sink() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let client_id = hum_identity::HumId::mint().to_string();
        let mut rx = h.thrum.register_synthetic(client_id.clone());
        let hello = h.signed_hello(serde_json::json!({
            "chi": "hello",
            "role": "builder",
            "hive": "bash",
            "version": "0.0.0",
            "protoVersion": thrum_core::THRUM_VERSION,
            "chis": ["hello", "tendril-reach", "tendril-result", "cleanup"],
        }));
        h.thrum.inject_tone(&client_id, hello).await;
        let thrum = h.thrum.clone();
        let cid_for_pump = client_id.clone();
        tokio::spawn(async move {
            while let Some(tone) = rx.recv().await {
                if tone.get("chi").and_then(Value::as_str) != Some("tendril-reach") {
                    continue;
                }
                let text = |k: &str| tone.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                let command = tone
                    .pointer("/args/command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let (output, exit) = run(&text("cwd"), &command);
                let reply = serde_json::json!({
                    "chi": "tendril-result",
                    "sid": text("sid"),
                    "callId": text("callId"),
                    "output": output,
                    "isError": exit != 0,
                    "metadata": { "exitCode": exit },
                });
                thrum.inject_tone(&cid_for_pump, reply).await;
            }
        });
        Ok(client_id)
    }

    pub fn attach_observer(&self, observer_humd: Hid, host_humd: Hid, sid: &str) -> Result<String> {
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
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(tone)) => {
                    if matches!(
                        tone.get("chi").and_then(|v| v.as_str()),
                        Some("peer-add") | Some("peer-remove")
                    ) {
                        continue;
                    }
                    return Some(tone);
                }
                _ => return None,
            }
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

    pub async fn publish(&self, from: Hid, topic: &str, payload: serde_json::Value) -> Result<()> {
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
        ens.ensemble
            .publish_with_dusk(topic, payload, Some(dusk_ms))
            .await;
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
        self.humds
            .read()
            .get(&humd)
            .map_or(0, |h| h.ensemble.expired_dusk())
    }

    pub async fn send_marks(&self, from: Hid, to: Hid, tag: &str, n: usize) -> Result<()> {
        for i in 0..n {
            let tone = serde_json::json!({
                "chi": "perf-mark",
                "rid": format!("{tag}-{i}"),
                "to": to.to_hex(),
                "from": from.to_hex(),
                "note": tag,
            });
            self.nestler_send_ordered(from, tone).await?;
        }
        Ok(())
    }

    pub fn rids(tones: &[Value]) -> Vec<String> {
        tones
            .iter()
            .filter_map(|t| t["rid"].as_str().map(String::from))
            .collect()
    }

    pub fn ensemble_dropped(&self, humd: Hid) -> u64 {
        self.humds
            .read()
            .get(&humd)
            .map(|h| h.ensemble.inbox_dropped())
            .unwrap_or(0)
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
