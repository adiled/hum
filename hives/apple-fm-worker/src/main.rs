//! apple-fm-worker — standalone worker-bee process.
//!
//! Registers with humd as `bee:["worker"]`, advertises the on-device model id,
//! and routes prompts to Apple's on-device 3B FoundationModel via a small
//! Swift bridge (`bridge/gen`) that wraps `FoundationModels`. No network,
//! no API key.
//!
//! Environment:
//!   APPLE_FM_MODEL   — advertised model id (default: apple-3b-on-device)
//!   APPLE_FM_BRIDGE  — path to the bridge binary (default: <dir>/bridge/gen,
//!                      resolved relative to this binary; override to test)

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use hum_nest::{serve_worker, HiveAdvert};

use lib::AppleFmWorker;
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

    let model_id =
        std::env::var("APPLE_FM_MODEL").unwrap_or_else(|_| "apple-3b-on-device".into());

    // Resolve the bridge next to this binary: <bin>/../bridge/gen.
    let bridge = resolve_bridge();
    tracing::info!(bridge = %bridge.display(), "apple-fm-worker: bridge resolved");

    let worker = Arc::new(AppleFmWorker {
        model_id: model_id.clone(),
        bridge,
    });

    let advert = HiveAdvert {
        hive: "apple-fm-worker".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        models: vec![model_id],
        source: Some(
            "https://github.com/adiled/hum/tree/main/hives/apple-fm-worker".to_string(),
        ),
    };

    serve_worker(worker, advert).await
}

fn resolve_bridge() -> PathBuf {
    if let Ok(p) = std::env::var("APPLE_FM_BRIDGE") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        // <...>/apple-fm-worker → <...>/../bridge/gen
        let sibling = exe
            .parent()
            .map(|p| p.join("bridge/gen"))
            .unwrap_or_else(|| PathBuf::from("bridge/gen"));
        if sibling.exists() {
            return sibling;
        }
    }
    // Fallback: relative to CWD (common in dev).
    PathBuf::from("bridge/gen")
}

/// No-op kept for clarity: the bridge is only ever invoked on macOS, and the
/// capability probe inside `raise` handles the "Apple Intelligence off" case.
#[cfg(not(target_os = "macos"))]
fn resolve_bridge() -> std::path::PathBuf {
    // Not on macOS — the bridge doesn't exist. Return a sentinel path;
    // the probe inside `raise` will fail gracefully and humd will stop routing.
    std::path::PathBuf::from("/dev/null")
}
