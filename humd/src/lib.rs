//! humd — the hum daemon as a library.
//!
//! `cargo run -p humd` is the binary at `src/main.rs`; this lib is the
//! reusable boot path. Tests, simulators (`p2p-sim`), and embedders all
//! call [`run`] with a [`DaemonConfig`] instead of duplicating the wiring.
//!
//! Boot order is fixed: state crates → trackers → nest pool → thrum →
//! MCP → wait for shutdown. Tracing setup is the binary's job; the lib
//! never touches the global subscriber.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ensemble::Ensemble;
use mcp::{serve as mcp_serve, Registry as McpRegistry};
use serde_json::Value;
use thrumd::{serve as thrum_serve, Thrum, Tone, ToneSink};
use thrum_core::{Chi, WaneTracker};
use tracing::{info, trace, warn};

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

    let waneman = Arc::new(WaneTracker::new());
    let _drift = drift::Drift::new();
    let _drone = drone::Drone::new();

    let nest_cfg = nest::pool::NestConfig {
        max_procs: cfg.hum_cfg.max_procs as usize,
        idle_timeout: Duration::from_millis(cfg.hum_cfg.idle_timeout),
    };
    let nest_pool = Arc::new(nest::Nest::new(nest_cfg, cfg.perches.pipe, cfg.perches.pty));
    let mcp_url = format!("http://{}", cfg.mcp_addr);

    // Caller-owned Thrum (sim) vs daemon-owned (production). When the
    // caller owns it, we install our sink onto theirs and never bind.
    let (thrum, bind_thrum) = match cfg.thrum_override {
        Some(t) => (t, false),
        None => (Thrum::new(), true),
    };
    let sink: Arc<dyn ToneSink> = Arc::new(HumdSink {
        thrum: thrum.clone(),
        waneman: waneman.clone(),
        nest: nest_pool.clone(),
        mcp_url: mcp_url.clone(),
        cli_path: cfg.cli_path.clone(),
        ensemble: cfg.ensemble.clone(),
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
    if let Some(ens) = cfg.ensemble.clone() {
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
}

impl NestListener {
    /// Build a reply tone and dispatch — locally and/or to the origin
    /// peer. Idempotent across both branches: a reply to a local
    /// nestler just goes through thrum_broadcast; a reply for a remote
    /// origin gets stamped with `to:` and routed via the ensemble.
    /// Both happen when the listener serves a hum that is being
    /// observed locally AND owned by a peer.
    async fn dispatch_reply(&self, tone: serde_json::Map<String, Value>) {
        let mut value = Value::Object(tone);
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
            }
            Some(Chi::Prompt) => {
                let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
                if sid.is_empty() {
                    warn!(client_id, "prompt.no-sid");
                    return;
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

/// Parse a hex HumdId string back into the typed [`ensemble::HumdId`].
/// Returns None on bad length or non-hex characters.
fn parse_humd_id(s: &str) -> Option<ensemble::HumdId> {
    let bytes = hex::decode(s).ok()?;
    if bytes.len() != 32 { return None; }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(ensemble::HumdId(arr))
}
