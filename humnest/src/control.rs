use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tracing::{info, trace, warn};

use crate::supervisor::Supervisor;

pub async fn serve(path: PathBuf, supervisor: Arc<Supervisor>) -> Result<JoinHandle<()>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        std::fs::remove_file(&path).ok();
    }
    let listener = UnixListener::bind(&path)?;
    info!(path = %path.display(), "humnest.control.listening");

    let info = hum_paths::HumnestRuntimeInfo {
        socket: path.clone(),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        bound_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0),
    };
    if let Err(e) = info.write() {
        warn!(err = %e, "humnest.runtime.write_failed");
    }

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((sock, _)) => {
                    let sup = supervisor.clone();
                    tokio::spawn(async move { handle_conn(sock, sup).await; });
                }
                Err(e) => warn!(err = %e, "humnest.control.accept_failed"),
            }
        }
    });
    Ok(handle)
}

async fn handle_conn(stream: UnixStream, supervisor: Arc<Supervisor>) {
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if line.is_empty() { continue; }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => { trace!(err = %e, "humnest.parse.skip"); continue; }
        };
        let chi = tone.get("chi").and_then(|c| c.as_str()).unwrap_or("");
        let reply = match chi {
            "humnest-spawn" => {
                let kind = tone.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                spawn_by_kind(&supervisor, kind).await
            }
            "humnest-kill" => {
                let kind = tone.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                match supervisor.kill_one(kind).await {
                    Ok(_) => json!({"chi":"humnest-ok"}),
                    Err(e) => json!({"chi":"humnest-err","message":e.to_string()}),
                }
            }
            "humnest-list" => {
                let bees = supervisor.list();
                json!({"chi":"humnest-list","bees":bees})
            }
            other => json!({"chi":"humnest-err","message":format!("unknown chi: {other}")}),
        };
        let line = format!("{}\n", reply);
        if w.write_all(line.as_bytes()).await.is_err() { break; }
    }
}

async fn spawn_by_kind(supervisor: &Arc<Supervisor>, kind: &str) -> Value {
    let cfg = config::load();
    match cfg.humnest.bees.iter().find(|b| b.kind == kind).cloned() {
        Some(bc) => match supervisor.clone().spawn_one(bc).await {
            Ok(_) => json!({"chi":"humnest-ok"}),
            Err(e) => json!({"chi":"humnest-err","message":e.to_string()}),
        }
        None => json!({"chi":"humnest-err","message":format!("no bee of kind '{kind}' in hum.json")}),
    }
}
