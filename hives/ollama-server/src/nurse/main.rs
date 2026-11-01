//! ollama-nurse — the nurse half of the ollama-server hive.
//!
//! Its own process and its own bee, entered as a nurse under the label
//! `nurse` beside the forager's `main`: it serves the models in
//! `OLLAMA_NURSE_MODELS` through Ollama's API.
//!
//! Environment, an empty value being no value:
//!   OLLAMA_NURSE_URL      — Ollama API base URL (default: http://127.0.0.1:11434)
//!   OLLAMA_NURSE_MODEL    — default model (default: smollm:1.7b)
//!   OLLAMA_NURSE_MODELS   — comma-separated model list (default: OLLAMA_NURSE_MODEL)
//!   OLLAMA_NURSE_CTX      — context length (default: 8192)

use std::sync::Arc;

use anyhow::Result;
#[path = "../hop.rs"]
mod hop;
mod ollama;

use hum_nest::{serve_nurse, NurseAdvert};

use crate::ollama::OllamaNurse;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[tokio::main]
async fn main() -> Result<()> {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let default_model = var("OLLAMA_NURSE_MODEL").unwrap_or_else(|| "smollm:1.7b".into());
    let models: Vec<String> = var("OLLAMA_NURSE_MODELS")
        .unwrap_or(default_model)
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let nurse = Arc::new(OllamaNurse::default());
    let advert = NurseAdvert {
        hive: "ollama-server".to_string(),
        label: "nurse".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        models,
        source: Some("https://github.com/adiled/hum/tree/main/hives/ollama-server".to_string()),
    };

    serve_nurse(nurse, advert).await
}
