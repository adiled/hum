//! humd — the hum daemon.
//!
//! One process, every wing. Boot order is fixed: tracing → config → sockets →
//! state crates → trackers → thrum server → MCP server → nest pool → signals.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use mcp::{serve as mcp_serve, Registry as McpRegistry};
use serde_json::Value;
use thrumd::{serve as thrum_serve, Thrum, Tone, ToneSink};
use thrum_core::{Chi, WaneTracker, THRUM_VERSION};
use tokio::signal::unix::{signal, SignalKind};
use tracing::{info, trace, warn};
use tracing_subscriber::EnvFilter;

// ── socket / port defaults ──────────────────────────────────────────────────

const MCP_PORT: u16 = 29147;
const PENNY_PERSIST_INTERVAL: Duration = Duration::from_secs(10);

/// `$XDG_RUNTIME_DIR/hum/`, or `/tmp/hum/` if unset. Same fallback the TS
/// daemon uses — the smoke example reads the same env var.
fn runtime_dir() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join("hum")
}

/// Base socket path. Matches the TS daemon convention: `HUM_SOCKET` (if
/// set) is the *base*, and the daemon appends `.thrum` / `.http` to it
/// for the two surfaces. Default base: `$XDG_RUNTIME_DIR/hum/hum.sock`.
fn socket_base() -> PathBuf {
    if let Ok(p) = std::env::var("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    runtime_dir().join("hum.sock")
}

fn thrum_socket_path() -> PathBuf {
    let mut p = socket_base().into_os_string();
    p.push(".thrum");
    PathBuf::from(p)
}

fn http_socket_path() -> PathBuf {
    let mut p = socket_base().into_os_string();
    p.push(".http");
    PathBuf::from(p)
}

fn penny_path() -> PathBuf {
    // Penny lives under XDG_STATE_HOME alongside hums; runtime_dir is
    // ephemeral but fine for v0 so the daemon stays self-contained.
    runtime_dir().join("penny.json")
}

// ── ToneSink — the big chi dispatch ─────────────────────────────────────────

/// State the handler closes over. Everything Arc-clonable so dispatch tasks
/// can be spawned freely without lifetime pain.
struct HumdSink {
    thrum: Thrum,
    waneman: Arc<WaneTracker>,
    nest: Arc<nest::Nest>,
    mcp_url: String,
    cli_path: String,
}

/// Listener that bridges nest petals → thrum chunks for one sid.
/// Translates each Claude stream event into a `chi:"chunk"` tone broadcast
/// over thrum to the nestler(s) attached to this sid.
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
                // Claim the sid on the connection so daemon→nestler tones route correctly.
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

// ── boot ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Tracing — HUM_LOG_LEVEL drives the filter, default trace.
    let filter = EnvFilter::try_from_env("HUM_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
    info!(version = %THRUM_VERSION, "humd.booting");

    // 2. Config.
    let cfg = config::load();
    info!(
        max_procs = cfg.max_procs,
        nest = ?cfg.nest,
        droned = cfg.droned,
        "config.loaded"
    );

    // 3. Socket / port resolution.
    let thrum_path = thrum_socket_path();
    let http_path = http_socket_path();
    let mcp_addr: std::net::SocketAddr = ([127, 0, 0, 1], MCP_PORT).into();
    info!(
        thrum = %thrum_path.display(),
        http = %http_path.display(),
        mcp = %mcp_addr,
        "humd.sockets"
    );

    // 4. Hums — session registry.
    let _hums = hums::Hums::load();

    // 5. Penny — lifetime counters, plus background persister.
    let penny = penny::Penny::load(&penny_path());
    penny.clone().spawn_persister(penny_path(), PENNY_PERSIST_INTERVAL);

    // 6. Trackers.
    let waneman = Arc::new(WaneTracker::new());
    let _drift = drift::Drift::new();
    let _drone = drone::Drone::new();

    // 7. Nest pool — built first so HumdSink can dispatch prompts to it.
    let pipe: Arc<dyn nest::Perch> = Arc::new(claude_cli::ClaudeCliPerch);
    let pty: Arc<dyn nest::Perch> = Arc::new(claude_repl::ClaudeReplPerch);
    let nest_cfg = nest::pool::NestConfig {
        max_procs: cfg.max_procs as usize,
        idle_timeout: Duration::from_millis(cfg.idle_timeout),
    };
    let nest_pool = Arc::new(nest::Nest::new(nest_cfg, pipe, pty));
    let cli_path = std::env::var("CLAUDE_CLI_PATH").unwrap_or_else(|_| "claude".into());
    let mcp_url = format!("http://{}", mcp_addr);

    // 8. Thrum server + sink (sink holds nest + mcp wiring).
    let thrum = Thrum::new();
    let sink: Arc<dyn ToneSink> = Arc::new(HumdSink {
        thrum: thrum.clone(),
        waneman: waneman.clone(),
        nest: nest_pool.clone(),
        mcp_url: mcp_url.clone(),
        cli_path: cli_path.clone(),
    });
    thrum.set_sink(sink);
    {
        let thrum = thrum.clone();
        let path = thrum_path.clone();
        tokio::spawn(async move {
            if let Err(e) = thrum_serve(thrum, &path).await {
                warn!(err = %e, "thrum.exit");
            }
        });
    }

    // 9. MCP HTTP server.
    let registry = McpRegistry::new();
    tokio::spawn(async move {
        if let Err(e) = mcp_serve(mcp_addr, registry).await {
            warn!(err = %e, "mcp.exit");
        }
    });

    // 10. Signal handlers — graceful shutdown persists penny then exits.
    let shutdown = wait_for_shutdown();

    info!("humd.ready");
    shutdown.await;
    info!("humd.shutting-down");
    // Best-effort flush — errors are traced, not propagated.
    if let Err(e) = penny.save(&penny_path()) {
        warn!(err = %e, "penny.save.failed");
    }
    info!("humd.exit");
    Ok(())
}

/// Resolves the first time SIGTERM, SIGINT, or ctrl-c arrives. tokio's
/// `ctrl_c` covers the interactive case; signal streams cover daemonised
/// runs (systemd, docker stop).
async fn wait_for_shutdown() {
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => trace!("shutdown.ctrl-c"),
        _ = term.recv() => trace!("shutdown.sigterm"),
        _ = int.recv() => trace!("shutdown.sigint"),
    }
}
