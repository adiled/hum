//! humd — the hum daemon as a library.
//!
//! `cargo run -p humd` is the binary at `src/main.rs`; this lib is the
//! reusable boot path. Tests, simulators (`p2p-sim`), and embedders all
//! call [`run`] with a [`DaemonConfig`] instead of duplicating the wiring.
//!
//! Boot order is fixed: state crates → trackers → nest pool → thrum →
//! MCP → wait for shutdown. Tracing setup is the binary's job; the lib
//! never touches the global subscriber.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ensemble::{Ensemble, HumdAddr, HumdId, HumdKey, PeerCapabilities};
use mcp::{serve as mcp_serve, Registry as McpRegistry};
use parking_lot::RwLock;
use serde_json::Value;
use thrumd::{serve as thrum_serve, Thrum, Tone, ToneSink};
use thrum_core::{Chi, WaneTracker};
use tracing::{info, trace, warn};

mod identity;
mod peers;
pub use identity::{key_path, load_or_mint_key};
pub use peers::{peers_path, PeerConfig};

/// Per-sid observer roster. Maps a hum's `sid` to a list of peer humds
/// that have asked (via `chi:"attach"`) to receive a copy of every
/// outbound reply tone. Shared between the HumdSink (writer on attach /
/// detach) and every NestListener serving that sid (reader on each
/// reply).
type Observers = Arc<RwLock<HashMap<String, Vec<HumdId>>>>;

// ── Public config ──────────────────────────────────────────────────────────

/// Pluggable nest backends. Default = the real claude-cli (pipe) and
/// claude-repl (pty) perches; sim driver swaps in `nest::MockPerch` so
/// nothing actually shells out.
pub struct PerchSet {
    pub pipe: Arc<dyn nest::Perch>,
    pub pty: Arc<dyn nest::Perch>,
}

impl Default for PerchSet {
    fn default() -> Self {
        Self {
            pipe: Arc::new(claude_cli::ClaudeCliPerch),
            pty: Arc::new(claude_repl::ClaudeReplPerch),
        }
    }
}

/// Where the daemon listens, and how it should pace itself. Construct
/// via [`DaemonConfig::from_env`] for production defaults or build one
/// by hand for tests / simulators.
///
/// Three sim hooks layered on top of the production fields:
/// `thrum_override` (caller-owned Thrum, skip the socket listener),
/// `ensemble` (peer registry for `to:`-addressed tones), `bind_mcp`
/// (skip the HTTP MCP listener), and `perches` (swap in mock perches).
pub struct DaemonConfig {
    pub thrum_path: PathBuf,
    pub http_path: PathBuf,
    pub mcp_addr: std::net::SocketAddr,
    pub penny_path: PathBuf,
    pub hum_cfg: config::HumConfig,
    pub cli_path: String,
    pub penny_persist_interval: Duration,
    /// When set, sim provides the Thrum and the daemon does NOT bind a
    /// unix socket. When None, humd builds its own Thrum and binds.
    pub thrum_override: Option<Thrum>,
    /// When set, daemon installs this Ensemble for inter-humd routing.
    /// When None, the daemon runs without a peer set (legacy single-host).
    pub ensemble: Option<Arc<Ensemble>>,
    /// Pluggable nest implementations (default = real claude-cli + claude-repl).
    pub perches: PerchSet,
    /// When false, skip mcp_serve too (sim doesn't need a live HTTP MCP).
    pub bind_mcp: bool,
    /// Cap on concurrent local hums. `Some(0)` means "always overflow to a
    /// peer"; `None` means unbounded (legacy behaviour). Used by the
    /// overflow-routing policy in the prompt arm of [`HumdSink::hear`].
    pub capacity_override: Option<usize>,
    /// Caller-owned WaneTracker. Sim supplies one so it can read/write
    /// wane values from the test driver. Production leaves this None and
    /// the daemon mints its own. Either way the sink uses the same
    /// shared Arc.
    pub waneman: Option<Arc<WaneTracker>>,
    /// Persistent humd identity. `from_env` loads (or mints + persists)
    /// from `$XDG_STATE_HOME/hum/humd.key`. Tests / sims leave this None
    /// and continue to generate ephemeral keys per spawn.
    pub humd_key: Option<Arc<HumdKey>>,
    /// Peers to dial on boot. `from_env` reads
    /// `$XDG_CONFIG_HOME/hum/peers.json`; missing file = empty list.
    pub bootstrap_peers: Vec<PeerConfig>,
}

impl DaemonConfig {
    pub fn from_env() -> Self {
        let runtime_dir = runtime_dir();
        let base = socket_base(&runtime_dir);
        let mut thrum_path = base.clone().into_os_string();
        thrum_path.push(".thrum");
        let mut http_path = base.into_os_string();
        http_path.push(".http");
        let mcp_port: u16 = std::env::var("HUM_MCP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(29147);
        // Identity is not fatal — if the state dir is unwritable we log
        // and run without a persisted key (legacy single-host).
        let humd_key = match identity::load_or_mint_key() {
            Ok(k) => Some(Arc::new(k)),
            Err(e) => {
                warn!(err = %e, "identity.load.failed");
                None
            }
        };
        let bootstrap_peers = peers::load();
        Self {
            thrum_path: PathBuf::from(thrum_path),
            http_path: PathBuf::from(http_path),
            mcp_addr: ([127, 0, 0, 1], mcp_port).into(),
            penny_path: runtime_dir.join("penny.json"),
            hum_cfg: config::load(),
            cli_path: std::env::var("CLAUDE_CLI_PATH").unwrap_or_else(|_| "claude".into()),
            penny_persist_interval: Duration::from_secs(10),
            thrum_override: None,
            ensemble: None,
            perches: PerchSet::default(),
            bind_mcp: true,
            capacity_override: None,
            waneman: None,
            humd_key,
            bootstrap_peers,
        }
    }
}

fn runtime_dir() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join("hum")
}

fn socket_base(runtime: &std::path::Path) -> PathBuf {
    if let Ok(p) = std::env::var("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    runtime.join("hum.sock")
}

// ── Public entry point ─────────────────────────────────────────────────────

/// Build a daemon and run until `shutdown` resolves.
///
/// `shutdown` is parameterized so the binary can plug ctrl-c/SIGTERM
/// while the simulator can plug an in-memory cancel token. The function
/// returns once the shutdown future resolves AND best-effort state has
/// been flushed.
pub async fn run<F>(cfg: DaemonConfig, shutdown: F) -> Result<()>
where
    F: std::future::Future<Output = ()> + Send,
{
    info!(
        thrum = %cfg.thrum_path.display(),
        http = %cfg.http_path.display(),
        mcp = %cfg.mcp_addr,
        "humd.sockets"
    );

    let _hums = hums::Hums::load();
    let penny = penny::Penny::load(&cfg.penny_path);
    penny.clone().spawn_persister(cfg.penny_path.clone(), cfg.penny_persist_interval);

    let waneman = cfg.waneman.clone().unwrap_or_else(|| Arc::new(WaneTracker::new()));
    let _drift = drift::Drift::new();
    let _drone = drone::Drone::new();

    // Bring up an Ensemble from the persisted identity when the caller
    // didn't supply one. Sim provides its own pre-wired Ensemble (with
    // InMemoryEndpoints); the production binary lets us mint one here so
    // peer dialling has something to install into.
    let ensemble_opt: Option<Arc<Ensemble>> = match cfg.ensemble.clone() {
        Some(e) => Some(e),
        None => cfg.humd_key.as_ref().map(|k| {
            let me = k.humd_id();
            info!(humd_id = %me, "ensemble.boot");
            Arc::new(Ensemble::new(me))
        }),
    };

    // Dial-on-boot. Each peer attempt is independently fallible — one
    // dead host shouldn't sink startup. TcpTransport is the only wired
    // transport today; other hint prefixes ("iroh:") are skipped with a
    // log line until their transports land.
    if let (Some(ens), Some(_key)) = (&ensemble_opt, &cfg.humd_key) {
        let my_caps = my_capabilities(&cfg);
        for peer in &cfg.bootstrap_peers {
            let tcp_hint = peer
                .hints
                .iter()
                .find_map(|h| h.strip_prefix("tcp:"));
            let Some(addr) = tcp_hint else {
                trace!(peer = %peer.humd_id.short(), "peers.skip.no-tcp-hint");
                continue;
            };
            let peer_addr = {
                let mut a = HumdAddr::new(peer.humd_id);
                for h in &peer.hints { a.hints.push(h.clone()); }
                a
            };
            match ensemble::TcpEndpoint::connect(addr, peer_addr, PeerCapabilities::default()).await {
                Ok(conn) => {
                    info!(peer = %peer.humd_id.short(), addr, "peer.dial.ok");
                    ens.add_peer_with_caps(conn as Arc<dyn ensemble::PeerConnection>, my_caps.clone());
                }
                Err(e) => {
                    warn!(peer = %peer.humd_id.short(), addr, err = %e, "peer.dial.failed");
                }
            }
        }
    } else if !cfg.bootstrap_peers.is_empty() {
        warn!(
            count = cfg.bootstrap_peers.len(),
            "peers.skip.no-identity-or-ensemble"
        );
    }

    // Stash so the rest of run() keeps working off the cfg-or-minted
    // ensemble instead of just cfg.ensemble.
    let ensemble_for_sink = ensemble_opt.clone();

    let nest_cfg = nest::pool::NestConfig {
        max_procs: cfg.hum_cfg.nest.max_procs as usize,
        idle_timeout: Duration::from_millis(cfg.hum_cfg.nest.idle_threshold_ms),
    };
    let nest_pool = Arc::new(nest::Nest::new(nest_cfg, cfg.perches.pipe, cfg.perches.pty));
    let mcp_url = format!("http://{}", cfg.mcp_addr);

    // Caller-owned Thrum (sim) vs daemon-owned (production). When the
    // caller owns it, we install our sink onto theirs and never bind.
    let (thrum, bind_thrum) = match cfg.thrum_override {
        Some(t) => (t, false),
        None => (Thrum::new(), true),
    };
    let observers: Observers = Arc::new(RwLock::new(HashMap::new()));
    let sink: Arc<dyn ToneSink> = Arc::new(HumdSink {
        thrum: thrum.clone(),
        waneman: waneman.clone(),
        nest: nest_pool.clone(),
        mcp_url: mcp_url.clone(),
        cli_path: cfg.cli_path.clone(),
        ensemble: ensemble_for_sink.clone(),
        observers: observers.clone(),
        capacity_override: cfg.capacity_override,
    });
    thrum.set_sink(sink);
    if bind_thrum {
        let thrum = thrum.clone();
        let path = cfg.thrum_path.clone();
        tokio::spawn(async move {
            if let Err(e) = thrum_serve(thrum, &path).await {
                warn!(err = %e, "thrum.exit");
            }
        });
    } else {
        trace!("thrum.override.installed");
    }

    if cfg.bind_mcp {
        let mcp_addr = cfg.mcp_addr;
        let registry = McpRegistry::new();
        tokio::spawn(async move {
            if let Err(e) = mcp_serve(mcp_addr, registry).await {
                warn!(err = %e, "mcp.exit");
            }
        });
    } else {
        trace!("mcp.bind.skipped");
    }

    // Ensemble inbound pump — every tone arriving from a peer humd is
    // injected back through our own Thrum's sink as if it had arrived
    // from a special "ensemble" client. This is how `to:`-routed tones
    // from peer-A reach peer-B's HumdSink dispatch.
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

    info!("humd.ready");
    shutdown.await;
    info!("humd.shutting-down");
    if let Err(e) = penny.save(&cfg.penny_path) {
        warn!(err = %e, "penny.save.failed");
    }
    info!("humd.exit");
    Ok(())
}

// ── ToneSink — the big chi dispatch ────────────────────────────────────────

struct HumdSink {
    thrum: Thrum,
    waneman: Arc<WaneTracker>,
    nest: Arc<nest::Nest>,
    mcp_url: String,
    cli_path: String,
    /// When present, tones with a `to:` hex addressed to a *different*
    /// humd are routed through here instead of being dispatched locally.
    ensemble: Option<Arc<Ensemble>>,
    /// Per-sid roster of peer humds tapping us in `hearOnly` mode. Read
    /// by every NestListener on each reply; written here on `attach` /
    /// `detach`.
    observers: Observers,
    /// Concurrent-hum cap. `Some(0)` triggers overflow routing in the
    /// prompt arm — local prompts get forwarded to a peer with spare
    /// capacity instead of awakening here. `None` = unbounded.
    capacity_override: Option<usize>,
}

struct NestListener {
    sid: String,
    thrum: Thrum,
    /// When the prompt arrived from a peer humd, `origin` carries that
    /// humd's id. Reply tones (chunk, finish, session-ready, error) get
    /// stamped `to: <origin>` and routed via the ensemble so they reach
    /// the originating peer. None ⇒ purely local nestler, normal
    /// thrum_broadcast suffices.
    origin: Option<ensemble::HumdId>,
    ensemble: Option<Arc<Ensemble>>,
    /// Shared with HumdSink — every reply tone fans out to whichever
    /// peer humds have registered themselves as observers of this sid.
    observers: Observers,
}

impl NestListener {
    /// Build a reply tone and dispatch — locally and/or to the origin
    /// peer. Idempotent across both branches: a reply to a local
    /// nestler just goes through thrum_broadcast; a reply for a remote
    /// origin gets stamped with `to:` and routed via the ensemble.
    /// Both happen when the listener serves a hum that is being
    /// observed locally AND owned by a peer.
    async fn dispatch_reply(&self, tone: serde_json::Map<String, Value>) {
        let value = Value::Object(tone);

        // Fan-out to observers (peer humds that sent `chi:"attach"` for
        // this sid). Each observer gets its own copy with `to: <obs>`
        // and `from: <me>`. This runs in addition to — not instead of —
        // the origin route and the local broadcast, so a hum can be
        // driven locally, owned remotely, and shadowed by N observers
        // all at once.
        if let Some(ens) = &self.ensemble {
            let obs = self.observers.read().get(&self.sid).cloned().unwrap_or_default();
            for peer in obs {
                let mut copy = value.clone();
                if let Some(obj) = copy.as_object_mut() {
                    obj.insert("to".into(), Value::String(peer.to_hex()));
                    obj.insert("from".into(), Value::String(ens.me().to_hex()));
                }
                if let Err(e) = ens.route(copy).await {
                    warn!(sid = %self.sid, peer = %peer.short(), err = %e, "reply.fanout.failed");
                }
            }
        }

        let mut value = value;
        if let (Some(origin), Some(ens)) = (&self.origin, &self.ensemble) {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("to".into(), Value::String(origin.to_hex()));
                obj.insert("from".into(), Value::String(ens.me().to_hex()));
            }
            if let Err(e) = ens.route(value.clone()).await {
                warn!(sid = %self.sid, err = %e, "reply.route.failed");
            }
        }
        // Local broadcast — no-op if no local clients claim the sigil.
        self.thrum.thrum_broadcast(&self.sid, "claude", value);
    }
}

#[async_trait::async_trait]
impl nest::Listener for NestListener {
    fn session_id(&self) -> &str { &self.sid }

    async fn on_petal(&self, kind: &str, payload: Value) {
        let mut body = serde_json::Map::new();
        body.insert("chi".into(), Value::String("chunk".into()));
        body.insert("sid".into(), Value::String(self.sid.clone()));
        body.insert("rid".into(), Value::String(thrum_core::rid()));
        body.insert("chunkType".into(), Value::String(kind.into()));
        if let Some(obj) = payload.as_object() {
            for (k, v) in obj { body.insert(k.clone(), v.clone()); }
        }
        self.dispatch_reply(body).await;
    }

    async fn on_roost(&self, nest_id: &str, model: &str, tools: Vec<String>) {
        let mut body = serde_json::Map::new();
        body.insert("chi".into(), Value::String("session-ready".into()));
        body.insert("sid".into(), Value::String(self.sid.clone()));
        body.insert("rid".into(), Value::String(thrum_core::rid()));
        body.insert("nestId".into(), Value::String(nest_id.into()));
        body.insert("model".into(), Value::String(model.into()));
        body.insert("tools".into(), serde_json::json!(tools));
        self.dispatch_reply(body).await;
    }

    async fn on_wilt(&self, finish_reason: &str, usage: Option<Value>, provider_meta: Value) {
        let mut body = serde_json::Map::new();
        body.insert("chi".into(), Value::String("finish".into()));
        body.insert("sid".into(), Value::String(self.sid.clone()));
        body.insert("rid".into(), Value::String(thrum_core::rid()));
        body.insert("finishReason".into(), Value::String(finish_reason.into()));
        body.insert("usage".into(), usage.unwrap_or(Value::Null));
        body.insert("providerMetadata".into(), provider_meta);
        self.dispatch_reply(body).await;
    }

    async fn on_thorn(&self, wound: &str) {
        let mut body = serde_json::Map::new();
        body.insert("chi".into(), Value::String("error".into()));
        body.insert("sid".into(), Value::String(self.sid.clone()));
        body.insert("rid".into(), Value::String(thrum_core::rid()));
        body.insert("message".into(), Value::String(wound.into()));
        self.dispatch_reply(body).await;
    }
}

#[async_trait::async_trait]
impl ToneSink for HumdSink {
    async fn hear(&self, client_id: &str, tone: Tone) {
        let chi_str = tone.get("chi").and_then(Value::as_str).unwrap_or("?");
        let chi: Option<Chi> = tone
            .get("chi")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());

        // Attach from a local nestler addressed at a peer humd needs a
        // sigil claim here *before* the cross-humd router whisks the tone
        // away — otherwise reply tones flowing back across the ensemble
        // pump get broadcast on the sid and find no claimant, falling
        // back to the unregistered-clients branch by luck. Claim first,
        // then let the standard routing block forward.
        if matches!(chi, Some(Chi::Attach)) && client_id != "ensemble" {
            if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                if !sid.is_empty() {
                    self.thrum.claim_sigil(client_id, thrum_core::sigil(sid, "claude"));
                    self.thrum.claim_sigil(client_id, sid.to_string());
                    let hear_only = tone.get("hearOnly").and_then(Value::as_bool).unwrap_or(false);
                    trace!(client_id, sid, hear_only, "attach.local.claimed");
                }
            }
        }

        // Cross-humd routing — if tone is addressed to a *different* humd
        // and we have an Ensemble, hand it off and stop here. Without an
        // Ensemble, `to:` is ignored (legacy single-host behaviour).
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

        // Inbound from a peer humd carrying daemon→nestler chi: forward
        // to local clients claiming the sid (the synthetic nestler that
        // originated the prompt). Without this, replies routed back
        // across the ensemble hit the daemon but never reach the
        // nestler tap.
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
                    self.thrum.thrum_broadcast(&sid, "claude", tone);
                    return;
                }
            }
        }

        match chi {
            Some(Chi::Hello) => {
                trace!(client_id, %chi_str, "thrum.recv.hello");
                let breath = thrumd::breath_tone(serde_json::json!({}));
                self.thrum.thrum_to(client_id, breath);

                // Advertise this nestler on the ensemble's nestling-discovery
                // topic so peer humds know which nestlings are available
                // here. Skipped if no ensemble is wired (sim or solo run).
                // Skipped for ensemble-routed tones: those came from a peer
                // humd whose nestlings we don't host.
                if client_id != "ensemble" {
                  if let Some(ensemble) = &self.ensemble {
                    let name = tone.get("nestling").and_then(Value::as_str)
                        .map(str::to_string)
                        // Older nestlings used `from` only; fall back to that
                        // so they get a manifest too.
                        .or_else(|| tone.get("from").and_then(Value::as_str).map(str::to_string));
                    if let Some(name) = name {
                        let proto = tone.get("protoVersion").and_then(Value::as_str)
                            .unwrap_or(thrum_core::THRUM_VERSION)
                            .to_string();
                        let version = tone.get("version").and_then(Value::as_str)
                            .or_else(|| tone.get("nestlingVersion").and_then(Value::as_str))
                            .unwrap_or("0.0.0")
                            .to_string();
                        let propensity = tone.get("propensity")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default();
                        // Hello tone has `chi: "hello"` (the discriminator),
                        // so the manifest's vocabulary list lives under
                        // `chis` (plural). Tolerate legacy `chi:[...]` form
                        // for back-compat with pre-rename hellos.
                        let chis: Vec<String> = tone.get("chis")
                            .or_else(|| tone.get("chi"))
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                            .unwrap_or_default();
                        let source = tone.get("source").and_then(Value::as_str).map(str::to_string);
                        let mut manifest = ensemble::NestlingManifest::new(name, version, proto);
                        manifest.propensity = propensity;
                        manifest.chis = chis;
                        manifest.source = source;
                        let ens = ensemble.clone();
                        tokio::spawn(async move {
                            ens.nestling_advertise(manifest).await;
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
                // Overflow routing — if this prompt came from a local
                // nestler AND we have no spare capacity, hand it to a
                // peer that advertises the nest kind with free slots.
                // Prompts arriving from a peer (`client_id == "ensemble"`)
                // are work *we* accepted from somebody else; never bounce
                // them again.
                if client_id != "ensemble"
                    && self.capacity_override.map(|c| c == 0).unwrap_or(false)
                {
                    if let Some(ensemble) = &self.ensemble {
                        let target = pick_overflow_peer(ensemble, "claude-cli");
                        if let Some(peer) = target {
                            // Claim the sid so reply tones (chunks +
                            // finish) routed back via the ensemble pump
                            // reach this client's queue.
                            self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, "claude"));
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
                let cwd = tone.get("cwd").and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
                let system_prompt = tone.get("systemPrompt").and_then(Value::as_str).map(str::to_string);
                let text = tone.get("text").and_then(Value::as_str).map(str::to_string)
                    .or_else(|| tone.get("content").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();
                if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                    self.thrum.thrum_to(client_id, thrumd::echo_tone(rid, true, None));
                }
                self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, "claude"));
                self.thrum.claim_sigil(client_id, &sid);
                // Origin detection: if the prompt arrived from a peer
                // (we received it via the ensemble pump, whose synthetic
                // client_id is "ensemble", and the tone carries a `from`
                // humd id that isn't us), reply tones must route back
                // there via the ensemble.
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
                let listener: Arc<dyn nest::Listener> = Arc::new(NestListener {
                    sid: sid.clone(),
                    thrum: self.thrum.clone(),
                    origin,
                    ensemble: self.ensemble.clone(),
                    observers: self.observers.clone(),
                });
                let mut spec = nest::SpawnSpec::new(sid.clone(), model.clone(), cwd.clone());
                spec.system_prompt = system_prompt;
                spec.mcp_url = Some(self.mcp_url.clone());
                spec.cli_path = Some(self.cli_path.clone());
                if let Err(e) = self.nest.awaken(&sid, listener, spec, false).await {
                    warn!(sid, err = %e, "nest.awaken.failed");
                    return;
                }
                if let Err(e) = self.nest.murmur(&sid, &sid, &text).await {
                    warn!(sid, err = %e, "nest.murmur.failed");
                }
                self.waneman.tick(&sid);
            }
            Some(Chi::Cancel) => {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                    let req_id = thrum_core::rid();
                    if let Err(e) = self.nest.interrupt(sid, &req_id).await {
                        trace!(sid, err = %e, "nest.interrupt.failed");
                    }
                }
            }
            Some(Chi::Cleanup) => {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                    self.nest.fell(sid).await;
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
                    // A peer humd is registering itself as an observer of
                    // a hum hosted here. Record so reply tones fan out.
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
                    // A local nestler is announcing it wants to observe a
                    // sid hosted on a peer. Claim the sigil so reply
                    // tones (which arrive via the ensemble pump and get
                    // broadcast on the sid) land in this client's queue,
                    // then forward the attach to the host humd.
                    self.thrum.claim_sigil(client_id, &thrum_core::sigil(&sid, "claude"));
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
                // Sim wires the connection into Ensemble directly; this
                // arm just records intent so peer-add tones round-trip
                // through the dispatcher for tests/logs.
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.add");
            }
            Some(Chi::PeerRemove) => {
                let humd_id = tone.get("humd_id").and_then(Value::as_str).unwrap_or("");
                trace!(client_id, humd_id, "ensemble.peer.remove");
                if let Some(ensemble) = &self.ensemble {
                    if let Ok(bytes) = hex::decode(humd_id) {
                        if bytes.len() == 32 {
                            let mut id = [0u8; 32];
                            id.copy_from_slice(&bytes);
                            ensemble.remove_peer(&ensemble::HumdId(id));
                        }
                    }
                }
            }
            Some(Chi::WaneSync) => {
                // Partition-heal reconciliation. Snapshot is a JSON object
                // of sigil → u64. Merge by max — wane is a Lamport clock,
                // max is the convergent join. We don't reply; both sides
                // emit on heal so each is informed exactly once.
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
            Some(Chi::Curate)
            | Some(Chi::ReleasePermit)
            | Some(Chi::TendrilResult)
            | Some(Chi::ToolResult)
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

/// Capabilities the daemon advertises in the hello we send when dialling
/// a bootstrap peer. Mirrors the local nest config so peers selecting an
/// overflow target see what we can actually host. `free_slots` is left
/// `None` (unspecified / unbounded) — the overflow heuristic in
/// `pick_overflow_peer` treats `None` as "available."
fn my_capabilities(cfg: &DaemonConfig) -> PeerCapabilities {
    let nest_name = cfg.hum_cfg.nest.default.clone();
    PeerCapabilities {
        proto_version: thrum_core::THRUM_VERSION.into(),
        nests: vec![nest_name],
        hosts: Vec::new(),
        can_relay: false,
        free_slots: None,
    }
}

/// Pick a peer to forward a prompt to when local capacity is exhausted.
/// Prefers peers that advertise the requested nest kind AND claim a
/// non-zero `free_slots`. Falls back to any peer with the nest kind.
/// Returns `None` if no peer in the ensemble is eligible.
fn pick_overflow_peer(ensemble: &Ensemble, nest_kind: &str) -> Option<HumdId> {
    let peers = ensemble.peers();
    // First pass: peer with the right nest kind AND advertised slots.
    let mut fallback: Option<HumdId> = None;
    for id in peers {
        let Some(caps) = ensemble.peer_caps(&id) else { continue };
        let has_nest = caps.nests.iter().any(|n| n == nest_kind);
        if !has_nest { continue; }
        match caps.free_slots {
            Some(n) if n > 0 => return Some(id),
            None => return Some(id), // unbounded
            Some(0) => { /* peer is full, skip but remember as fallback */
                if fallback.is_none() { fallback = Some(id); }
            }
            _ => {}
        }
    }
    fallback
}

/// Parse a hex HumdId string back into the typed [`ensemble::HumdId`].
/// Returns None on bad length or non-hex characters.
fn parse_humd_id(s: &str) -> Option<ensemble::HumdId> {
    let bytes = hex::decode(s).ok()?;
    if bytes.len() != 32 { return None; }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(ensemble::HumdId(arr))
}
