//! ollama-worker — standalone worker-bee process.
//!
//! Registers with humd as `bee:["worker"]`, advertises models from
//! `OLLAMA_WORKER_MODELS`, and routes prompts to Ollama's API.
//!
//! Environment:
//!   OLLAMA_WORKER_URL      — Ollama API base URL (default: http://127.0.0.1:11434)
//!   OLLAMA_WORKER_MODEL    — default model (default: llama3.2)
//!   OLLAMA_WORKER_MODELS   — comma-separated model list (default: OLLAMA_WORKER_MODEL)
//!   OLLAMA_WORKER_CTX      — context length (default: 8192)

use std::sync::Arc;

use anyhow::Result;
use nest_common::{serve_worker, HiveAdvert};

use lib::OllamaWorker;
mod lib;

#[tokio::main]
async fn main() -> Result<()> {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let default_model = std::env::var("OLLAMA_WORKER_MODEL")
        .unwrap_or_else(|_| "smollm:1.7b".into());
    let models_env = std::env::var("OLLAMA_WORKER_MODELS")
        .unwrap_or_else(|_| default_model.clone());
    let models: Vec<String> = models_env
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let worker = Arc::new(OllamaWorker::default());
    let advert = HiveAdvert {
        hive: "ollama-worker".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        models,
        source: Some("https://github.com/adiled/hum/tree/main/hives/ollama-worker".to_string()),
    };

    serve_worker(worker, advert).await
}
