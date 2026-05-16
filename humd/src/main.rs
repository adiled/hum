//! humd binary entry. The boot logic lives in `humd::run` (lib.rs) so
//! tests and simulators can spawn humds without going through this entry.

use anyhow::Result;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{info, trace};
use tracing_subscriber::EnvFilter;

use humd::DaemonConfig;
use thrum_core::THRUM_VERSION;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_env("HUM_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
    info!(version = %THRUM_VERSION, "humd.booting");

    let cfg = DaemonConfig::from_env();
    info!(
        max_procs = cfg.hum_cfg.max_procs,
        nest = ?cfg.hum_cfg.nest,
        droned = cfg.hum_cfg.droned,
        "config.loaded"
    );

    humd::run(cfg, wait_for_shutdown()).await
}

/// Resolves the first time SIGTERM, SIGINT, or ctrl-c arrives.
async fn wait_for_shutdown() {
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => trace!("shutdown.ctrl-c"),
        _ = term.recv() => trace!("shutdown.sigterm"),
        _ = int.recv() => trace!("shutdown.sigint"),
    }
}
