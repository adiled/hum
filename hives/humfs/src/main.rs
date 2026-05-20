//! humfs-forager — hum's native filesystem hive.
//!
//! Stands up a thrum-attached forager process that advertises hum's
//! filesystem tool surface (`humfs_read`, `humfs_do_code`,
//! `humfs_do_noncode`, `humfs_bash`, future `humfs_task`) and handles
//! `chi:"tool-call"` tones humd routes here. The other-forager-of-
//! forager pattern: openai-server (or any nestler) sends a tool-call,
//! humd routes by `toolName` to this hive, this hive executes
//! against its local disk (gated by `fs.roots` from its hum.json),
//! and ships back `chi:"tool-result"`.
//!
//! Not yet a worker bee — it produces no LLM compute. Pure forager:
//! it translates `chi:"tool-call"` ↔ filesystem operations. Hybrid
//! bees (worker + forager) are allowed by the bee model but humfs
//! sticks to forager today.

use std::sync::Arc;

use anyhow::Result;
use nest_common::{serve_forager, ForagerAdvert};
use tracing_subscriber::EnvFilter;

mod ast;
mod dispatch;
mod tools;

use dispatch::HumfsDispatcher;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("HUM_LOG_LEVEL")
                .unwrap_or_else(|_| EnvFilter::new("info,humfs=trace")),
        )
        .init();

    let dispatcher = Arc::new(HumfsDispatcher::new());
    let advert = ForagerAdvert {
        hive: "humfs".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        source: Some("https://github.com/adiled/hum/tree/main/hives/humfs".into()),
        // Hive-level capability claim: humfs owns the fs surface
        // for whichever humd it attaches to. humd uses this to
        // deauthorize nestler-declared fs tools (Read/Write/Edit/
        // Glob/Grep/Bash/MultiEdit) from MCP tools/list so the
        // asking nestler's catalogue doesn't shadow humfs_*.
        provides: vec!["fs".into()],
    };
    serve_forager(dispatcher, advert).await
}
