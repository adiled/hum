
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ensemble::{Ensemble, Hid, HumdKey, PeerCapabilities};
use parking_lot::RwLock;
use serde_json::Value;
use thrumd::{serve_with_hook as thrum_serve_with_hook, Thrum, Tone, ToneSink};
use thrum_core::{Chi, WaneTracker};
use tracing::{info, trace, warn};

mod drone;
mod drift;
mod identity;
mod peer_transport;
pub mod peers;
mod penny;
pub mod redial;
mod routing_store;
pub mod supervisor;
pub mod thrumd;
pub use identity::{key_path, load_or_mint_key, read_key};
pub use peers::{peers_path, PeerConfig};

type Observers = Arc<RwLock<HashMap<String, Vec<Hid>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalCapacity {
    #[default]
    Unlimited,
    OverflowAlways,
    Slots(usize),
}

pub struct DaemonConfig {
    pub thrum_path: PathBuf,
    pub http_path: PathBuf,
    pub mcp_addr: std::net::SocketAddr,
    pub penny_path: PathBuf,
    pub routing_path: PathBuf,
    pub hum_cfg: hum_paths::config::HumConfig,
    pub cli_path: String,
    pub penny_persist_interval: Duration,
    pub routing_persist_interval: Duration,
    pub thrum_override: Option<Thrum>,
    pub ensemble: Option<Arc<Ensemble>>,
    pub bind_mcp: bool,
    pub capacity: LocalCapacity,
    pub waneman: Option<Arc<WaneTracker>>,
    pub humd_key: Option<Arc<HumdKey>>,
    pub bootstrap_peers: Vec<PeerConfig>,
    pub thehum_cfg: Option<thehum::Config>,
    pub trust_remote_workers: bool,
}

impl DaemonConfig {
    pub fn from_env() -> Self {
        let thrum_path = thrumd::default_socket_path();
        let http_path = hum_paths::http_sock();
        let mcp_port: u16 = std::env::var("HUM_MCP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(29147);
        let humd_key = match identity::load_or_mint_key() {
            Ok(k) => Some(Arc::new(k)),
            Err(e) => {
                warn!(err = %e, "identity.load.failed");
                None
            }
        };
        let bootstrap_peers = peers::load();
        Self {
            thrum_path,
            http_path,
            mcp_addr: ([127, 0, 0, 1], mcp_port).into(),
            penny_path: hum_paths::penny(),
            routing_path: hum_paths::routing_json(),
            hum_cfg: hum_paths::config::load(),
            cli_path: std::env::var("CLAUDE_CLI_PATH").unwrap_or_else(|_| "claude".into()),
            penny_persist_interval: Duration::from_secs(10),
            routing_persist_interval: Duration::from_secs(30),
            thrum_override: None,
            ensemble: None,
            bind_mcp: true,
            capacity: LocalCapacity::default(),
            waneman: None,
            humd_key,
            bootstrap_peers,
            thehum_cfg: None,
            trust_remote_workers: matches!(
                std::env::var("HUM_TRUST_REMOTE_WORKERS")
                    .ok()
                    .map(|v| v.trim().to_string())
                    .as_deref(),
                Some("1") | Some("true") | Some("yes")
            ),
        }
    }
}

pub async fn run<F>(mut cfg: DaemonConfig, shutdown: F) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send,
{
    hum_paths::init();

    info!(
        thrum = %cfg.thrum_path.display(),
        http = %cfg.http_path.display(),
        mcp = %cfg.mcp_addr,
        "humd.sockets"
    );

    if let Ok(addr) = cfg.hum_cfg.humd.metrics_addr.parse::<std::net::SocketAddr>() {
        match metrics_exporter_prometheus::PrometheusBuilder::new()
            .with_http_listener(addr)
            .install()
        {
            Ok(()) => info!(%addr, "humd.metrics.listening"),
            Err(e) => warn!(%addr, err = %e, "humd.metrics.install_failed"),
        }
    } else {
        warn!(addr = %cfg.hum_cfg.humd.metrics_addr, "humd.metrics.addr_parse_failed");
    }

    let penny = penny::Penny::load(&cfg.penny_path);
    penny.clone().spawn_persister(cfg.penny_path.clone(), cfg.penny_persist_interval);

    let waneman = cfg.waneman.clone().unwrap_or_else(|| Arc::new(WaneTracker::new()));
    let _drift = drift::Drift::with_store_dir(hum_paths::drift_dir());
    let _drone = drone::Drone::new();

    let thehum_handle: Option<Arc<thehum::TheHum>> = cfg.humd_key.as_ref().and_then(|k| {
        match thehum::TheHum::open(
            &hum_paths::thehum_dir(),
            k.0.clone(),
            cfg.thehum_cfg.clone().unwrap_or_default(),
        ) {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                warn!(err = %e, "thehum.open.failed");
                None
            }
        }
    });
    if let Some(t) = thehum_handle.as_ref() {
        info!(author = %t.author_hid(), dir = %t.dir().display(), "thehum.opened");
    }

    let ensemble_opt: Option<Arc<Ensemble>> = match cfg.ensemble.clone() {
        Some(e) => Some(e),
        None => cfg.humd_key.as_ref().map(|k| {
            let me = k.hid();
            info!(humd_id = %me, "ensemble.boot");
            Arc::new(Ensemble::new(me))
        }),
    };

    if let Some(ens) = &ensemble_opt {
        routing_store::restore_on_boot(ens, &cfg.routing_path);
        routing_store::spawn_persister(ens.clone(), cfg.routing_path.clone(), cfg.routing_persist_interval);
    }

    let mut peer_reach: Vec<String> = Vec::new();
    let mut supervisor: Option<supervisor::Supervisor> = None;
    if let (Some(ens), Some(key)) = (&ensemble_opt, &cfg.humd_key) {
        let my_caps = my_capabilities(&cfg);
        let mut iroh_transport: Option<Arc<ensemble::IrohTransport>> = None;

        match peer_transport::iroh::bind(key).await {
            Ok((transport, hints)) => {
                let transport = Arc::new(transport);
                peer_transport::iroh::dial_all(&transport, ens, key, &cfg.bootstrap_peers, &my_caps).await;
                peer_transport::iroh::spawn_listener(
                    transport.clone(),
                    ens.clone(),
                    key.clone(),
                    my_caps.clone(),
                );
                iroh_transport = Some(transport);
                peer_reach.extend(hints);
            }
            Err(e) => warn!(err = %e, "peer.iroh.bind_failed"),
        }

        if let Some(addr) = cfg.hum_cfg.humd.tcp_listen.as_deref().filter(|s| !s.is_empty()) {
            match peer_transport::tcp::spawn_listener(addr, ens.clone(), key.clone(), my_caps.clone()).await {
                Ok((_bound, hints)) => peer_reach.extend(hints),
                Err(e) => warn!(addr, err = %e, "peer.tcp.bind_failed"),
            }
        }

        peer_transport::tcp::dial_all(ens, key, &cfg.bootstrap_peers, &my_caps).await;

        if !cfg.bootstrap_peers.is_empty() {
            supervisor = Some(supervisor::Supervisor::new(
                ens.clone(),
                key.clone(),
                Arc::new(cfg.bootstrap_peers.clone()),
                my_caps,
                iroh_transport,
                supervisor::LivenessConfig::default(),
            ));
        }
    } else if !cfg.bootstrap_peers.is_empty() {
        warn!(
            count = cfg.bootstrap_peers.len(),
            "peers.skip.no-identity-or-ensemble"
        );
    }

    let ensemble_for_sink = ensemble_opt.clone();

    if let Some(sup) = supervisor {
        tokio::spawn(sup.run());
    }

    let is_embedded = cfg.thrum_override.is_some();
    let (thrum, bind_thrum) = match cfg.thrum_override.take() {
        Some(t) => (t, false),
        None => (Thrum::new(), true),
    };
    let observers: Observers = Arc::new(RwLock::new(HashMap::new()));
    let hive_tag = cfg.hum_cfg.nest.default.clone();
    let manifests: Manifests = Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let remote_hives: RemoteHives =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    if let Some(thehum) = thehum_handle.as_ref() {
        let manifests_for_replay = manifests.clone();
        if let Err(e) = thehum.replay(|event| {
            let body = &event.body;
            let client_id = body.get("client_id").and_then(Value::as_str)
                .or_else(|| body.get("nestlerId").and_then(Value::as_str))
                .or_else(|| body.get("from").and_then(Value::as_str))
                .map(str::to_string);
            let Some(client_id) = client_id else { return };
            match event.chi.as_str() {
                "hello" => {
                    let name = body.get("hive").and_then(Value::as_str)
                        .or_else(|| body.get("from").and_then(Value::as_str))
                        .unwrap_or(&client_id)
                        .to_string();
                    let version = body.get("version").and_then(Value::as_str)
                        .unwrap_or("0.0.0").to_string();
                    let proto = body.get("protoVersion").and_then(Value::as_str)
                        .unwrap_or(thrum_core::THRUM_VERSION).to_string();
                    let mut manifest = ensemble::HiveManifest::new(name, version, proto);
                    manifest.bee = body.get("bee").and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    manifest.models = body.get("models").and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    manifest.hid = body.get("hid").and_then(Value::as_str)
                        .and_then(|s| ensemble::Hid::from_hex(s).ok());
                    manifest.nestler_id = body.get("nestlerId").and_then(Value::as_str).map(str::to_string);
                    manifests_for_replay.write().insert(client_id, manifest);
                }
                "disconnect" | "forget" => {
                    manifests_for_replay.write().remove(&client_id);
                }
                _ => {}
            }
            let _ = event.ts_ms;
        }) {
            warn!(err = %e, "thehum.replay.failed");
        }
        info!(bees = manifests.read().len(), "thehum.replay.bees-derived");
    }
    let sid_origins: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let tool_routes: Arc<parking_lot::RwLock<HashMap<String, String>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let sid_fs: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let alias_resolver = Arc::new(PeersAliasResolver::from_peers(&cfg.bootstrap_peers));
    let tool_routes_peer: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let incoming_tool_calls: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let sink: Arc<dyn ToneSink> = Arc::new(HumdSink {
        thrum: thrum.clone(),
        waneman: waneman.clone(),
        ensemble: ensemble_for_sink.clone(),
        observers: observers.clone(),
        capacity: cfg.capacity,
        hive_tag: hive_tag.clone(),
        manifests: manifests.clone(),
        remote_hives: remote_hives.clone(),
        bees_snapshot_path: bees_snapshot_path(),
        sid_origins: sid_origins.clone(),
        tool_routes,
        sid_fs: sid_fs.clone(),
        alias_resolver: alias_resolver.clone(),
        tool_routes_peer: tool_routes_peer.clone(),
        incoming_tool_calls: incoming_tool_calls.clone(),
        thehum: thehum_handle.clone(),
        trust_remote_workers: cfg.trust_remote_workers,
    });
    thrum.set_sink(sink);
    if let Some(ens) = &ensemble_for_sink {
        let ens = ens.clone();
        let remote = remote_hives.clone();
        tokio::spawn(async move {
            let mut seen = ens.hive_discover_all();
            while let Some((humd_id, manifest)) = seen.recv().await {
                let key = bee_key(&manifest);
                remote.write().entry(humd_id).or_default().insert(key, manifest);
            }
        });
    }
    if bind_thrum {
        let thrum = thrum.clone();
        let path = cfg.thrum_path.clone();
        let humd_version = env!("CARGO_PKG_VERSION").to_string();
        let peer_reach = peer_reach.clone();
        tokio::spawn(async move {
            let res = thrum_serve_with_hook(thrum, &path, move |bound| {
                let info = hum_paths::RuntimeInfo {
                    socket: bound.to_path_buf(),
                    pid: std::process::id(),
                    version: humd_version,
                    thrum_version: thrum_core::THRUM_VERSION.to_string(),
                    bound_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64).unwrap_or(0),
                    ensemble_addrs: peer_reach.clone(),
                };
                if let Err(e) = info.write() {
                    warn!(err = %e, "humd.runtime_info.write_failed");
                } else {
                    info!(path = %hum_paths::runtime_info().display(), "humd.runtime_info.published");
                }
            }).await;
            if let Err(e) = res {
                warn!(err = %e, "thrum.exit");
            }
            hum_paths::RuntimeInfo::remove();
        });
    } else {
        trace!("thrum.override.installed");
    }

    if let Some(ens) = ensemble_for_sink.clone() {
        let mut rx = ens.subscribe();
        let thrum_for_pump = thrum.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(tone) => {
                        thrum_for_pump.inject_tone("ensemble", tone).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "ensemble.inbox.lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            trace!("ensemble.inbox.closed");
        });
    }

    if let Some(thehum) = thehum_handle.clone() {
        let manifests_for_snapshot = manifests.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_retention_ms: i64 = 0;
            loop {
                tick.tick().await;
                let now_ms: i64 = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                if thehum.should_snapshot(now_ms) {
                    let leaves: std::collections::BTreeMap<String, Value> = manifests_for_snapshot
                        .read()
                        .iter()
                        .map(|(cid, m)| (cid.clone(), serde_json::to_value(m).unwrap_or_default()))
                        .collect();
                    match thehum.snapshot(leaves).await {
                        Ok(root) => trace!(root = %hex::encode(root), "thehum.snapshot.ok"),
                        Err(e) => warn!(err = %e, "thehum.snapshot.failed"),
                    }
                }
                if now_ms.saturating_sub(last_retention_ms) >= 3_600_000 {
                    match thehum.enforce_retention() {
                        Ok(r) => trace!(removed = r.removed_files, kept = r.kept_files, "thehum.retention.ok"),
                        Err(e) => warn!(err = %e, "thehum.retention.failed"),
                    }
                    last_retention_ms = now_ms;
                }
            }
        });
    }

    if !is_embedded {
        tokio::spawn(autoupdate_loop());
    }

    info!("humd.ready");
    shutdown.await;
    info!("humd.shutting-down");
    if let Err(e) = penny.save(&cfg.penny_path) {
        warn!(err = %e, "penny.save.failed");
    }
    info!("humd.exit");
    Ok(())
}

async fn autoupdate_loop() {
    tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
    let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        match autoupdate_check_once().await {
            Ok(true) => info!("autoupdate.applied"),
            Ok(false) => trace!("autoupdate.up-to-date"),
            Err(e) => {
                warn!(err = %e, "autoupdate.failed");
                tokio::time::sleep(Duration::from_secs(15 * 60)).await;
            }
        }
    }
}

async fn autoupdate_check_once() -> Result<bool> {
    let local = env!("CARGO_PKG_VERSION").to_string();
    let body = tokio::process::Command::new("curl")
        .args([
            "-fsSL",
            "-H", "Accept: application/vnd.github+json",
            "https://api.github.com/repos/adiled/hum/releases/latest",
        ])
        .output()
        .await?;
    if !body.status.success() {
        anyhow::bail!("github releases fetch failed: {}", body.status);
    }
    let body = String::from_utf8(body.stdout)?;
    let upstream = parse_tag_name(&body).ok_or_else(|| anyhow::anyhow!("no tag_name in response"))?;
    let upstream_trim = upstream.trim_start_matches('v');
    if upstream_trim == local {
        return Ok(false);
    }
    info!(local = %local, upstream = %upstream_trim, "autoupdate.newer.found");
    let status = tokio::process::Command::new("bash")
        .arg("-c")
        .arg("curl -fsSL https://raw.githubusercontent.com/adiled/hum/main/install | bash")
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("installer exited with {status}");
    }
    Ok(true)
}

fn parse_tag_name(body: &str) -> Option<String> {
    let needle = "\"tag_name\":";
    let start = body.find(needle)? + needle.len();
    let rest = &body[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')?;
    Some(rest[q1..q1 + q2].to_string())
}

struct HumdSink {
    thrum: Thrum,
    waneman: Arc<WaneTracker>,
    ensemble: Option<Arc<Ensemble>>,
    observers: Observers,
    capacity: LocalCapacity,
    hive_tag: String,
    manifests: Manifests,
    remote_hives: RemoteHives,
    bees_snapshot_path: std::path::PathBuf,
    sid_origins: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>>,
    tool_routes: Arc<parking_lot::RwLock<HashMap<String, String>>>,
    sid_fs: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>>,
    alias_resolver: Arc<PeersAliasResolver>,
    tool_routes_peer: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>>,
    incoming_tool_calls: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>>,
    thehum: Option<Arc<thehum::TheHum>>,
    trust_remote_workers: bool,
}

pub struct PeersAliasResolver {
    by_alias: HashMap<String, ensemble::Hid>,
}

impl PeersAliasResolver {
    pub fn from_peers(peers: &[peers::PeerConfig]) -> Self {
        let mut by_alias = HashMap::new();
        for p in peers {
            if let Some(name) = &p.alias {
                by_alias.insert(name.clone(), p.humd_id);
            }
        }
        Self { by_alias }
    }
}

impl ensemble::AliasResolver for PeersAliasResolver {
    fn resolve(&self, alias: &str) -> Option<ensemble::Hid> {
        self.by_alias.get(alias).copied()
    }
}

type Manifests = Arc<parking_lot::RwLock<HashMap<String, ensemble::HiveManifest>>>;
type RemoteHives =
    Arc<parking_lot::RwLock<HashMap<ensemble::Hid, HashMap<String, ensemble::HiveManifest>>>>;

fn bee_key(manifest: &ensemble::HiveManifest) -> String {
    manifest
        .hid
        .map(|h| h.to_hex())
        .or_else(|| manifest.nestler_id.clone())
        .unwrap_or_else(|| manifest.name.clone())
}

fn bees_snapshot_path() -> std::path::PathBuf {
    hum_paths::bees_snapshot()
}

impl HumdSink {
    fn pick_remote_worker(&self, model: &str) -> Option<ensemble::Hid> {
        let ens = self.ensemble.as_ref()?;
        if !self.trust_remote_workers {
            warn!(
                model,
                "prompt.remote-routing.disabled — a peer can advertise any model and be handed the prompt; set HUM_TRUST_REMOTE_WORKERS=1 to accept that risk"
            );
            return None;
        }
        let live: std::collections::BTreeSet<String> =
            ens.peers().iter().map(|h| h.to_hex()).collect();
        let table = self.remote_hives.read();
        let mut found: Vec<String> = table
            .iter()
            .filter(|(humd, bees)| {
                live.contains(&humd.to_hex())
                    && bees.values().any(|m| {
                        m.bee.iter().any(|b| b == "worker") && m.models.iter().any(|x| x == model)
                    })
            })
            .map(|(humd, _)| humd.to_hex())
            .collect();
        found.sort();
        Hid::from_hex(found.first()?).ok()
    }

    fn snapshot_bees(&self) {
        let json = {
            let m = self.manifests.read();
            serde_json::to_vec_pretty(&*m).unwrap_or_default()
        };
        let path = &self.bees_snapshot_path;
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, path)).is_err() {
            trace!(path = %path.display(), "bees.snapshot.write.failed");
        }
    }
}

#[async_trait::async_trait]
impl ToneSink for HumdSink {
    async fn forget(&self, client_id: &str) {
        if client_id == "ensemble" { return; }
        let had_manifest = self.manifests.write().remove(client_id).is_some();
        self.tool_routes.write().retain(|_, originator| originator != client_id);
        if had_manifest {
            trace!(client_id, "manifest.evict.disconnect");
            self.snapshot_bees();
        }
    }

    async fn hear(&self, client_id: &str, tone: Tone) {
        let chi_str = tone.get("chi").and_then(Value::as_str).unwrap_or("?");
        let chi: Option<Chi> = tone
            .get("chi")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());

        if let Some(thehum) = self.thehum.as_ref() {
            if client_id != "ensemble" {
                let sid = tone.get("sid").and_then(Value::as_str).and_then(|s| {
                    hum_identity::HumId::parse(s).ok().or_else(|| Some(hum_identity::HumId::from_foreign(s)))
                });
                let rid = tone.get("rid").and_then(Value::as_str)
                    .map(|s| hum_identity::HumId::parse(s).unwrap_or_else(|_| hum_identity::HumId::from_foreign(s)))
                    .unwrap_or_else(hum_identity::HumId::mint);
                let body = serde_json::to_value(&tone).unwrap_or_default();
                if let Err(e) = thehum.append(chi_str, sid, rid, body).await {
                    warn!(client_id, %chi_str, err = %e, "thehum.append.failed");
                }
            }
        }

        if client_id == "ensemble" && matches!(chi, Some(Chi::ToolCall)) {
            let tool_name = tone.get("toolName").and_then(Value::as_str)
                .or_else(|| tone.get("name").and_then(Value::as_str))
                .map(str::to_string);
            if let Some(tool_name) = tool_name {
                let forager_cid = {
                    let m = self.manifests.read();
                    m.iter().find(|(_, man)| {
                        man.bee.iter().any(|b| b == "forager")
                        && man.tools.iter().any(|t| t.name == tool_name)
                    }).map(|(cid, _)| cid.clone())
                };
                if let Some(fcid) = forager_cid {
                    let call_id = tone.get("callId").and_then(Value::as_str)
                        .unwrap_or("").to_string();
                    let from_peer = tone.get("from").and_then(Value::as_str)
                        .and_then(parse_humd_id);
                    if let (Some(call_id), Some(from_peer)) = (
                        (!call_id.is_empty()).then_some(call_id.clone()),
                        from_peer,
                    ) {
                        self.incoming_tool_calls.write().insert(call_id, from_peer);
                    }
                    trace!(
                        from = "ensemble", to = %fcid, %tool_name,
                        "tool-call.peer.route.to-local-forager"
                    );
                    self.thrum.thrum_to(&fcid, tone);
                    return;
                } else {
                    warn!(%tool_name, "tool-call.peer.no-local-forager");
                    return;
                }
            }
        }

        if !matches!(chi, Some(Chi::Hello)) {
            let bee_kind: Vec<String> = {
                let m = self.manifests.read();
                m.get(client_id).map(|man| man.bee.clone()).unwrap_or_default()
            };
            let is_worker = bee_kind.iter().any(|b| b == "worker");
            let _ = &is_worker;
            if !is_worker
                && matches!(chi, Some(Chi::ToolCall))
                && client_id != "ensemble"
            {
                let sid = tone.get("sid").and_then(Value::as_str).map(str::to_string).unwrap_or_default();
                let tool_name = tone.get("toolName").and_then(Value::as_str)
                    .or_else(|| tone.get("name").and_then(Value::as_str))
                    .map(str::to_string);
                if !sid.is_empty() {
                    self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, &self.hive_tag));
                    self.thrum.claim_sigil(client_id, &sid);
                }
                if let Some(tn) = tool_name.as_ref() {
                    let forager_cid: Option<String> = self.manifests.read()
                        .iter()
                        .find(|(_, m)| m.bee.iter().any(|b| b == "forager")
                            && m.tools.iter().any(|t| &t.name == tn))
                        .map(|(cid, _)| cid.clone());
                    if let Some(fcid) = forager_cid {
                        let call_id = tone.get("callId").and_then(Value::as_str)
                            .unwrap_or("").to_string();
                        if !call_id.is_empty() {
                            self.tool_routes.write().insert(call_id.clone(), client_id.to_string());
                        }
                        trace!(from = client_id, to = %fcid, %tn, %call_id, "tool-call.route.from-asker");
                        self.thrum.thrum_to(&fcid, tone);
                        return;
                    }
                }
            }
            if is_worker {
                if matches!(chi, Some(Chi::ToolCall)) {
                    let sid_for_lookup = tone.get("sid").and_then(Value::as_str)
                        .map(str::to_string).unwrap_or_default();
                    if !sid_for_lookup.is_empty() {
                        self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid_for_lookup, &self.hive_tag));
                        self.thrum.claim_sigil(client_id, &sid_for_lookup);
                    }
                    let tool_name = tone.get("toolName").and_then(Value::as_str)
                        .or_else(|| tone.get("name").and_then(Value::as_str))
                        .map(str::to_string);
                    let pinned_fs = self.sid_fs.read().get(&sid_for_lookup).copied();

                    if let (Some(fs_hid), Some(ens), Some(tn)) =
                        (pinned_fs, &self.ensemble, tool_name.as_ref())
                    {
                        if fs_hid != ens.me() {
                            let call_id = tone.get("callId").and_then(Value::as_str)
                                .unwrap_or("").to_string();
                            if !call_id.is_empty() {
                                self.tool_routes.write().insert(
                                    call_id.clone(), client_id.to_string()
                                );
                                self.tool_routes_peer.write().insert(
                                    call_id.clone(), fs_hid
                                );
                            }
                            let mut routed = tone.clone();
                            if let Some(obj) = routed.as_object_mut() {
                                obj.insert("to".into(), Value::String(fs_hid.to_hex()));
                                obj.insert("from".into(), Value::String(ens.me().to_hex()));
                            }
                            trace!(
                                from = client_id, to = %fs_hid.short(),
                                tool_name = %tn, %call_id,
                                "tool-call.route.to-peer-forager"
                            );
                            if let Err(e) = ens.route(routed).await {
                                warn!(err = %e, "tool-call.peer.route.failed");
                            }
                            return;
                        }
                    }

                    if let Some(tool_name) = tool_name {
                        let forager_cid = {
                            let m = self.manifests.read();
                            m.iter().find(|(_, man)| {
                                man.bee.iter().any(|b| b == "forager")
                                && man.tools.iter().any(|t| t.name == tool_name)
                            }).map(|(cid, _)| cid.clone())
                        };
                        if let Some(fcid) = forager_cid {
                            let call_id = tone.get("callId").and_then(Value::as_str)
                                .unwrap_or("").to_string();
                            if !call_id.is_empty() {
                                self.tool_routes.write().insert(
                                    call_id.clone(), client_id.to_string()
                                );
                            }
                            trace!(
                                from = client_id, to = %fcid, %tool_name, %call_id,
                                "tool-call.route.to-forager"
                            );
                            self.thrum.thrum_to(&fcid, tone);
                            return;
                        }
                    }
                }
                if let Some(sid) = tone.get("sid").and_then(Value::as_str).map(str::to_string) {
                    if matches!(chi,
                        Some(Chi::Chunk) | Some(Chi::Finish) | Some(Chi::Error)
                        | Some(Chi::ToolCall) | Some(Chi::ToolInfo) | Some(Chi::SessionReady)
                        | Some(Chi::Pulse) | Some(Chi::Breath)
                    ) {
                        if let Some(ens) = &self.ensemble {
                            let obs = self.observers.read().get(&sid).cloned().unwrap_or_default();
                            for peer in obs {
                                let mut copy = tone.clone();
                                if let Some(obj) = copy.as_object_mut() {
                                    obj.insert("to".into(), Value::String(peer.to_hex()));
                                    obj.insert("from".into(), Value::String(ens.me().to_hex()));
                                }
                                if let Err(e) = ens.route(copy).await {
                                    warn!(err = %e, "worker.reply.observer.failed");
                                }
                            }
                        }
                        let origin = self.sid_origins.read().get(&sid).cloned();
                        if let (Some(origin), Some(ens)) = (origin, &self.ensemble) {
                            let mut copy = tone.clone();
                            if let Some(obj) = copy.as_object_mut() {
                                obj.insert("to".into(), Value::String(origin.to_hex()));
                                obj.insert("from".into(), Value::String(ens.me().to_hex()));
                            }
                            if let Err(e) = ens.route(copy).await {
                                warn!(err = %e, "worker.reply.ensemble.failed");
                            }
                        }
                        self.thrum.thrum_broadcast(&sid, &self.hive_tag, tone.clone());
                        return;
                    }
                }
            }
        }

        if matches!(chi, Some(Chi::Attach)) && client_id != "ensemble" {
            if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                if !sid.is_empty() {
                    self.thrum.claim_sigil(client_id, thrum_core::sigil(sid, &self.hive_tag));
                    self.thrum.claim_sigil(client_id, sid.to_string());
                    let hear_only = tone.get("hearOnly").and_then(Value::as_bool).unwrap_or(false);
                    trace!(client_id, sid, hear_only, "attach.local.claimed");
                }
            }
        }

        if let Some(ensemble) = &self.ensemble {
            if let Some(to) = tone.get("to").and_then(Value::as_str) {
                if !to.is_empty() && to != ensemble.me().to_hex() {
                    trace!(client_id, %chi_str, to, "ensemble.route");
                    if let Err(e) = ensemble.route(tone).await {
                        warn!(client_id, err = %e, "ensemble.route.failed");
                    }
                    return;
                }
            }
        }

        if client_id == "ensemble" {
            let is_reply = matches!(
                chi,
                Some(Chi::Chunk)
                    | Some(Chi::Finish)
                    | Some(Chi::Error)
                    | Some(Chi::SessionReady)
                    | Some(Chi::Pulse)
                    | Some(Chi::ToolCall)
                    | Some(Chi::ToolMeta)
                    | Some(Chi::PermissionAsk)
            );
            if is_reply {
                let sid_opt = tone.get("sid").and_then(Value::as_str).map(str::to_string);
                if let Some(sid) = sid_opt {
                    trace!(client_id, %chi_str, %sid, "thrum.recv.peer-reply.forward");
                    self.thrum.thrum_broadcast(&sid, &self.hive_tag, tone);
                    return;
                }
            }
        }

        match chi {
            Some(Chi::Hello) => {
                trace!(client_id, %chi_str, "thrum.recv.hello");
                let breath = thrumd::breath_tone(serde_json::json!({}));
                self.thrum.thrum_to(client_id, breath);

                let bee: Vec<String> = match tone.get("bee") {
                    Some(Value::Array(arr)) => arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    Some(Value::String(s)) => vec![s.clone()],
                    _ => Vec::new(),
                };
                let models: Vec<String> = tone.get("models")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                let name = tone.get("hive").and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| tone.get("from").and_then(Value::as_str).map(str::to_string));
                if let Some(name) = name {
                    let proto = tone.get("protoVersion").and_then(Value::as_str)
                        .unwrap_or(thrum_core::THRUM_VERSION).to_string();
                    let version = tone.get("version").and_then(Value::as_str)
                        .unwrap_or("0.0.0").to_string();
                    let propensity = tone.get("propensity")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let chis: Vec<String> = tone.get("chis")
                        .or_else(|| tone.get("chi"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    let source = tone.get("source").and_then(Value::as_str).map(str::to_string);
                    let bind: Option<ensemble::BindAddr> = tone.get("bind")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let nestler_id = tone.get("nestlerId").and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| client_id.to_string());
                    let mut manifest = ensemble::HiveManifest::new(name, version, proto);
                    manifest.propensity = propensity;
                    manifest.chis = chis;
                    manifest.source = source;
                    manifest.bind = bind;
                    manifest.nestler_id = Some(nestler_id);
                    manifest.bee = bee.clone();
                    manifest.models = models.clone();
                    let raw_hid = tone.get("hid").and_then(Value::as_str);
                    manifest.hid = raw_hid.and_then(|s| ensemble::Hid::from_hex(s).ok());
                    match (raw_hid, manifest.hid) {
                        (Some(_), Some(hid)) => {
                            trace!(client_id, hid = %hid.short(), "bee.hid.registered");
                        }
                        (Some(bad), None) => {
                            warn!(client_id, bad_hid = %bad,
                                "bee.hid.invalid — not a canonical Hid; reconnect dedup disabled for this bee. \
                                 Derive a stable hid from a persisted key (see hives/common identity).");
                        }
                        (None, _) => {
                            warn!(client_id,
                                "bee.hid.missing — hello has no hid; reconnect dedup disabled. \
                                 Ghost manifests will accumulate on reconnect.");
                        }
                    }
                    if let Some(arr) = tone.get("tools").and_then(Value::as_array) {
                        manifest.tools = arr.iter().filter_map(|v| {
                            let name = v.get("name").and_then(Value::as_str)?.to_string();
                            let description = v.get("description").and_then(Value::as_str)
                                .unwrap_or("").to_string();
                            let input_schema = v.get("inputSchema").cloned().unwrap_or(Value::Null);
                            Some(ensemble::ToolEntry { name, description, input_schema })
                        }).collect();
                    }
                    if let Some(arr) = tone.get("provides").and_then(Value::as_array) {
                        manifest.provides = arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect();
                        if !manifest.provides.is_empty() {
                            info!(
                                client_id,
                                provides = ?manifest.provides,
                                "forager.capabilities.registered"
                            );
                        }
                        if !manifest.tools.is_empty() {
                            info!(
                                client_id,
                                count = manifest.tools.len(),
                                "forager.tools.registered"
                            );
                        }
                    }

                    if client_id != "ensemble" {
                        let mut m = self.manifests.write();
                        if let Some(new_hid) = manifest.hid {
                            let stale: Vec<String> = m.iter()
                                .filter_map(|(cid, mf)| match mf.hid {
                                    Some(h) if h == new_hid && cid != client_id => Some(cid.clone()),
                                    _ => None,
                                })
                                .collect();
                            for cid in &stale {
                                trace!(stale_client_id = %cid, hid = %new_hid.short(),
                                    "manifest.evict.stale-rehello");
                                m.remove(cid);
                            }
                        }
                        m.insert(client_id.to_string(), manifest.clone());
                        drop(m);
                        self.snapshot_bees();
                        if bee.iter().any(|b| b == "worker") {
                            info!(client_id, ?models, "worker.registered");
                        }
                    }

                    if client_id != "ensemble" {
                        if let Some(ensemble) = &self.ensemble {
                            let ens = ensemble.clone();
                            tokio::spawn(async move {
                                ens.hive_advertise(manifest).await;
                            });
                        }
                    }
                }
            }
            Some(Chi::Prompt) => {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if sid.is_empty() {
                    warn!(client_id, "prompt.no-sid");
                    return;
                }
                if client_id != "ensemble"
                    && self.capacity == LocalCapacity::OverflowAlways
                {
                    if let Some(ensemble) = &self.ensemble {
                        let target = pick_overflow_peer(ensemble, &self.hive_tag);
                        if let Some(peer) = target {
                            self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, &self.hive_tag));
                            self.thrum.claim_sigil(client_id, &sid);
                            if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                                self.thrum.thrum_to(client_id, thrumd::echo_tone(rid, true, None));
                            }
                            let mut forward = tone.clone();
                            if let Some(obj) = forward.as_object_mut() {
                                obj.insert("to".into(), Value::String(peer.to_hex()));
                                obj.insert("from".into(), Value::String(ensemble.me().to_hex()));
                            }
                            trace!(sid, peer = %peer.short(), "overflow.route");
                            if let Err(e) = ensemble.route(forward).await {
                                warn!(sid, err = %e, "overflow.route.failed");
                            }
                            return;
                        } else {
                            warn!(sid, "overflow.no-route");
                        }
                    }
                }
                let model = tone.get("modelId").and_then(Value::as_str).unwrap_or("sonnet").to_string();
                let cwd_raw = tone.get("cwd").and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| "/".into());
                let cwd = if ensemble::HumUri::starts_with_scheme(&cwd_raw) {
                    match ensemble::HumUri::parse(&cwd_raw) {
                        Ok(uri) => {
                            let fs_hid = match &uri.host {
                                ensemble::HostRef::Hid(h) => Some(*h),
                                ensemble::HostRef::Alias(name) => {
                                    use ensemble::AliasResolver;
                                    self.alias_resolver.resolve(name)
                                }
                            };
                            if let Some(fs_hid) = fs_hid {
                                self.sid_fs.write().insert(sid.clone(), fs_hid);
                                trace!(sid, fs_hid = %fs_hid.short(), "prompt.fs.pinned");
                            } else {
                                warn!(sid, uri = %cwd_raw, "prompt.fs.alias.unknown");
                            }
                            format!("/{}", uri.path)
                        }
                        Err(e) => {
                            warn!(uri = %cwd_raw, err = %e, "prompt.cwd.uri.parse.failed");
                            cwd_raw
                        }
                    }
                } else {
                    cwd_raw
                };
                let system_prompt = tone.get("systemPrompt").and_then(Value::as_str).map(str::to_string);
                let text = tone.get("text").and_then(Value::as_str).map(str::to_string)
                    .or_else(|| tone.get("content").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                    self.thrum.thrum_to(client_id, thrumd::echo_tone(rid, true, None));
                }
                self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, &self.hive_tag));
                self.thrum.claim_sigil(client_id, &sid);
                let origin = if client_id == "ensemble" {
                    tone.get("from")
                        .and_then(Value::as_str)
                        .and_then(parse_humd_id)
                        .filter(|h| {
                            self.ensemble
                                .as_ref()
                                .map(|e| *h != e.me())
                                .unwrap_or(false)
                        })
                } else {
                    None
                };
                trace!(sid, model, ?origin, "thrum.recv.prompt");

                let worker_client = {
                    let mut to_prune: Vec<String> = Vec::new();
                    let pick = {
                        let m = self.manifests.read();
                        let mut found: Option<String> = None;
                        let mut sorted: Vec<_> = m.iter().collect();
                        sorted.sort_by(|a, b| a.0.cmp(b.0));
                        for (cid, man) in sorted {
                            if man.bee.iter().any(|b| b == "worker")
                                && man.models.iter().any(|m| m == &model)
                            {
                                if self.thrum.is_connected(cid) {
                                    found = Some(cid.clone());
                                    break;
                                } else {
                                    to_prune.push(cid.clone());
                                }
                            }
                        }
                        found
                    };
                    if !to_prune.is_empty() {
                        let mut m = self.manifests.write();
                        for cid in &to_prune { m.remove(cid); }
                    }
                    pick
                };
                let Some(worker_client) = worker_client else {
                    if let Some(peer) = self.pick_remote_worker(&model)
                        && let Some(ens) = self.ensemble.clone()
                    {
                        let mut forward = tone.clone();
                        if let Some(obj) = forward.as_object_mut() {
                            obj.insert("to".into(), Value::String(peer.to_hex()));
                            obj.insert("from".into(), Value::String(ens.me().to_hex()));
                        }
                        trace!(sid, model, peer = %peer.short(), "prompt.forward.remote");
                        match ens.route(forward).await {
                            Ok(()) => return,
                            Err(e) => warn!(sid, peer = %peer.short(), err = %e,
                                "prompt.forward.remote.failed"),
                        }
                    }
                    warn!(sid, model, "prompt.no-worker — no bee on this mesh advertises this model");
                    let err = serde_json::json!({
                        "chi": "error",
                        "sid": sid,
                        "message": format!("no worker bee advertises model '{}'", model),
                    });
                    if let (Some(origin), Some(ens)) = (origin, &self.ensemble) {
                        let mut out = err.clone();
                        if let Some(obj) = out.as_object_mut() {
                            obj.insert("to".into(), Value::String(origin.to_hex()));
                            obj.insert("from".into(), Value::String(ens.me().to_hex()));
                        }
                        let _ = ens.route(out).await;
                    }
                    self.thrum.thrum_broadcast(&sid, &self.hive_tag, err);
                    return;
                };

                let mut forward = tone.clone();
                if let Some(obj) = forward.as_object_mut() {
                    if obj.get("cwd").is_none() {
                        obj.insert("cwd".into(), Value::String(cwd.clone()));
                    }
                    if obj.get("content").is_none() && !text.is_empty() {
                        obj.insert("content".into(), Value::String(text.clone()));
                    }
                    if obj.get("systemPrompt").is_none() {
                        if let Some(sp) = system_prompt.as_ref() {
                            obj.insert("systemPrompt".into(), Value::String(sp.clone()));
                        }
                    }
                    let (forager_tools_json, provided_caps): (Vec<Value>, Vec<String>) = {
                        let m = self.manifests.read();
                        let mut tools: Vec<Value> = Vec::new();
                        let mut caps: std::collections::BTreeSet<String> =
                            std::collections::BTreeSet::new();
                        let mut sorted: Vec<_> = m.iter().collect();
                        sorted.sort_by(|a, b| a.0.cmp(b.0));
                        for (_, man) in sorted {
                            if !man.bee.iter().any(|b| b == "forager") { continue; }
                            for t in &man.tools {
                                tools.push(serde_json::json!({
                                    "name": t.name,
                                    "description": t.description,
                                    "inputSchema": t.input_schema,
                                }));
                            }
                            for c in &man.provides { caps.insert(c.clone()); }
                        }
                        (tools, caps.into_iter().collect())
                    };
                    if !forager_tools_json.is_empty() {
                        obj.insert("foragerTools".into(), Value::Array(forager_tools_json));
                    }
                    if !provided_caps.is_empty() {
                        obj.insert("provided".into(),
                            Value::Array(provided_caps.iter().cloned().map(Value::String).collect()));
                    }

                    let mut disallowed: std::collections::BTreeSet<String> =
                        obj.get("disallowedTools")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                            .unwrap_or_default();
                    for cap in &provided_caps {
                        if let Some(names) = hum_mcp::capability::capability_tools(cap) {
                            for n in names { disallowed.insert((*n).into()); }
                        }
                    }
                    if !disallowed.is_empty() {
                        let arr: Vec<Value> = disallowed.into_iter().map(Value::String).collect();
                        obj.insert("disallowedTools".into(), Value::Array(arr));
                    }
                }
                trace!(sid, model, worker_client = %worker_client, "prompt.forward.to-worker");
                if let Some(origin) = origin {
                    self.sid_origins.write().insert(sid.clone(), origin);
                }
                self.thrum.thrum_to(&worker_client, forward);
                self.waneman.tick(&sid);
            }
            Some(Chi::Cancel) => {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                    let workers: Vec<String> = self.manifests.read()
                        .iter()
                        .filter(|(_, m)| m.bee.iter().any(|b| b == "worker"))
                        .map(|(cid, _)| cid.clone())
                        .collect();
                    for wc in workers {
                        self.thrum.thrum_to(&wc, tone.clone());
                    }
                    let _ = sid;
                }
            }
            Some(Chi::Cleanup) => {
                if let Some(_sid) = tone.get("sid").and_then(Value::as_str) {
                    let workers: Vec<String> = self.manifests.read()
                        .iter()
                        .filter(|(_, m)| m.bee.iter().any(|b| b == "worker"))
                        .map(|(cid, _)| cid.clone())
                        .collect();
                    for wc in workers {
                        self.thrum.thrum_to(&wc, tone.clone());
                    }
                }
            }
            Some(Chi::Attach) => {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if sid.is_empty() {
                    warn!(client_id, "attach.no-sid");
                    return;
                }
                let hear_only = tone.get("hearOnly").and_then(Value::as_bool).unwrap_or(false);
                if client_id == "ensemble" {
                    let peer = tone.get("from").and_then(Value::as_str).and_then(parse_humd_id);
                    if let Some(peer) = peer {
                        let mut obs = self.observers.write();
                        let list = obs.entry(sid.clone()).or_default();
                        if !list.contains(&peer) {
                            list.push(peer);
                        }
                        trace!(client_id, sid, peer = %peer.short(), hear_only, "observer.registered");
                    } else {
                        warn!(client_id, sid, "attach.bad-from");
                    }
                } else {
                    self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, &self.hive_tag));
                    self.thrum.claim_sigil(client_id, &sid);
                    if let Some(ensemble) = &self.ensemble {
                        if let Some(to) = tone.get("to").and_then(Value::as_str) {
                            if !to.is_empty() && to != ensemble.me().to_hex() {
                                trace!(client_id, sid, to, hear_only, "attach.forward");
                                let mut forward = tone.clone();
                                if let Some(obj) = forward.as_object_mut() {
                                    obj.entry("from".to_string())
                                        .or_insert_with(|| Value::String(ensemble.me().to_hex()));
                                }
                                if let Err(e) = ensemble.route(forward).await {
                                    warn!(client_id, sid, err = %e, "attach.forward.failed");
                                }
                            }
                        }
                    }
                }
            }
            Some(Chi::Detach) => {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if sid.is_empty() {
                    warn!(client_id, "detach.no-sid");
                    return;
                }
                if client_id == "ensemble" {
                    let peer = tone.get("from").and_then(Value::as_str).and_then(parse_humd_id);
                    if let Some(peer) = peer {
                        let mut obs = self.observers.write();
                        if let Some(list) = obs.get_mut(&sid) {
                            list.retain(|p| *p != peer);
                            if list.is_empty() { obs.remove(&sid); }
                        }
                        trace!(client_id, sid, peer = %peer.short(), "observer.removed");
                    }
                } else if let Some(ensemble) = &self.ensemble {
                    if let Some(to) = tone.get("to").and_then(Value::as_str) {
                        if !to.is_empty() && to != ensemble.me().to_hex() {
                            let mut forward = tone.clone();
                            if let Some(obj) = forward.as_object_mut() {
                                obj.entry("from".to_string())
                                    .or_insert_with(|| Value::String(ensemble.me().to_hex()));
                            }
                            if let Err(e) = ensemble.route(forward).await {
                                warn!(client_id, sid, err = %e, "detach.forward.failed");
                            }
                        }
                    }
                }
            }
            Some(Chi::PeerAdd) => {
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.add");
                if let Some(ens) = self.ensemble.clone() {
                    let known = self.manifests.read().values().cloned().collect::<Vec<_>>();
                    tokio::spawn(async move {
                        for manifest in known {
                            ens.hive_advertise(manifest).await;
                        }
                    });
                }
            }
            Some(Chi::PeerRemove) => {
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.remove");
                if let Some(ensemble) = &self.ensemble {
                    if let Ok(bytes) = hex::decode(humd_id) {
                        if bytes.len() == 32 {
                            let mut id = [0u8; 32];
                            id.copy_from_slice(&bytes);
                            let gone = ensemble::Hid::from(id);
                            ensemble.remove_peer(&gone);
                            if self.remote_hives.write().remove(&gone).is_some() {
                                trace!(peer = %gone.short(), "discovery.evicted");
                            }
                        }
                    }
                }
            }
            Some(Chi::WaneSync) => {
                let snapshot = tone
                    .get("snapshot")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let mut remote: HashMap<String, u64> = HashMap::new();
                for (sigil, v) in snapshot {
                    if let Some(n) = v.as_u64() {
                        remote.insert(sigil, n);
                    }
                }
                let advanced = self.waneman.merge(&remote);
                trace!(
                    client_id,
                    entries = remote.len(),
                    advanced,
                    "thrum.recv.wane-sync"
                );
            }
            Some(Chi::Error) => {
                let call_id = tone.get("callId").and_then(Value::as_str).map(str::to_string);
                if let Some(cid) = call_id.as_deref() {
                    let origin_peer = self.incoming_tool_calls.read().get(cid).copied();
                    if let (Some(origin_peer), Some(ens)) = (origin_peer, &self.ensemble) {
                        let mut routed = tone.clone();
                        if let Some(obj) = routed.as_object_mut() {
                            obj.insert("to".into(), Value::String(origin_peer.to_hex()));
                            obj.insert("from".into(), Value::String(ens.me().to_hex()));
                        }
                        trace!(call_id = cid, to = %origin_peer.short(), "error.route.to-origin-peer");
                        if let Err(e) = ens.route(routed).await {
                            warn!(err = %e, "error.peer.route.failed");
                        }
                        return;
                    }
                    if let Some(originator) = self.tool_routes.read().get(cid).cloned() {
                        trace!(call_id = cid, %originator, "error.route.to-originator");
                        self.thrum.thrum_to(&originator, tone.clone());
                        return;
                    }
                }
                if let Some(sid) = tone.get("sid").and_then(Value::as_str).map(str::to_string) {
                    trace!(%sid, "error.broadcast.fallback");
                    self.thrum.thrum_broadcast(&sid, &self.hive_tag, tone);
                }
            }
            Some(Chi::ToolResult) => {
                let call_id = tone.get("callId").and_then(Value::as_str);

                if let Some(call_id) = call_id {
                    let origin_peer = self.incoming_tool_calls.write().remove(call_id);
                    if let Some(origin_peer) = origin_peer {
                        if let Some(ens) = &self.ensemble {
                            let mut routed = tone.clone();
                            if let Some(obj) = routed.as_object_mut() {
                                obj.insert("to".into(), Value::String(origin_peer.to_hex()));
                                obj.insert("from".into(), Value::String(ens.me().to_hex()));
                            }
                            trace!(call_id, to = %origin_peer.short(),
                                "tool_result.route.to-origin-peer");
                            if let Err(e) = ens.route(routed).await {
                                warn!(err = %e, "tool_result.peer.route.failed");
                            }
                            return;
                        }
                    }
                    self.tool_routes_peer.write().remove(call_id);
                    if let Some(worker_cid) = self.tool_routes.write().remove(call_id) {
                        trace!(call_id, %worker_cid, "tool_result.route.to-worker");
                        self.thrum.thrum_to(&worker_cid, tone.clone());
                        return;
                    }
                }
                if let Some(call_id) = call_id {
                    trace!(call_id, "tool_result.unrouted");
                }
            }
            Some(Chi::Backfill) => {
                let Some(thehum) = self.thehum.as_ref() else {
                    trace!(client_id, "backfill.no-thehum");
                    return;
                };
                let author = tone.get("author").and_then(Value::as_str).unwrap_or("").to_string();
                let from = tone.get("from").and_then(Value::as_u64).unwrap_or(0);
                if author.is_empty() {
                    warn!(client_id, "backfill.no-author");
                    return;
                }
                match thehum.range(&author, from) {
                    Ok(events) => {
                        trace!(client_id, %author, from, count = events.len(), "backfill.serve");
                        for ev in events {
                            let body = serde_json::to_value(&ev).unwrap_or_default();
                            let reply = serde_json::json!({
                                "chi": "backfill-event",
                                "rid": ev.rid,
                                "event": body,
                            });
                            self.thrum.thrum_to(client_id, reply);
                        }
                    }
                    Err(e) => warn!(client_id, %author, from, err = %e, "backfill.range.failed"),
                }
            }
            Some(Chi::Curate) => {
                if let Some(_sid) = tone.get("sid").and_then(Value::as_str) {
                    let workers: Vec<String> = self.manifests.read()
                        .iter()
                        .filter(|(_, m)| m.bee.iter().any(|b| b == "worker"))
                        .map(|(cid, _)| cid.clone())
                        .collect();
                    for wc in workers {
                        self.thrum.thrum_to(&wc, tone.clone());
                    }
                }
            }
            Some(Chi::ReleasePermit)
            | Some(Chi::TendrilResult)
            | Some(Chi::PetalCell)
            | Some(Chi::Echo)
            | Some(Chi::PerfMark)
            | Some(Chi::Log)
            | Some(Chi::Drone)
            | Some(Chi::DroneRetrofit) => {
                trace!(client_id, %chi_str, "thrum.recv.todo");
            }
            Some(other) => {
                warn!(client_id, ?other, "thrum.recv.unexpected-direction");
            }
            None => {
                warn!(client_id, %chi_str, "thrum.recv.unknown-chi");
            }
        }
    }
}

fn my_capabilities(cfg: &DaemonConfig) -> PeerCapabilities {
    let nest_name = cfg.hum_cfg.nest.default.clone();
    let total_slots = cfg.hum_cfg.nest.max_active_cells;
    let headroom = ensemble::headroom::CellHeadroom::from_counts(total_slots, total_slots, None);
    PeerCapabilities {
        proto_version: thrum_core::THRUM_VERSION.into(),
        nests: vec![nest_name],
        hosts: Vec::new(),
        can_relay: false,
        free_slots: Some(total_slots as usize),
        headroom,
    }
}

fn pick_overflow_peer(ensemble: &Ensemble, nest_kind: &str) -> Option<Hid> {
    let peers = ensemble.peers();
    let mut fallback: Option<Hid> = None;
    for id in peers {
        let Some(caps) = ensemble.peer_caps(&id) else { continue };
        let has_nest = caps.nests.iter().any(|n| n == nest_kind);
        if !has_nest { continue; }
        match caps.free_slots {
            Some(n) if n > 0 => return Some(id),
            None => return Some(id),
            Some(0) => { /* peer is full, skip but remember as fallback */
                if fallback.is_none() { fallback = Some(id); }
            }
            _ => {}
        }
    }
    fallback
}

fn parse_humd_id(s: &str) -> Option<ensemble::Hid> {
    ensemble::Hid::from_hex(s).ok()
}
