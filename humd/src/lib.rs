use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ensemble::{Ensemble, Hid, HumdKey, PeerCapabilities};
use parking_lot::RwLock;
use serde_json::Value;
use thrum_core::{Chi, Role, WaneTracker};
use thrumd::{Thrum, Tone, ToneSink, serve_with_hook as thrum_serve_with_hook};
use tracing::{error, info, trace, warn};

pub mod advise;
mod comb;
mod drone;
pub mod entered;
pub mod guard;
pub mod hives;
mod identity;
mod peer_transport;
pub mod peers;
pub mod redial;
pub mod revoked;
mod routing_store;
pub mod supervisor;
pub mod thrumd;
pub use entered::EnteredBees;
pub use hives::InstalledHives;
pub use identity::{key_path, load_or_mint_key, read_key};
pub use peers::{PeerConfig, peers_path};

type Observers = Arc<RwLock<HashMap<String, Vec<Hid>>>>;

pub struct DaemonConfig {
    pub thrum_path: PathBuf,
    pub http_path: PathBuf,
    pub comb_path: PathBuf,
    pub routing_path: PathBuf,
    pub hum_cfg: hum_paths::config::HumConfig,
    pub comb_persist_interval: Duration,
    pub routing_persist_interval: Duration,
    pub thrum_override: Option<Thrum>,
    pub ensemble: Option<Arc<Ensemble>>,
    pub revoked_path: PathBuf,
    pub bees_snapshot_path: PathBuf,
    pub advisories_path: PathBuf,
    pub waneman: Option<Arc<WaneTracker>>,
    pub humd_key: Option<Arc<HumdKey>>,
    pub bootstrap_peers: Vec<PeerConfig>,
    pub thehum_cfg: Option<thehum::Config>,
    pub trust_remote_nurses: bool,
    pub trust_remote_attendants: bool,
    pub hives: InstalledHives,
    pub bees: EnteredBees,
}

fn env_says(var: &str) -> bool {
    matches!(
        std::env::var(var).ok().map(|v| v.trim().to_string()).as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

impl DaemonConfig {
    pub fn from_env() -> Self {
        let thrum_path = thrumd::default_socket_path();
        let http_path = hum_paths::http_sock();
        let humd_key = match identity::load_or_mint_key() {
            Ok(k) => Some(Arc::new(k)),
            Err(e) => {
                warn!(err = %e, "identity.load.failed");
                None
            }
        };
        let bootstrap_peers = peers::load();
        let hum_cfg = hum_paths::config::load();
        let trust_remote_nurses = env_says("HUM_TRUST_REMOTE_NURSES") || hum_cfg.humd.trust_remote_nurses;
        let trust_remote_attendants =
            env_says("HUM_TRUST_REMOTE_ATTENDANTS") || hum_cfg.humd.trust_remote_attendants;
        Self {
            thrum_path,
            http_path,
            comb_path: hum_paths::comb(),
            routing_path: hum_paths::routing_json(),
            hum_cfg,
            comb_persist_interval: Duration::from_secs(10),
            routing_persist_interval: Duration::from_secs(30),
            thrum_override: None,
            ensemble: None,
            revoked_path: hum_paths::revoked_bees(),
            bees_snapshot_path: hum_paths::bees_snapshot(),
            advisories_path: hum_paths::advisories(),
            waneman: None,
            humd_key,
            bootstrap_peers,
            thehum_cfg: None,
            trust_remote_nurses,
            trust_remote_attendants,
            hives: InstalledHives::on_disk(),
            bees: EnteredBees::on_disk(),
        }
    }
}

pub async fn run<F>(mut cfg: DaemonConfig, shutdown: F) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send,
{
    hum_paths::init();
    let advisories = Arc::new(advise::Advisories::load(
        cfg.advisories_path.clone(),
        thrum_core::now_ms(),
    ));
    advisories.expect(last_seen(&cfg.bees_snapshot_path));

    info!(
        thrum = %cfg.thrum_path.display(),
        http = %cfg.http_path.display(),
        "humd.sockets"
    );

    if let Ok(addr) = cfg
        .hum_cfg
        .humd
        .metrics_addr
        .parse::<std::net::SocketAddr>()
    {
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

    let comb = comb::Comb::load(&cfg.comb_path);
    comb.clone()
        .spawn_persister(cfg.comb_path.clone(), cfg.comb_persist_interval);

    let waneman = cfg
        .waneman
        .clone()
        .unwrap_or_else(|| Arc::new(WaneTracker::new()));
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
        routing_store::spawn_persister(
            ens.clone(),
            cfg.routing_path.clone(),
            cfg.routing_persist_interval,
        );
    }

    let discovery = ensemble_opt.as_ref().map(|e| e.bee_discover_all());

    let mut peer_reach: Vec<String> = Vec::new();
    let mut supervisor: Option<supervisor::Supervisor> = None;
    if let (Some(ens), Some(key)) = (&ensemble_opt, &cfg.humd_key) {
        let my_caps = my_capabilities();
        let mut iroh_transport: Option<Arc<ensemble::IrohTransport>> = None;
        let mut reached_by_iroh: Vec<String> = Vec::new();

        match peer_transport::iroh::bind(key, cfg.hum_cfg.humd.iroh_port).await {
            Ok((transport, hints)) => {
                let transport = Arc::new(transport);
                reached_by_iroh = peer_transport::iroh::dial_all(
                    &transport,
                    ens,
                    key,
                    &cfg.bootstrap_peers,
                    &my_caps,
                )
                .await;
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

        if let Some(addr) = cfg
            .hum_cfg
            .humd
            .tcp_listen
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            match peer_transport::tcp::spawn_listener(
                addr,
                ens.clone(),
                key.clone(),
                my_caps.clone(),
            )
            .await
            {
                Ok((_bound, hints)) => peer_reach.extend(hints),
                Err(e) => warn!(addr, err = %e, "peer.tcp.bind_failed"),
            }
        }

        peer_transport::tcp::dial_all(
            ens,
            key,
            &peer_transport::tcp::without(&cfg.bootstrap_peers, &reached_by_iroh),
            &my_caps,
        )
        .await;

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
    let manifests: Manifests = Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let remote_bees: RemoteBees = Arc::new(parking_lot::RwLock::new(HashMap::new()));
    if let Some(thehum) = thehum_handle.as_ref() {
        let manifests_for_replay = manifests.clone();
        let hives_for_replay = cfg.hives.clone();
        let bees_for_replay = cfg.bees.clone();
        if let Err(e) = thehum.replay(|event| {
            let body = &event.body;
            let client_id = body
                .get("client_id")
                .and_then(Value::as_str)
                .or_else(|| body.get("from").and_then(Value::as_str))
                .map(str::to_string);
            let Some(client_id) = client_id else { return };
            match event.chi.as_str() {
                "hello" => {
                    let hive = hives::hive_of(body);
                    let entered = body
                        .get("hid")
                        .and_then(Value::as_str)
                        .and_then(|s| ensemble::Hid::from_hex(s).ok())
                        .is_some_and(|hid| {
                            hives::role_of(body).is_some_and(|role| bees_for_replay.admits(&hid, hive, role))
                        });
                    if !hives_for_replay.recognises(hive) || !entered {
                        return;
                    }
                    let name = hive.to_string();
                    let version = body
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or("0.0.0")
                        .to_string();
                    let proto = body
                        .get("protoVersion")
                        .and_then(Value::as_str)
                        .unwrap_or(thrum_core::THRUM_VERSION)
                        .to_string();
                    let mut manifest = ensemble::BeeManifest::new(name, version, proto);
                    manifest.role = hives::role_of(body);
                    manifest.builds = body.get("builds").and_then(Value::as_bool).unwrap_or(false);
                    manifest.models = body
                        .get("models")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    manifest.hid = body
                        .get("hid")
                        .and_then(Value::as_str)
                        .and_then(|s| ensemble::Hid::from_hex(s).ok());
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
    let sid_growers: Arc<parking_lot::RwLock<HashMap<String, Grower>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let tendril_routes: Arc<parking_lot::RwLock<HashMap<String, String>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let build_sites: Arc<parking_lot::RwLock<HashMap<String, BuildSite>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let forager_tools: Arc<parking_lot::RwLock<HashMap<String, Vec<String>>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let alias_resolver = Arc::new(PeersAliasResolver::from_peers(&cfg.bootstrap_peers));
    let tendril_routes_peer: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let incoming_tendrils: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>> =
        Arc::new(parking_lot::RwLock::new(HashMap::new()));
    let humd_sink = Arc::new(HumdSink {
        thrum: thrum.clone(),
        waneman: waneman.clone(),
        ensemble: ensemble_for_sink.clone(),
        observers: observers.clone(),
        manifests: manifests.clone(),
        remote_bees: remote_bees.clone(),
        bees_snapshot_path: cfg.bees_snapshot_path.clone(),
        advisories: advisories.clone(),
        sid_origins: sid_origins.clone(),
        sid_growers,
        tendril_routes,
        build_sites: build_sites.clone(),
        forager_tools: forager_tools.clone(),
        alias_resolver: alias_resolver.clone(),
        tendril_routes_peer: tendril_routes_peer.clone(),
        incoming_tendrils: incoming_tendrils.clone(),
        thehum: thehum_handle.clone(),
        comb: comb.clone(),
        trust_remote_nurses: cfg.trust_remote_nurses,
        trust_remote_attendants: cfg.trust_remote_attendants,
        breathed: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        revoked: revoked::Revocations::load(&cfg.revoked_path),
        hives: cfg.hives.clone(),
        bees: cfg.bees.clone(),
    });
    {
        let humd_sink = humd_sink.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(BREATH_EVERY);
            loop {
                tick.tick().await;
                humd_sink.breathe_foragers();
            }
        });
    }
    let sink: Arc<dyn ToneSink> = humd_sink;
    thrum.set_sink(sink);
    {
        let advisories = advisories.clone();
        tokio::spawn(async move {
            tokio::time::sleep(advise::RETURN_GRACE).await;
            advisories.unreturned(thrum_core::now_ms());
        });
    }
    if let Some(mut seen) = discovery {
        let remote = remote_bees.clone();
        tokio::spawn(async move {
            while let Some((humd_id, manifest)) = seen.recv().await {
                let key = bee_key(&manifest);
                remote
                    .write()
                    .entry(humd_id)
                    .or_default()
                    .insert(key, manifest);
            }
        });
    }
    if bind_thrum {
        let thrum = thrum.clone();
        let path = cfg.thrum_path.clone();
        let humd_version = env!("HUM_VERSION").to_string();
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
                        Ok(r) => trace!(
                            removed = r.removed_files,
                            kept = r.kept_files,
                            "thehum.retention.ok"
                        ),
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
    if let Err(e) = comb.save(&cfg.comb_path) {
        warn!(err = %e, "comb.save.failed");
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
    let stamped = env!("HUM_VERSION");
    let body = tokio::process::Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "https://api.github.com/repos/adiled/hum/releases/latest",
        ])
        .output()
        .await?;
    if !body.status.success() {
        anyhow::bail!("github releases fetch failed: {}", body.status);
    }
    let body = String::from_utf8(body.stdout)?;
    let upstream =
        parse_tag_name(&body).ok_or_else(|| anyhow::anyhow!("no tag_name in response"))?;
    let upstream_trim = upstream.trim_start_matches('v');
    if upstream_trim == local {
        return Ok(false);
    }
    info!(local = %stamped, upstream = %upstream_trim, "autoupdate.newer.found");
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
    manifests: Manifests,
    remote_bees: RemoteBees,
    bees_snapshot_path: std::path::PathBuf,
    advisories: Arc<advise::Advisories>,
    sid_origins: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>>,
    sid_growers: Arc<parking_lot::RwLock<HashMap<String, Grower>>>,
    tendril_routes: Arc<parking_lot::RwLock<HashMap<String, String>>>,
    build_sites: Arc<parking_lot::RwLock<HashMap<String, BuildSite>>>,
    forager_tools: Arc<parking_lot::RwLock<HashMap<String, Vec<String>>>>,
    alias_resolver: Arc<PeersAliasResolver>,
    tendril_routes_peer: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>>,
    incoming_tendrils: Arc<parking_lot::RwLock<HashMap<String, ensemble::Hid>>>,
    thehum: Option<Arc<thehum::TheHum>>,
    comb: Arc<comb::Comb>,
    trust_remote_nurses: bool,
    trust_remote_attendants: bool,
    breathed: Arc<parking_lot::RwLock<HashMap<String, String>>>,
    revoked: revoked::Revocations,
    hives: InstalledHives,
    bees: EnteredBees,
}

#[derive(Debug, Clone, Copy)]
struct Grower {
    humd: ensemble::Hid,
    open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildSite {
    humd: Option<ensemble::Hid>,
    path: String,
}

impl BuildSite {
    fn uri(&self) -> String {
        let host = self.humd.map(|h| h.to_hex()).unwrap_or_default();
        format!("hum://{host}/{}", self.path)
    }
}

enum Unserved {
    NowhereToBuild,
    NoBuilder,
    NoOneServes(String),
    NoScent(String),
}

impl Unserved {
    fn why(&self) -> String {
        match self {
            Unserved::NowhereToBuild => {
                "bash has nowhere to run: this conversation names no cwd, and the bee that prompted it does not build".into()
            }
            Unserved::NoBuilder => {
                "bash has nowhere to run: no builder is nestled on the humd this conversation builds on".into()
            }
            Unserved::NoOneServes(name) => format!("no bee serves {name} to this conversation"),
            Unserved::NoScent(model) => format!("no attendant here or on a trusted peer distils scents by {model}"),
        }
    }
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

type Manifests = Arc<parking_lot::RwLock<HashMap<String, ensemble::BeeManifest>>>;
type RemoteBees =
    Arc<parking_lot::RwLock<HashMap<ensemble::Hid, HashMap<String, ensemble::BeeManifest>>>>;

fn bee_key(manifest: &ensemble::BeeManifest) -> String {
    manifest
        .hid
        .map(|h| hex::encode(h.as_bytes()))
        .unwrap_or_else(|| manifest.hive.clone())
}

fn last_seen(path: &std::path::Path) -> HashMap<String, String> {
    let Some(Value::Object(snapshot)) = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return HashMap::new();
    };
    snapshot
        .values()
        .filter_map(|m| {
            let hid = m.get("hid").and_then(Value::as_str)?;
            let hive = m.get("hive").and_then(Value::as_str)?;
            Some((hid.to_string(), hive.to_string()))
        })
        .collect()
}

impl HumdSink {
    fn pick_remote_nurse(&self, model: Option<&str>) -> Option<ensemble::Hid> {
        let ens = self.ensemble.as_ref()?;
        if !self.trust_remote_nurses {
            warn!(
                model,
                "prompt.remote-routing.disabled — a peer can advertise any model and be handed the prompt; set HUM_TRUST_REMOTE_NURSES=1 to accept that risk"
            );
            return None;
        }
        let live: std::collections::BTreeSet<String> =
            ens.peers().iter().map(|h| h.to_hex()).collect();
        let table = self.remote_bees.read();
        let mut found: Vec<(u32, String)> = table
            .iter()
            .filter(|(humd, _)| live.contains(&humd.to_hex()))
            .filter_map(|(humd, bees)| {
                bees.values()
                    .filter(|m| {
                        m.plays(Role::Nurse)
                            && model.is_none_or(|model| m.models.iter().any(|x| x == model))
                            && self.hives.recognises(&m.hive)
                            && !m.is_full()
                    })
                    .map(|m| m.free_slots())
                    .max()
                    .map(|free| (free, humd.to_hex()))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Hid::from_hex(&found.first()?.1).ok()
    }

    fn trusted_remote_attendants(&self) -> Vec<(ensemble::Hid, ensemble::BeeManifest)> {
        let Some(ens) = self.ensemble.as_ref().filter(|_| self.trust_remote_attendants) else {
            return Vec::new();
        };
        let live: std::collections::BTreeSet<String> = ens.peers().iter().map(|h| h.to_hex()).collect();
        let table = self.remote_bees.read();
        let mut found: Vec<(String, String, ensemble::Hid, ensemble::BeeManifest)> = table
            .iter()
            .filter(|(humd, _)| live.contains(&humd.to_hex()))
            .flat_map(|(humd, bees)| {
                bees.iter()
                    .filter(|(_, m)| m.plays(Role::Attendant) && self.hives.recognises(&m.hive))
                    .map(|(key, m)| (humd.to_hex(), key.clone(), *humd, m.clone()))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        found.into_iter().map(|(_, _, humd, m)| (humd, m)).collect()
    }

    fn remote_attendant(&self, tool: &str) -> Option<ensemble::Hid> {
        self.trusted_remote_attendants()
            .into_iter()
            .find(|(_, m)| m.tools.iter().any(|t| t.name == tool))
            .map(|(humd, _)| humd)
    }

    fn scent_here(&self, model: &str, from: &str) -> Option<String> {
        let m = self.manifests.read();
        let mut distilling: Vec<&String> = m
            .iter()
            .filter(|(cid, man)| {
                cid.as_str() != from
                    && man.plays(Role::Attendant)
                    && man.models.iter().any(|x| x == model)
                    && self.thrum.is_connected(cid)
            })
            .map(|(cid, _)| cid)
            .collect();
        distilling.sort();
        distilling.first().map(|cid| (*cid).clone())
    }

    fn remote_scent(&self, model: &str) -> Option<ensemble::Hid> {
        self.trusted_remote_attendants()
            .into_iter()
            .find(|(_, m)| m.models.iter().any(|x| x == model))
            .map(|(humd, _)| humd)
    }

    fn reachable(&self) -> Value {
        let mut models = std::collections::BTreeSet::new();
        let mut scents = std::collections::BTreeSet::new();
        let mut attendants: Vec<ensemble::BeeManifest> = Vec::new();
        {
            let m = self.manifests.read();
            let mut here: Vec<_> = m.iter().filter(|(cid, _)| self.thrum.is_connected(cid)).collect();
            here.sort_by(|a, b| a.0.cmp(b.0));
            for (_, man) in here {
                if man.plays(Role::Nurse) {
                    models.extend(man.models.iter().map(|id| thrum_core::Model { id: id.clone(), hive: man.hive.clone() }));
                } else if man.plays(Role::Attendant) {
                    attendants.push(man.clone());
                }
            }
        }
        if self.trust_remote_nurses
            && let Some(ens) = &self.ensemble
        {
            let live: std::collections::BTreeSet<String> = ens.peers().iter().map(|h| h.to_hex()).collect();
            for (humd, bees) in self.remote_bees.read().iter() {
                if !live.contains(&humd.to_hex()) {
                    continue;
                }
                for man in bees.values().filter(|m| m.plays(Role::Nurse) && self.hives.recognises(&m.hive)) {
                    models.extend(man.models.iter().map(|id| thrum_core::Model { id: id.clone(), hive: man.hive.clone() }));
                }
            }
        }
        attendants.extend(self.trusted_remote_attendants().into_iter().map(|(_, m)| m));
        let mut named = std::collections::BTreeSet::new();
        let mut tools = Vec::new();
        for man in &attendants {
            scents.extend(man.models.iter().map(|id| thrum_core::Model { id: id.clone(), hive: man.hive.clone() }));
            for t in &man.tools {
                if !reserved(&t.name) && named.insert(t.name.clone()) {
                    tools.push(serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    }));
                }
            }
        }
        serde_json::json!({ "models": models, "scents": scents, "tools": tools })
    }

    fn forager_breath(listing: &Value) -> Tone {
        let mut breath = thrumd::breath_tone(serde_json::json!({}));
        if let (Some(obj), Some(said)) = (breath.as_object_mut(), listing.as_object()) {
            for (k, v) in said {
                obj.insert(k.clone(), v.clone());
            }
        }
        breath
    }

    fn breathe_foragers(&self) {
        let foragers: Vec<String> = self
            .manifests
            .read()
            .iter()
            .filter(|(cid, m)| m.plays(Role::Forager) && self.thrum.is_connected(cid))
            .map(|(cid, _)| cid.clone())
            .collect();
        if foragers.is_empty() {
            return;
        }
        let listing = self.reachable();
        let said = listing.to_string();
        let mut breathed = self.breathed.write();
        for cid in foragers {
            if breathed.get(&cid) != Some(&said) {
                trace!(client_id = %cid, "breath.forager.reach-changed");
                self.thrum.thrum_to(&cid, Self::forager_breath(&listing));
                breathed.insert(cid, said.clone());
            }
        }
    }

    fn attendant_serving(&self, tool: &str, from: &str) -> Option<String> {
        let m = self.manifests.read();
        let mut serving: Vec<&String> = m
            .iter()
            .filter(|(cid, man)| {
                cid.as_str() != from
                    && man.plays(Role::Attendant)
                    && man.tools.iter().any(|t| t.name == tool)
            })
            .map(|(cid, _)| cid)
            .collect();
        serving.sort();
        serving.first().map(|cid| (*cid).clone())
    }

    fn playing(&self, roles: &[Role]) -> Vec<String> {
        self.manifests
            .read()
            .iter()
            .filter(|(_, m)| m.role.is_some_and(|r| roles.contains(&r)))
            .map(|(cid, _)| cid.clone())
            .collect()
    }

    fn builder_here(&self) -> Option<String> {
        let m = self.manifests.read();
        let mut builders: Vec<&String> = m
            .iter()
            .filter(|(cid, man)| man.plays(Role::Builder) && self.thrum.is_connected(cid))
            .map(|(cid, _)| cid)
            .collect();
        builders.sort();
        builders.first().map(|cid| (*cid).clone())
    }

    fn bloom_tools(&self, forager: &str, builds: bool, offered: Option<&Value>) -> Vec<Value> {
        let mut tools = Vec::new();
        let mut named = std::collections::BTreeSet::new();
        if builds {
            named.insert(thrum_core::BASH.to_string());
            tools.push(thrum_core::bash_tool());
        }
        {
            let m = self.manifests.read();
            let mut attendants: Vec<_> = m
                .iter()
                .filter(|(cid, man)| cid.as_str() != forager && man.plays(Role::Attendant))
                .collect();
            attendants.sort_by(|a, b| a.0.cmp(b.0));
            for (_, man) in attendants {
                for t in &man.tools {
                    if !reserved(&t.name) && named.insert(t.name.clone()) {
                        tools.push(serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.input_schema,
                        }));
                    }
                }
            }
        }
        for t in offered.and_then(Value::as_array).into_iter().flatten() {
            if let Some(name) = t.get("name").and_then(Value::as_str)
                && named.insert(name.to_string())
            {
                tools.push(t.clone());
            }
        }
        for (_, man) in self.trusted_remote_attendants() {
            for t in &man.tools {
                if !reserved(&t.name) && named.insert(t.name.clone()) {
                    tools.push(serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    }));
                }
            }
        }
        tools
    }

    fn build_site(&self, client_id: &str, sid: &str, tone: &Value) -> Option<BuildSite> {
        let here = self.ensemble.as_ref().map(|e| e.me());
        let local = client_id != "ensemble";
        if let Some(cwd) = tone.get("cwd").and_then(Value::as_str) {
            match ensemble::HumUri::parse(cwd) {
                Ok(uri) => {
                    let humd = match &uri.host {
                        ensemble::HostRef::Hid(h) => Some(*h),
                        ensemble::HostRef::Alias(name) => {
                            use ensemble::AliasResolver;
                            let Some(h) = self.alias_resolver.resolve(name) else {
                                warn!(sid, uri = cwd, "prompt.cwd.alias.unknown");
                                return None;
                            };
                            Some(h)
                        }
                        ensemble::HostRef::Here if local => here,
                        ensemble::HostRef::Here => tone
                            .get("from")
                            .and_then(Value::as_str)
                            .and_then(parse_humd_id)
                            .or(here),
                    };
                    return Some(BuildSite { humd, path: uri.path });
                }
                Err(e) if local => self.advise_cwd_not_hum(client_id, cwd, &e.to_string()),
                Err(e) => warn!(sid, uri = cwd, err = %e, "prompt.cwd.not-hum"),
            }
        }
        if !local {
            return None;
        }
        if let Some(site) = self.build_sites.read().get(sid).cloned() {
            return Some(site);
        }
        let builds = self.manifests.read().get(client_id).is_some_and(|m| m.builds);
        builds.then(|| BuildSite { humd: here, path: String::new() })
    }

    fn advise_cwd_not_hum(&self, client_id: &str, cwd: &str, why: &str) {
        let bee = self
            .manifests
            .read()
            .get(client_id)
            .and_then(|m| Some((m.hid?.to_hex(), m.hive.clone())));
        let Some((bee, hive)) = bee else { return };
        let mut fields = serde_json::Map::new();
        fields.insert("cwd".into(), Value::String(cwd.to_string()));
        let message = format!(
            "cwd {cwd:?} is not a hum:// address ({why}); it was dropped. Say hum://<humdid>/<path>, or hum:///<path> for this humd"
        );
        let entries = self.advisories.note(
            &bee,
            &hive,
            thrum_core::AdvisoryCode::CwdNotHum,
            message,
            fields,
            thrum_core::now_ms(),
        );
        self.advise(client_id, entries.into_iter().collect());
    }

    fn unserved(&self, nurse: &str, sid: &str, call_id: &str, why: Unserved) {
        let why = why.why();
        warn!(sid, call_id, %why, "tendril-reach.unserved");
        self.thrum.thrum_to(nurse, failed_grasp(sid, call_id, &why));
    }

    async fn reach_builder(&self, nurse: &str, sid: &str, call_id: &str, site: BuildSite, tone: Value) {
        let mut reach = tone;
        if let Some(obj) = reach.as_object_mut() {
            if site.path.is_empty() {
                obj.remove("cwd");
            } else {
                obj.insert("cwd".into(), Value::String(format!("/{}", site.path)));
            }
        }
        if !call_id.is_empty() {
            self.tendril_routes.write().insert(call_id.to_string(), nurse.to_string());
        }
        let here = self.ensemble.as_ref().map(|e| e.me());
        if let (Some(humd), Some(me)) = (site.humd, here)
            && humd != me
        {
            self.reach_peer(nurse, sid, call_id, humd, reach, Unserved::NoBuilder).await;
            return;
        }
        match self.builder_here() {
            Some(builder) => {
                trace!(nurse, %builder, call_id, "tendril-reach.bash.route.local");
                self.thrum.thrum_to(&builder, reach);
            }
            None => {
                self.tendril_routes.write().remove(call_id);
                self.unserved(nurse, sid, call_id, Unserved::NoBuilder);
            }
        }
    }

    async fn follow_to_grower(&self, client_id: &str, sid: &str, tone: &Value, last: bool) {
        if client_id == "ensemble" {
            return;
        }
        let grower = if last {
            self.sid_growers.write().remove(sid)
        } else {
            self.sid_growers.read().get(sid).copied()
        }
        .map(|g| g.humd);
        let (Some(peer), Some(ens)) = (grower, &self.ensemble) else { return };
        let mut copy = tone.clone();
        if let Some(obj) = copy.as_object_mut() {
            obj.insert("to".into(), Value::String(peer.to_hex()));
            obj.insert("from".into(), Value::String(ens.me().to_hex()));
        }
        trace!(sid, to = %peer.short(), "sigil.follow.grower");
        if let Err(e) = ens.route(copy).await {
            warn!(sid, err = %e, "sigil.follow.grower.failed");
        }
    }

    fn forget_sid(&self, sid: &str) {
        self.sid_growers.write().remove(sid);
        self.build_sites.write().remove(sid);
        self.forager_tools.write().remove(sid);
        self.waneman.forget(sid);
    }

    fn break_off_grown_on(&self, gone: ensemble::Hid) {
        let lost: Vec<String> = self
            .sid_growers
            .write()
            .iter_mut()
            .filter(|(_, g)| g.humd == gone && g.open)
            .map(|(sid, g)| {
                g.open = false;
                sid.clone()
            })
            .collect();
        for sid in lost {
            warn!(sid, peer = %gone.short(), "bloom.nurse-gone — the peer growing it left the ensemble");
            let err = serde_json::json!({
                "chi": "error",
                "sid": sid,
                "code": "nurse-gone",
                "message": "the peer growing it left the ensemble; it may be grown again",
            });
            self.thrum.thrum_broadcast(&sid, err);
        }
    }

    async fn reach_peer(&self, nurse: &str, sid: &str, call_id: &str, humd: ensemble::Hid, mut reach: Value, failed: Unserved) {
        let Some(ens) = &self.ensemble else {
            self.unserved(nurse, sid, call_id, failed);
            return;
        };
        if !call_id.is_empty() {
            self.tendril_routes.write().insert(call_id.to_string(), nurse.to_string());
            self.tendril_routes_peer.write().insert(call_id.to_string(), humd);
        }
        if let Some(obj) = reach.as_object_mut() {
            obj.insert("to".into(), Value::String(humd.to_hex()));
            obj.insert("from".into(), Value::String(ens.me().to_hex()));
        }
        let name = reach.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        trace!(nurse, to = %humd.short(), call_id, %name, "tendril-reach.route.peer");
        if let Err(e) = ens.route(reach).await {
            warn!(err = %e, %name, "tendril-reach.peer.route.failed");
            self.tendril_routes.write().remove(call_id);
            self.tendril_routes_peer.write().remove(call_id);
            self.unserved(nurse, sid, call_id, failed);
        }
    }

    async fn claim_headroom(&self, client_id: &str, headroom: ensemble::headroom::CellHeadroom) {
        let changed = {
            let mut m = self.manifests.write();
            let Some(man) = m.get_mut(client_id) else {
                return;
            };
            let was_full = man.is_full();
            man.headroom = Some(headroom);
            (was_full != man.is_full()).then(|| man.clone())
        };
        if let Some(manifest) = changed {
            self.snapshot_bees();
            if let Some(ens) = &self.ensemble {
                ens.bee_advertise(manifest).await;
            }
        }
    }

    fn advise(&self, client_id: &str, entries: Vec<advise::Entry>) {
        for entry in entries {
            self.thrum.thrum_to(client_id, advise::tone(&entry));
        }
    }

    fn undeclared_entry(&self, bee: &str, hive: &str, tone: &Value) -> Option<advise::Entry> {
        let (chi, keys) = thrum_core::undeclared(tone)?;
        let chi = serde_json::to_value(chi)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let message = format!(
            "{chi} carries {}, which the thrum does not declare for it",
            keys.join(", ")
        );
        let mut fields = serde_json::Map::new();
        fields.insert("chi".into(), Value::String(chi));
        fields.insert("keys".into(), serde_json::json!(keys));
        self.advisories.note(
            bee,
            hive,
            thrum_core::AdvisoryCode::UndeclaredKey,
            message,
            fields,
            thrum_core::now_ms(),
        )
    }

    fn advise_on_wire(&self, client_id: &str, tone: &Value) {
        let bee = self
            .manifests
            .read()
            .get(client_id)
            .and_then(|m| Some((m.hid?.to_hex(), m.hive.clone())));
        let Some((bee, hive)) = bee else { return };
        if let Some(entry) = self.undeclared_entry(&bee, &hive, tone) {
            self.advise(client_id, vec![entry]);
        }
    }

    fn advise_on_hello(&self, client_id: &str, bee: &str, hive: &str, hello: &Value) {
        let now = thrum_core::now_ms();
        let mut entries = self.advisories.hello(bee, hive, now);
        if let Some(declared) = hello.get("protoVersion").and_then(Value::as_str)
            && thrumd::proto_verdict(declared, thrum_core::THRUM_VERSION)
                == thrumd::ProtoVerdict::SameMajor
        {
            let mut fields = serde_json::Map::new();
            fields.insert("declared".into(), Value::String(declared.to_string()));
            fields.insert("current".into(), Value::String(thrum_core::THRUM_VERSION.to_string()));
            let message = format!(
                "speaks thrum {declared} to a humd on {}; rebuild the hive against this hum",
                thrum_core::THRUM_VERSION
            );
            entries.extend(self.advisories.note(
                bee,
                hive,
                thrum_core::AdvisoryCode::ProtoDrift,
                message,
                fields,
                now,
            ));
        }
        entries.extend(self.undeclared_entry(bee, hive, hello));
        self.advise(client_id, entries);
    }

    fn snapshot_bees(&self) {
        let json = {
            let m = self.manifests.read();
            serde_json::to_vec_pretty(&*m).unwrap_or_default()
        };
        let path = &self.bees_snapshot_path;
        if hum_paths::write_atomic(path, &json).is_err() {
            trace!(path = %path.display(), "bees.snapshot.write.failed");
        }
    }
}

#[async_trait::async_trait]
impl ToneSink for HumdSink {
    async fn forget(&self, client_id: &str, sids: Vec<String>) {
        if client_id == "ensemble" {
            return;
        }
        for sid in sids {
            if !self.thrum.holds(&sid) && !self.sid_origins.read().contains_key(&sid) {
                self.forget_sid(&sid);
            }
        }
        let had_manifest = self.manifests.write().remove(client_id).is_some();
        self.breathed.write().remove(client_id);
        self.tendril_routes
            .write()
            .retain(|_, originator| originator != client_id);
        if had_manifest {
            trace!(client_id, "manifest.evict.disconnect");
            self.snapshot_bees();
            self.breathe_foragers();
        }
    }

    async fn hear(&self, client_id: &str, tone: Tone) {
        let chi_str = tone.get("chi").and_then(Value::as_str).unwrap_or("?");
        let chi: Option<Chi> = tone
            .get("chi")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());
        if !matches!(chi, Some(Chi::Hello)) {
            self.advise_on_wire(client_id, &tone);
        }

        let bee = if comb::WORK_CHIS.contains(&chi_str) {
            self.manifests
                .read()
                .get(client_id)
                .map(|m| m.hive.clone())
        } else {
            None
        };
        self.comb
            .record(&chi_str, bee.as_deref(), tone.get("nectar"));

        if let Some(thehum) = self.thehum.as_ref()
            && client_id != "ensemble"
        {
            let sid = tone.get("sid").and_then(Value::as_str).and_then(|s| {
                hum_identity::HumId::parse(s)
                    .ok()
                    .or_else(|| Some(hum_identity::HumId::from_foreign(s)))
            });
            let rid = tone
                .get("rid")
                .and_then(Value::as_str)
                .map(|s| {
                    hum_identity::HumId::parse(s)
                        .unwrap_or_else(|_| hum_identity::HumId::from_foreign(s))
                })
                .unwrap_or_else(hum_identity::HumId::mint);
            let body = serde_json::to_value(&tone).unwrap_or_default();
            if let Err(e) = thehum.append(chi_str, sid, rid, body).await {
                warn!(client_id, %chi_str, err = %e, "thehum.append.failed");
            }
        }

        if client_id == "ensemble"
            && matches!(chi, Some(Chi::TendrilReach))
            && let Some(name) = tone.get("name").and_then(Value::as_str).map(str::to_string)
        {
            let call_id = tone.get("callId").and_then(Value::as_str).unwrap_or("").to_string();
            let from_peer = tone.get("from").and_then(Value::as_str).and_then(parse_humd_id);
            if from_peer.is_none() {
                warn!(%name, "tendril-reach.peer.no-origin — a reach from the ensemble names no humd it came from, so nothing could answer it");
                return;
            }
            let reached = if name == thrum_core::BASH {
                self.builder_here()
            } else if name == thrum_core::SCENT {
                self.scent_here(&scent_model(&tone), client_id)
            } else {
                self.attendant_serving(&name, client_id)
            };
            let Some(reached) = reached else {
                warn!(%name, "tendril-reach.peer.unserved");
                if let (Some(peer), Some(ens)) = (from_peer, &self.ensemble) {
                    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("");
                    let why = if name == thrum_core::BASH {
                        Unserved::NoBuilder.why()
                    } else if name == thrum_core::SCENT {
                        Unserved::NoScent(scent_model(&tone)).why()
                    } else {
                        format!("no bee serves {name} on the humd it was sent to")
                    };
                    let mut result = failed_grasp(sid, &call_id, &why);
                    if let Some(obj) = result.as_object_mut() {
                        obj.insert("to".into(), Value::String(peer.to_hex()));
                        obj.insert("from".into(), Value::String(ens.me().to_hex()));
                    }
                    if let Err(e) = ens.route(result).await {
                        warn!(err = %e, "tendril-result.unserved.route.failed");
                    }
                }
                return;
            };
            if let Some(from_peer) = from_peer
                && !call_id.is_empty()
            {
                self.incoming_tendrils.write().insert(call_id, from_peer);
            }
            trace!(from = "ensemble", to = %reached, %name, "tendril-reach.peer.route.local");
            self.thrum.thrum_to(&reached, tone);
            return;
        }

        if matches!(chi, Some(Chi::Pulse))
            && let Some(headroom) = hives::headroom_of(&tone)
        {
            self.claim_headroom(client_id, headroom).await;
        }

        if !matches!(chi, Some(Chi::Hello)) {
            let is_nurse = self
                .manifests
                .read()
                .get(client_id)
                .is_some_and(|man| man.plays(Role::Nurse));
            if matches!(chi, Some(Chi::TendrilReach)) && client_id != "ensemble" {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if !sid.is_empty() && !is_nurse {
                    self.thrum.hold(client_id, &sid);
                }
                let name = tone.get("name").and_then(Value::as_str).map(str::to_string);
                let call_id = tone.get("callId").and_then(Value::as_str).unwrap_or("").to_string();

                if is_nurse && name.as_deref() == Some(thrum_core::BASH) {
                    let site = self.build_sites.read().get(&sid).cloned();
                    let offered = self
                        .forager_tools
                        .read()
                        .get(&sid)
                        .is_some_and(|names| names.iter().any(|n| n == thrum_core::BASH));
                    match site {
                        Some(site) => {
                            self.reach_builder(client_id, &sid, &call_id, site, tone).await;
                            return;
                        }
                        None if !offered => {
                            self.unserved(client_id, &sid, &call_id, Unserved::NowhereToBuild);
                            return;
                        }
                        None => {}
                    }
                }

                if name.as_deref() == Some(thrum_core::SCENT) {
                    let model = scent_model(&tone);
                    if let Some(reached) = self.scent_here(&model, client_id) {
                        if !call_id.is_empty() {
                            self.tendril_routes.write().insert(call_id.clone(), client_id.to_string());
                        }
                        trace!(from = client_id, to = %reached, %model, %call_id, "tendril-reach.scent.local");
                        let mut local = tone;
                        if let Some(obj) = local.as_object_mut() {
                            obj.remove("from");
                            obj.remove("to");
                        }
                        self.thrum.thrum_to(&reached, local);
                    } else if let Some(humd) = self.remote_scent(&model) {
                        self.reach_peer(client_id, &sid, &call_id, humd, tone, Unserved::NoScent(model)).await;
                    } else {
                        self.unserved(client_id, &sid, &call_id, Unserved::NoScent(model));
                    }
                    return;
                }

                if let Some(name) = name {
                    if let Some(reached) = self.attendant_serving(&name, client_id) {
                        if !call_id.is_empty() {
                            self.tendril_routes.write().insert(call_id.clone(), client_id.to_string());
                        }
                        trace!(from = client_id, to = %reached, %name, %call_id, "tendril-reach.route.local");
                        let mut local = tone;
                        if let Some(obj) = local.as_object_mut() {
                            obj.remove("from");
                            obj.remove("to");
                        }
                        self.thrum.thrum_to(&reached, local);
                        return;
                    }
                    let offered = self
                        .forager_tools
                        .read()
                        .get(&sid)
                        .is_some_and(|names| names.iter().any(|n| *n == name));
                    if !offered && let Some(humd) = self.remote_attendant(&name) {
                        self.reach_peer(client_id, &sid, &call_id, humd, tone, Unserved::NoOneServes(name)).await;
                        return;
                    }
                    if is_nurse && !offered && self.forager_tools.read().contains_key(&sid) {
                        self.unserved(client_id, &sid, &call_id, Unserved::NoOneServes(name));
                        return;
                    }
                    if is_nurse && offered && !call_id.is_empty() {
                        trace!(from = client_id, %name, %call_id, "tendril-reach.route.forager");
                        self.tendril_routes.write().insert(call_id.clone(), client_id.to_string());
                    }
                }
            }
            if is_nurse {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str).map(str::to_string)
                    && matches!(
                        chi,
                        Some(Chi::Unfurl)
                            | Some(Chi::Wilt)
                            | Some(Chi::Error)
                            | Some(Chi::TendrilReach)
                            | Some(Chi::TendrilInfo)
                            | Some(Chi::Pulse)
                            | Some(Chi::Breath)
                    )
                {
                    if let Some(ens) = &self.ensemble {
                        let obs = self.observers.read().get(&sid).cloned().unwrap_or_default();
                        for peer in obs {
                            let mut copy = tone.clone();
                            if let Some(obj) = copy.as_object_mut() {
                                obj.insert("to".into(), Value::String(peer.to_hex()));
                                obj.insert("from".into(), Value::String(ens.me().to_hex()));
                            }
                            if let Err(e) = ens.route(copy).await {
                                warn!(err = %e, "nurse.reply.observer.failed");
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
                            warn!(err = %e, "nurse.reply.ensemble.failed");
                        }
                    }
                    self.thrum
                        .thrum_broadcast(&sid, tone.clone());
                    return;
                }
            }
        }

        if let Some(sid) = tone.get("sid").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            match chi {
                Some(Chi::Prompt) | Some(Chi::Attach) if client_id != "ensemble" => {
                    self.thrum.hold(client_id, sid);
                    trace!(client_id, sid, %chi_str, "sid.held");
                }
                Some(Chi::Cleanup) => self.thrum.release(sid),
                _ => {}
            }
        }

        if let Some(ensemble) = &self.ensemble
            && let Some(to) = tone.get("to").and_then(Value::as_str)
            && !to.is_empty()
            && to != ensemble.me().to_hex()
        {
            trace!(client_id, %chi_str, to, "ensemble.route");
            if let Err(e) = ensemble.route(tone).await {
                warn!(client_id, err = %e, "ensemble.route.failed");
            }
            return;
        }

        if client_id == "ensemble" {
            let answers_a_reach = tone.get("callId").is_some();
            let is_reply = match chi {
                Some(Chi::Error) => !answers_a_reach,
                _ => matches!(
                    chi,
                    Some(Chi::Unfurl)
                        | Some(Chi::Wilt)
                        | Some(Chi::Pulse)
                        | Some(Chi::TendrilReach)
                        | Some(Chi::TendrilMeta)
                ),
            };
            if is_reply {
                let sid_opt = tone.get("sid").and_then(Value::as_str).map(str::to_string);
                if let Some(sid) = sid_opt {
                    let closes = matches!(chi, Some(Chi::Wilt) | Some(Chi::Error));
                    let from = tone.get("from").and_then(Value::as_str).and_then(parse_humd_id);
                    if closes
                        && let Some(g) = self.sid_growers.write().get_mut(&sid)
                        && Some(g.humd) == from
                    {
                        g.open = false;
                    }
                    trace!(client_id, %chi_str, %sid, "thrum.recv.peer-reply.forward");
                    self.thrum.thrum_broadcast(&sid, tone);
                    return;
                }
            }
        }

        match chi {
            Some(Chi::Hello) => {
                trace!(client_id, %chi_str, "thrum.recv.hello");

                let rid = tone
                    .get("rid")
                    .and_then(Value::as_str)
                    .unwrap_or("hello-1")
                    .to_string();

                match tone.get("protoVersion").and_then(Value::as_str) {
                    None => {
                        error!(
                            client_id,
                            "thrum.hello.proto-missing — hello declares no protoVersion. \
                             Not registered, not announced to the ensemble."
                        );
                        self.thrum.thrum_to(
                            client_id,
                            thrumd::echo_tone(&rid, false, Some("protoVersion is required")),
                        );
                        self.thrum.thrum_close(client_id);
                        return;
                    }
                    Some(declared) => {
                        match thrumd::proto_verdict(declared, thrum_core::THRUM_VERSION) {
                            thrumd::ProtoVerdict::Same => {}
                            thrumd::ProtoVerdict::SameMajor => {
                                warn!(
                                    client_id,
                                    declared,
                                    current = thrum_core::THRUM_VERSION,
                                    "thrum.hello.proto-drift — admitted within major"
                                );
                            }
                            thrumd::ProtoVerdict::Incompatible => {
                                error!(
                                    client_id,
                                    declared,
                                    current = thrum_core::THRUM_VERSION,
                                    "thrum.hello.proto-incompatible — rejected, not announced"
                                );
                                self.thrum.thrum_to(
                                    client_id,
                                    thrumd::echo_tone(
                                        &rid,
                                        false,
                                        Some("protoVersion incompatible"),
                                    ),
                                );
                                self.thrum.thrum_close(client_id);
                                return;
                            }
                        }
                    }
                }

                let hive = hives::hive_of(&tone).to_string();
                if client_id != "ensemble" && !self.hives.recognises(&hive) {
                    let why = if hive.is_empty() {
                        "hello names no hive"
                    } else {
                        "hive not installed on this humd"
                    };
                    error!(
                        client_id,
                        hive = %hive,
                        reason = why,
                        "thrum.hello.hive-unrecognised — a bee is understood only through a hive \
                         installed here (`hum hive <ref> install`). Not registered, not announced."
                    );
                    self.thrum
                        .thrum_to(client_id, thrumd::echo_tone(&rid, false, Some(why)));
                    self.thrum.thrum_close(client_id);
                    return;
                }

                let role = hives::role_of(&tone);
                if client_id != "ensemble" && role.is_none() {
                    let why = "hello names no role: forager | nurse | builder | attendant";
                    error!(client_id, hive = %hive, reason = why, "thrum.hello.role-missing — Not registered, not announced.");
                    self.thrum
                        .thrum_to(client_id, thrumd::echo_tone(&rid, false, Some(why)));
                    self.thrum.thrum_close(client_id);
                    return;
                }

                let signed_hid = if client_id == "ensemble" {
                    None
                } else {
                    let refusal = match ensemble::parse_socket_identity(&tone) {
                        ensemble::SocketIdentity::Verified(hid)
                            if self.revoked.is_revoked(&hid.to_hex()) =>
                        {
                            Err("bee identity revoked")
                        }
                        ensemble::SocketIdentity::Verified(hid)
                            if role.is_some_and(|role| self.bees.admits(&hid, &hive, role)) =>
                        {
                            Ok(hid)
                        }
                        ensemble::SocketIdentity::Verified(_) => {
                            Err("bee has not entered under this hive for this role")
                        }
                        ensemble::SocketIdentity::Unsigned(_) => {
                            Err("hello must be signed by an entered bee")
                        }
                        ensemble::SocketIdentity::Invalid(why) => Err(why),
                    };
                    match refusal {
                        Ok(hid) => Some(hid),
                        Err(why) => {
                            error!(
                                client_id,
                                hive = %hive,
                                reason = why,
                                "thrum.hello.not-entered — a bee nestles only by signing as a key \
                                 its hive entered for its role (`hum bee enter --hive <hive> --role <role>`). Not registered, not announced."
                            );
                            self.thrum
                                .thrum_to(client_id, thrumd::echo_tone(&rid, false, Some(why)));
                            self.thrum.thrum_close(client_id);
                            return;
                        }
                    }
                };

                let breath = if role == Some(Role::Forager) && client_id != "ensemble" {
                    let listing = self.reachable();
                    self.breathed.write().insert(client_id.to_string(), listing.to_string());
                    Self::forager_breath(&listing)
                } else {
                    thrumd::breath_tone(serde_json::json!({}))
                };
                self.thrum.admit(client_id);
                self.thrum.thrum_to(client_id, breath);

                let models: Vec<String> = tone
                    .get("models")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let name = (!hive.is_empty()).then_some(hive);
                if let Some(name) = name {
                    let proto = tone
                        .get("protoVersion")
                        .and_then(Value::as_str)
                        .unwrap_or(thrum_core::THRUM_VERSION)
                        .to_string();
                    let version = tone
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or("0.0.0")
                        .to_string();
                    let propensity = tone
                        .get("propensity")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let chis: Vec<String> = tone
                        .get("chis")
                        .or_else(|| tone.get("chi"))
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    self.thrum.claim_chis(client_id, chis.clone());
                    let source = tone
                        .get("source")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let bind: Option<ensemble::BindAddr> = tone
                        .get("bind")
                        .and_then(|v| serde_json::from_value(v.clone()).ok());
                    let mut manifest = ensemble::BeeManifest::new(name, version, proto);
                    manifest.propensity = propensity;
                    manifest.chis = chis;
                    manifest.source = source;
                    manifest.bind = bind;
                    manifest.role = role;
                    manifest.builds = role == Some(Role::Forager)
                        && tone.get("builds").and_then(Value::as_bool).unwrap_or(false);
                    manifest.models = models.clone();
                    manifest.headroom = hives::headroom_of(&tone);
                    manifest.hid = signed_hid;
                    manifest.hid_verified = signed_hid.is_some();
                    if let Some(arr) = tone.get("tools").and_then(Value::as_array) {
                        manifest.tools = arr
                            .iter()
                            .filter_map(|v| {
                                let name = v.get("name").and_then(Value::as_str)?.to_string();
                                let description = v
                                    .get("description")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let input_schema =
                                    v.get("inputSchema").cloned().unwrap_or(Value::Null);
                                Some(ensemble::ToolEntry {
                                    name,
                                    description,
                                    input_schema,
                                })
                            })
                            .collect();
                    }
                    if !manifest.tools.is_empty() {
                        if role == Some(Role::Attendant) {
                            info!(client_id, count = manifest.tools.len(), "attendant.tools.registered");
                        } else {
                            warn!(
                                client_id,
                                role = role.map(Role::as_str),
                                "bee.tools.ignored — only an attendant serves the tools in its hello"
                            );
                        }
                    }

                    if client_id != "ensemble" {
                        let mut m = self.manifests.write();
                        let mut stale: Vec<String> = Vec::new();
                        if let Some(new_hid) = manifest.hid {
                            stale = m
                                .iter()
                                .filter_map(|(cid, mf)| match mf.hid {
                                    Some(h) if h == new_hid && cid != client_id => {
                                        Some(cid.clone())
                                    }
                                    _ => None,
                                })
                                .collect();
                            for cid in &stale {
                                info!(stale_client_id = %cid, hid = %new_hid.short(),
                                    "bee.replaced — the bee's newer connection replaces this one, which is closed");
                                m.remove(cid);
                            }
                        }
                        m.insert(client_id.to_string(), manifest.clone());
                        drop(m);
                        for cid in &stale {
                            self.thrum.thrum_close(cid);
                        }
                        self.snapshot_bees();
                        if matches!(role, Some(Role::Nurse) | Some(Role::Attendant)) {
                            self.breathe_foragers();
                        }
                        if role == Some(Role::Nurse) {
                            info!(client_id, ?models, "nurse.registered");
                        }
                        if let Some(hid) = manifest.hid {
                            self.advise_on_hello(client_id, &hid.to_hex(), &manifest.hive, &tone);
                        }
                    }

                    if client_id != "ensemble"
                        && let Some(ensemble) = &self.ensemble
                    {
                        let ens = ensemble.clone();
                        tokio::spawn(async move {
                            ens.bee_advertise(manifest).await;
                        });
                    }
                }
            }
            Some(Chi::Prompt) => {
                let sid = tone
                    .get("sid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() {
                    warn!(client_id, "prompt.no-sid");
                    return;
                }
                let model = tone
                    .get("modelId")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .map(str::to_string);
                let grows = |models: &[String]| model.as_ref().is_none_or(|m| models.iter().any(|x| x == m));
                let site = self.build_site(client_id, &sid, &tone);
                let mut tone = tone;
                if let Some(obj) = tone.as_object_mut() {
                    match &site {
                        Some(site) => {
                            obj.insert("cwd".into(), Value::String(site.uri()));
                        }
                        None => {
                            obj.remove("cwd");
                        }
                    }
                }
                match &site {
                    Some(site) => {
                        trace!(sid, at = %site.uri(), "prompt.builds");
                        self.build_sites.write().insert(sid.clone(), site.clone());
                    }
                    None => {
                        self.build_sites.write().remove(&sid);
                    }
                }
                let offered: Vec<String> = tone
                    .get("tools")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                self.forager_tools.write().insert(sid.clone(), offered);
                let system_prompt = tone
                    .get("systemPrompt")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let text = tone
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                    self.thrum
                        .thrum_to(client_id, thrumd::echo_tone(rid, true, None));
                }
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
                trace!(sid, ?model, ?origin, "thrum.recv.prompt");

                let (with_room, full) = {
                    let mut to_prune: Vec<String> = Vec::new();
                    let pick = {
                        let m = self.manifests.read();
                        let mut with_room: Option<String> = None;
                        let mut full: Option<String> = None;
                        let mut sorted: Vec<_> = m.iter().collect();
                        sorted.sort_by(|a, b| a.0.cmp(b.0));
                        for (cid, man) in sorted {
                            if man.plays(Role::Nurse) && grows(&man.models) {
                                if !self.thrum.is_connected(cid) {
                                    to_prune.push(cid.clone());
                                } else if man.is_full() {
                                    full.get_or_insert_with(|| cid.clone());
                                } else {
                                    with_room = Some(cid.clone());
                                    break;
                                }
                            }
                        }
                        (with_room, full)
                    };
                    if !to_prune.is_empty() {
                        let mut m = self.manifests.write();
                        for cid in &to_prune {
                            m.remove(cid);
                        }
                    }
                    pick
                };
                if with_room.is_none()
                    && let Some(peer) = self.pick_remote_nurse(model.as_deref())
                    && let Some(ens) = self.ensemble.clone()
                {
                    let mut forward = tone.clone();
                    if let Some(obj) = forward.as_object_mut() {
                        obj.insert("to".into(), Value::String(peer.to_hex()));
                        obj.insert("from".into(), Value::String(ens.me().to_hex()));
                    }
                    trace!(sid, ?model, peer = %peer.short(), "prompt.forward.remote");
                    match ens.route(forward).await {
                        Ok(()) => {
                            self.sid_growers.write().insert(sid.clone(), Grower { humd: peer, open: true });
                            return;
                        }
                        Err(e) => warn!(sid, peer = %peer.short(), err = %e,
                            "prompt.forward.remote.failed"),
                    }
                }
                let Some(nurse) = with_room.or(full) else {
                    warn!(sid, ?model, "prompt.no-nurse — no nurse here or on a trusted peer grows this model");
                    let message = match &model {
                        Some(model) => format!("no nurse here or on a trusted peer grows {model}"),
                        None => "no nurse is here, or on a trusted peer, to grow it".to_string(),
                    };
                    let err = serde_json::json!({
                        "chi": "error",
                        "sid": sid,
                        "code": "no-nurse",
                        "message": message,
                    });
                    if let (Some(origin), Some(ens)) = (origin, &self.ensemble) {
                        let mut out = err.clone();
                        if let Some(obj) = out.as_object_mut() {
                            obj.insert("to".into(), Value::String(origin.to_hex()));
                            obj.insert("from".into(), Value::String(ens.me().to_hex()));
                        }
                        let _ = ens.route(out).await;
                    }
                    self.thrum.thrum_broadcast(&sid, err);
                    return;
                };

                let mut forward = tone.clone();
                if let Some(obj) = forward.as_object_mut() {
                    if obj.get("content").is_none() && !text.is_empty() {
                        obj.insert(
                            "content".into(),
                            serde_json::json!([{ "kind": thrum_core::PetalKind::Text.as_str(), "text": text }]),
                        );
                    }
                    if obj.get("systemPrompt").is_none()
                        && let Some(sp) = system_prompt.as_ref()
                    {
                        obj.insert("systemPrompt".into(), Value::String(sp.clone()));
                    }
                    let tools = self.bloom_tools(client_id, site.is_some(), obj.get("tools"));
                    if tools.is_empty() {
                        obj.remove("tools");
                    } else {
                        obj.insert("tools".into(), Value::Array(tools));
                    }
                }
                trace!(sid, ?model, %nurse, "prompt.forward.to-nurse");
                self.sid_growers.write().remove(&sid);
                if let Some(origin) = origin {
                    self.sid_origins.write().insert(sid.clone(), origin);
                }
                self.thrum.thrum_to(&nurse, forward);
                self.waneman.tick(&sid);
            }
            Some(Chi::Cancel) => {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                    self.follow_to_grower(client_id, sid, &tone, false).await;
                    for nurse in self.playing(&[Role::Nurse]) {
                        self.thrum.thrum_to(&nurse, tone.clone());
                    }
                }
            }
            Some(Chi::Cleanup) => {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                    self.follow_to_grower(client_id, sid, &tone, true).await;
                    self.forget_sid(sid);
                    self.sid_origins.write().remove(sid);
                    self.observers.write().remove(sid);
                    for bee in self.playing(&[Role::Nurse, Role::Builder]) {
                        self.thrum.thrum_to(&bee, tone.clone());
                    }
                }
            }
            Some(Chi::Attach) => {
                let sid = tone
                    .get("sid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() {
                    warn!(client_id, "attach.no-sid");
                    return;
                }
                let hear_only = tone
                    .get("hearOnly")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if client_id == "ensemble" {
                    let peer = tone
                        .get("from")
                        .and_then(Value::as_str)
                        .and_then(parse_humd_id);
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
                } else if let Some(ensemble) = &self.ensemble
                    && let Some(to) = tone.get("to").and_then(Value::as_str)
                    && !to.is_empty()
                    && to != ensemble.me().to_hex()
                {
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
            Some(Chi::Detach) => {
                let sid = tone
                    .get("sid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if sid.is_empty() {
                    warn!(client_id, "detach.no-sid");
                    return;
                }
                if client_id == "ensemble" {
                    let peer = tone
                        .get("from")
                        .and_then(Value::as_str)
                        .and_then(parse_humd_id);
                    if let Some(peer) = peer {
                        let mut obs = self.observers.write();
                        if let Some(list) = obs.get_mut(&sid) {
                            list.retain(|p| *p != peer);
                            if list.is_empty() {
                                obs.remove(&sid);
                            }
                        }
                        trace!(client_id, sid, peer = %peer.short(), "observer.removed");
                    }
                } else if let Some(ensemble) = &self.ensemble
                    && let Some(to) = tone.get("to").and_then(Value::as_str)
                    && !to.is_empty()
                    && to != ensemble.me().to_hex()
                {
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
            Some(Chi::PeerAdd) => {
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.add");
                if let Some(ens) = self.ensemble.clone() {
                    let known = self.manifests.read().values().cloned().collect::<Vec<_>>();
                    tokio::spawn(async move {
                        for manifest in known {
                            ens.bee_advertise(manifest).await;
                        }
                    });
                }
            }
            Some(Chi::PeerRemove) => {
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.remove");
                if let Some(ensemble) = &self.ensemble
                    && let Ok(bytes) = hex::decode(humd_id)
                    && bytes.len() == 32
                {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(&bytes);
                    let gone = ensemble::Hid::from(id);
                    let live = ensemble
                        .peer_liveness(&gone, ensemble::LIVENESS_TTL)
                        .is_some_and(|l| l == ensemble::Liveness::Live);
                    if !live {
                        ensemble.remove_peer(&gone);
                        if self.remote_bees.write().remove(&gone).is_some() {
                            trace!(peer = %gone.short(), "discovery.evicted");
                        }
                    }
                    self.break_off_grown_on(gone);
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
                let call_id = tone
                    .get("callId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(cid) = call_id.as_deref() {
                    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("");
                    let why = tone.get("message").and_then(Value::as_str).unwrap_or("the tendril broke off");
                    let mut failed = failed_grasp(sid, cid, why);
                    if let Some(obj) = failed.as_object_mut() {
                        let said: serde_json::Map<String, Value> = ["code", "subtype"]
                            .into_iter()
                            .filter_map(|k| tone.get(k).map(|v| (k.to_string(), v.clone())))
                            .collect();
                        if !said.is_empty() {
                            obj.insert("result".into(), Value::Object(said));
                        }
                        if let Some(ext) = tone.get("ext") {
                            obj.insert("ext".into(), ext.clone());
                        }
                    }
                    let origin_peer = self.incoming_tendrils.write().remove(cid);
                    self.tendril_routes_peer.write().remove(cid);
                    let reacher = self.tendril_routes.write().remove(cid);
                    if let (Some(origin_peer), Some(ens)) = (origin_peer, &self.ensemble) {
                        let mut routed = failed;
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
                    if let Some(reacher) = reacher {
                        trace!(call_id = cid, %reacher, "error.route.to-reacher");
                        self.thrum.thrum_to(&reacher, failed);
                        return;
                    }
                }
                if let Some(sid) = tone.get("sid").and_then(Value::as_str).map(str::to_string) {
                    trace!(%sid, "error.broadcast.fallback");
                    self.thrum.thrum_broadcast(&sid, tone);
                }
            }
            Some(Chi::TendrilResult) => {
                let call_id = tone.get("callId").and_then(Value::as_str);

                if let Some(call_id) = call_id {
                    let origin_peer = self.incoming_tendrils.write().remove(call_id);
                    if let Some(origin_peer) = origin_peer
                        && let Some(ens) = &self.ensemble
                    {
                        let mut routed = tone.clone();
                        if let Some(obj) = routed.as_object_mut() {
                            obj.insert("to".into(), Value::String(origin_peer.to_hex()));
                            obj.insert("from".into(), Value::String(ens.me().to_hex()));
                        }
                        trace!(call_id, to = %origin_peer.short(),
                                "tendril-result.route.peer");
                        if let Err(e) = ens.route(routed).await {
                            warn!(err = %e, "tendril-result.peer.route.failed");
                        }
                        return;
                    }
                    self.tendril_routes_peer.write().remove(call_id);
                    if let Some(nurse) = self.tendril_routes.write().remove(call_id) {
                        trace!(call_id, %nurse, "tendril-result.route.local");
                        self.thrum.thrum_to(&nurse, tone.clone());
                        return;
                    }
                }
                if let Some(call_id) = call_id {
                    trace!(call_id, "tendril-result.unrouted");
                }
            }
            Some(Chi::Backfill) => {
                let Some(thehum) = self.thehum.as_ref() else {
                    trace!(client_id, "backfill.no-thehum");
                    return;
                };
                let author = tone
                    .get("author")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
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
                if tone.get("sid").and_then(Value::as_str).is_some() {
                    for nurse in self.playing(&[Role::Nurse]) {
                        self.thrum.thrum_to(&nurse, tone.clone());
                    }
                }
            }
            Some(Chi::ScoutResult)
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

fn my_capabilities() -> PeerCapabilities {
    PeerCapabilities {
        proto_version: thrum_core::THRUM_VERSION.into(),
        hosts: Vec::new(),
        can_relay: false,
        ..Default::default()
    }
}

fn failed_grasp(sid: &str, call_id: &str, why: &str) -> Value {
    let mut result = serde_json::json!({
        "chi": "tendril-result",
        "callId": call_id,
        "output": why,
        "isError": true,
    });
    if !sid.is_empty()
        && let Some(obj) = result.as_object_mut()
    {
        obj.insert("sid".into(), Value::String(sid.to_string()));
    }
    result
}

fn reserved(tool: &str) -> bool {
    tool == thrum_core::BASH || tool == thrum_core::SCENT
}

fn scent_model(tone: &Value) -> String {
    tone.pointer("/args/model").and_then(Value::as_str).unwrap_or("").to_string()
}

const BREATH_EVERY: Duration = Duration::from_secs(2);

fn parse_humd_id(s: &str) -> Option<ensemble::Hid> {
    ensemble::Hid::from_hex(s).ok()
}
