//! humd — the hum daemon.
//!
//! One process, every wing. Boot order is fixed: tracing → config → sockets →
//! state crates → trackers → thrum server → MCP server → nest pool → signals.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use mcpd::{serve as mcp_serve, Registry as McpRegistry};
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

fn thrum_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    runtime_dir().join("hum.sock.thrum")
}

fn http_socket_path() -> PathBuf {
    runtime_dir().join("hum.sock.http")
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
}

#[async_trait::async_trait]
impl ToneSink for HumdSink {
    async fn hear(&self, client_id: &str, tone: Tone) {
        let chi_str = tone.get("chi").and_then(Value::as_str).unwrap_or("?");
        // Parse chi into the typed enum where possible; unknown values fall
        // through to a generic trace.
        let chi: Option<Chi> = tone
            .get("chi")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok());

        match chi {
            Some(Chi::Hello) => {
                trace!(client_id, %chi_str, "thrum.recv.hello");
                // v0 ack: synthesize a minimal breath and ship it back. The
                // real breath builder lives in thrumd::breath_tone; we pass
                // an empty sessions view for v0.
                let breath = thrumd::breath_tone(serde_json::json!({}));
                self.thrum.thrum_to(client_id, breath);
            }
            Some(Chi::Prompt) => {
                trace!(client_id, %chi_str, "thrum.recv.prompt");
                // v0 ack: echo the incoming rid so the sender's wane advances.
                if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                    self.thrum.thrum_to(client_id, thrumd::echo_tone(rid, true, None));
                }
                // The big migration target: dispatch to nest, stream chunks
                // back. v0 is intentionally a no-op past the echo.
                let _ = &self.waneman;
            }
            Some(Chi::Cancel)
            | Some(Chi::Cleanup)
            | Some(Chi::Curate)
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
            // Daemon → nestler chis arriving from a nestler are protocol
            // violations; log but don't panic.
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

    // 7. Thrum server + sink.
    let thrum = Thrum::new();
    let sink: Arc<dyn ToneSink> =
        Arc::new(HumdSink { thrum: thrum.clone(), waneman: waneman.clone() });
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

    // 8. MCP HTTP server.
    let registry = McpRegistry::new();
    tokio::spawn(async move {
        if let Err(e) = mcp_serve(mcp_addr, registry).await {
            warn!(err = %e, "mcp.exit");
        }
    });

    // 9. Nest pool — pipe and pty perches.
    let pipe: Arc<dyn nest::Perch> = Arc::new(nest::PipePerch);
    let pty: Arc<dyn nest::Perch> = Arc::new(nest::PtyPerch);
    let nest_cfg = nest::pool::NestConfig {
        max_procs: cfg.max_procs as usize,
        idle_timeout: Duration::from_millis(cfg.idle_timeout),
    };
    let _nest = Arc::new(nest::Nest::new(nest_cfg, pipe, pty));

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
