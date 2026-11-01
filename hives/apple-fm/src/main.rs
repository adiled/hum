//! apple-fm-nurse — Apple's on-device foundation model, as a nurse.
//!
//! It grows the model it advertises through `apple-fm-gen`, the Swift bridge
//! to FoundationModels beside it, and needs no network and no key. Before it
//! nestles it asks the bridge whether the model can run here. While Apple
//! Intelligence is off, or its model is still downloading, it waits without
//! nestling and asks again; on a Mac that cannot run the model at all it
//! says so and exits 0, so its unit stays down. A bridge that cannot be run
//! at all exits non-zero.
//!
//! Environment, an empty value being no value:
//!   APPLE_FM_MODEL     — the model id it advertises (default: apple-3b-on-device)
//!   APPLE_FM_LABEL     — the label its bee is entered as (default: main)
//!   APPLE_FM_BRIDGE    — the bridge (default: apple-fm-gen beside this binary)
//!   APPLE_FM_PROBE_MS  — how often a waiting nurse asks again (default: 60000)

mod bridge;
mod nurse;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use hum_nest::{serve_nurse, NurseAdvert};
use tracing::{info, warn};

use crate::bridge::Availability;
use crate::nurse::AppleFm;

const HIVE: &str = "apple-fm";

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn beside_me() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("apple-fm-gen")))
        .unwrap_or_else(|| PathBuf::from("apple-fm-gen"))
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

    let model = var("APPLE_FM_MODEL").unwrap_or_else(|| "apple-3b-on-device".into());
    let label = var("APPLE_FM_LABEL").unwrap_or_else(|| "main".into());
    let bridge = var("APPLE_FM_BRIDGE").map(PathBuf::from).unwrap_or_else(beside_me);
    let every = Duration::from_millis(var("APPLE_FM_PROBE_MS").and_then(|ms| ms.trim().parse().ok()).unwrap_or(60_000));

    let mut waiting: Option<String> = None;
    loop {
        match bridge::probe(&bridge).await? {
            Availability::Available => break,
            Availability::Never(reason) => {
                warn!(%reason, bridge = %bridge.display(), "apple-fm.never: this Mac cannot run Apple's on-device model, so this unit stays down");
                return Ok(());
            }
            Availability::Dormant(reason) => {
                if waiting.as_deref() != Some(reason.as_str()) {
                    warn!(%reason, every_ms = every.as_millis() as u64, "apple-fm.dormant: Apple's on-device model is not ready, asking again");
                }
                waiting = Some(reason);
                tokio::time::sleep(every).await;
            }
        }
    }
    info!(bridge = %bridge.display(), %model, "apple-fm.available");

    let advert = NurseAdvert {
        hive: HIVE.to_string(),
        label,
        version: env!("CARGO_PKG_VERSION").to_string(),
        models: vec![model],
        source: Some("https://github.com/adiled/hum/tree/main/hives/apple-fm".to_string()),
    };
    serve_nurse(Arc::new(AppleFm { bridge }), advert).await
}
