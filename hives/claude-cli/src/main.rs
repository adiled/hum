//! claude-cli — standalone nurse process.
//!
//! Runs as its own process, handshakes with humd over thrum, accepts
//! prompts addressed to claude models, and pipes each bloom back as
//! `chi:"unfurl"` tones. humd never compiles us in; we register at
//! runtime through our hello.
//!
//! Env knobs:
//!   HUM_THRUM_SOCK     thrum socket (default `$XDG_STATE_HOME/hum/thrum.sock`)
//!   CLAUDE_CLI_PATH    claude binary (default `claude`, by hum_paths::program_dirs and ~/.claude/local)
//!   CLAUDE_MODELS      comma-separated models advertised on hello
//!                       (default: claude-opus-4-7,claude-sonnet-4-6,claude-haiku-4-5)

use std::sync::Arc;

use anyhow::Result;
use hum_nest::{serve_nurse, NurseAdvert};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    hum_paths::init();
    let filter = EnvFilter::try_from_env("HUM_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    match claude_cli::claude_program() {
        Ok(cli) => info!(claude = %cli.display(), "claude-cli.program"),
        Err(e) => warn!("claude-cli.program-missing — {e}"),
    }

    let nurse = Arc::new(claude_cli::ClaudeCliNurse);

    let models: Vec<String> = std::env::var("CLAUDE_MODELS")
        .ok()
        .map(|s| s.split(',').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect())
        .unwrap_or_else(|| vec![
            "claude-opus-4-7".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-haiku-4-5".to_string(),
            "claude-sonnet-4-5".to_string(),
            "claude-haiku-4-5-20251001".to_string(),
        ]);

    let advert = NurseAdvert {
        hive: "claude-cli".to_string(),
        label: "main".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        models,
        source: Some("https://github.com/adiled/hum/tree/main/hives/claude-cli".to_string()),
    };

    serve_nurse(nurse, advert).await
}
