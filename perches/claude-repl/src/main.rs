//! claude-repl-perch — standalone perch process for the PTY/REPL
//! variant of claude. Runs as its own process, handshakes with humd
//! over thrum.

use std::sync::Arc;

use anyhow::Result;
use nest_common::{serve_perch, PerchAdvert};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_env("HUM_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    let perch = Arc::new(claude_repl::ClaudeReplPerch);

    let models: Vec<String> = std::env::var("CLAUDE_MODELS")
        .ok()
        .map(|s| s.split(',').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect())
        .unwrap_or_else(|| vec![
            "claude-opus-4-7".to_string(),
            "claude-sonnet-4-6".to_string(),
            "claude-haiku-4-5".to_string(),
        ]);

    let advert = PerchAdvert {
        kind: "claude-repl".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        models,
        source: Some("https://github.com/adiled/hum/tree/main/perches/claude-repl".to_string()),
    };

    serve_perch(perch, advert).await
}
