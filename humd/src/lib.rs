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
use mcp::{serve as mcp_serve, Registry as McpRegistry};
use serde_json::Value;
use thrumd::{serve as thrum_serve, Thrum, Tone, ToneSink};
use thrum_core::{Chi, WaneTracker};
use tracing::{info, trace, warn};

// ── Public config ──────────────────────────────────────────────────────────

/// Where the daemon listens, and how it should pace itself. Construct
/// via [`DaemonConfig::from_env`] for production defaults or build one
/// by hand for tests / simulators.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub thrum_path: PathBuf,
    pub http_path: PathBuf,
    pub mcp_addr: std::net::SocketAddr,
    pub penny_path: PathBuf,
    pub hum_cfg: config::HumConfig,
    pub cli_path: String,
    pub penny_persist_interval: Duration,
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

    let pipe: Arc<dyn nest::Perch> = Arc::new(claude_cli::ClaudeCliPerch);
    let pty: Arc<dyn nest::Perch> = Arc::new(claude_repl::ClaudeReplPerch);
    let nest_cfg = nest::pool::NestConfig {
        max_procs: cfg.hum_cfg.max_procs as usize,
        idle_timeout: Duration::from_millis(cfg.hum_cfg.idle_timeout),
    };
    let nest_pool = Arc::new(nest::Nest::new(nest_cfg, pipe, pty));
    let mcp_url = format!("http://{}", cfg.mcp_addr);

    let thrum = Thrum::new();
    let sink: Arc<dyn ToneSink> = Arc::new(HumdSink {
        thrum: thrum.clone(),
        waneman: waneman.clone(),
        nest: nest_pool.clone(),
        mcp_url: mcp_url.clone(),
        cli_path: cfg.cli_path.clone(),
    });
    thrum.set_sink(sink);
    {
        let thrum = thrum.clone();
        let path = cfg.thrum_path.clone();
        tokio::spawn(async move {
            if let Err(e) = thrum_serve(thrum, &path).await {
                warn!(err = %e, "thrum.exit");
            }
        });
    }

    let mcp_addr = cfg.mcp_addr;
    let registry = McpRegistry::new();
    tokio::spawn(async move {
        if let Err(e) = mcp_serve(mcp_addr, registry).await {
            warn!(err = %e, "mcp.exit");
        }
    });

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
}

struct NestListener {
    sid: String,
    thrum: Thrum,
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
        self.thrum.thrum_broadcast(&self.sid, "claude", Value::Object(body));
    }

    async fn on_roost(&self, nest_id: &str, model: &str, tools: Vec<String>) {
        let tone = serde_json::json!({
            "chi": "session-ready",
            "sid": self.sid,
            "rid": thrum_core::rid(),
            "nestId": nest_id,
            "model": model,
            "tools": tools,
        });
        self.thrum.thrum_broadcast(&self.sid, "claude", tone);
    }

    async fn on_wilt(&self, finish_reason: &str, usage: Option<Value>, provider_meta: Value) {
        let tone = serde_json::json!({
            "chi": "finish",
            "sid": self.sid,
            "rid": thrum_core::rid(),
            "finishReason": finish_reason,
            "usage": usage.unwrap_or(Value::Null),
            "providerMetadata": provider_meta,
        });
        self.thrum.thrum_broadcast(&self.sid, "claude", tone);
    }

    async fn on_thorn(&self, wound: &str) {
        let tone = serde_json::json!({
            "chi": "error",
            "sid": self.sid,
            "rid": thrum_core::rid(),
            "message": wound,
        });
        self.thrum.thrum_broadcast(&self.sid, "claude", tone);
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
                trace!(sid, model, "thrum.recv.prompt");
                let listener: Arc<dyn nest::Listener> = Arc::new(NestListener {
                    sid: sid.clone(),
                    thrum: self.thrum.clone(),
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
