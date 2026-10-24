//! humd binary entry. The boot logic lives in `humd::run` (lib.rs) so
//! tests and simulators can spawn humds without going through this entry.
//!
//! humd is the daemon AND the daemon-inspection CLI. Default invocation
//! (no args) is daemon mode. `--version` / `--help` are the minimum
//! handshake any CLI should support; richer inspection subcommands
//! (peers, drone) will plug in once humd exposes an RPC control
//! socket on `cfg.http_path`.

use anyhow::Result;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{info, trace};
use tracing_subscriber::EnvFilter;

use humd::DaemonConfig;
use thrum_core::THRUM_VERSION;

fn print_help() {
    println!("humd — hum daemon");
    println!();
    println!("Usage:");
    println!("  humd               run the daemon (default)");
    println!("  humd --version     print version");
    println!("  humd --help        this surface");
    println!();
    println!("User-facing CLI is `hum`. humd's own subcommands will grow as");
    println!("the daemon RPC surface lands (peers, drone, sessions).");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        match args[0].as_str() {
            "--version" | "-V" => {
                println!("humd {} (thrum {})", env!("HUM_VERSION"), THRUM_VERSION);
                return Ok(());
            }
            "--help" | "-h" | "help" => {
                print_help();
                return Ok(());
            }
            other => {
                eprintln!("humd: unknown argument '{other}' — run `humd --help`");
                std::process::exit(2);
            }
        }
    }

    hum_paths::init();
    let filter = EnvFilter::try_from_env("HUM_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("trace"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
    info!(version = %THRUM_VERSION, "humd.booting");

    for key in hum_paths::config::migrate_file() {
        info!(key = %key, "config.retired_key_dropped");
    }
    hum_paths::config::validate_or_exit();

    let cfg = DaemonConfig::from_env();
    info!(
        fs_roots = cfg.hum_cfg.fs.roots.len(),
        "config.loaded"
    );

    humd::run(cfg, wait_for_shutdown()).await
}

async fn wait_for_shutdown() {
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => trace!("shutdown.ctrl-c"),
        _ = term.recv() => trace!("shutdown.sigterm"),
        _ = int.recv() => trace!("shutdown.sigint"),
    }
}
