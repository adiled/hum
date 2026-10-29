//! apple-fm-worker — WorkerBee that runs Apple's on-device FoundationModel.
//!
//! Mirrors `ollama-worker`, but instead of an HTTP client it spawns a one-shot
//! Swift bridge (`bridge/gen`) that wraps `FoundationModels` and speaks NDJSON
//! on stdout. The bridge is the ONLY Apple-specific artifact; this Rust worker
//! is platform-neutral and degrades to "no models" on non-macOS hosts.
//!
//! stdin  to gen:  {"system":"...","prompt":"..."}   (one request per spawn)
//! stdout from gen: {"event":"delta","text":"..."} / {"event":"finish"}
//!                  {"event":"error","message":"..."} / {"event":"probe","available":bool}
//!
//! Each `feed` message spawns a fresh `gen` (the bridge holds no state), so the
//! cell transparently handles follow-up turns in the same session.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use hum_nest::{Cell, Egg, Propensity, WorkerBee};
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub struct AppleFmWorker {
    pub model_id: String,
    pub bridge: PathBuf,
}

#[async_trait]
impl WorkerBee for AppleFmWorker {
    fn ephemeral(&self) -> bool { false }

    fn propensity(&self) -> Propensity {
        // The bridge is stateless per spawn, so humd can reuse the cell for
        // follow-up turns in the same sid — each turn just spawns a fresh gen.
        Propensity::StatefulSession
    }

    async fn raise(&self, egg: Egg) -> Result<Cell> {
        let model = if egg.model_id.is_empty() {
            self.model_id.clone()
        } else {
            egg.model_id.clone()
        };
        let system = egg.system_prompt.clone();
        let bridge = self.bridge.clone();

        info!(model = %model, bridge = %bridge.display(), cwd = %egg.cwd, "apple-fm-worker.raise");

        let (tx_feed, mut rx_feed) = tokio::sync::mpsc::channel::<String>(64);
        let (tx_mmm, rx_mmm) = tokio::sync::mpsc::channel::<Value>(256);
        let silence = CancellationToken::new();

        let task_silence = silence.clone();
        let wire = tx_mmm.clone();
        tokio::spawn(async move {
            // Handle every prompt fed to this cell until the feed closes.
            while let Some(prompt) = rx_feed.recv().await {
                if task_silence.is_cancelled() { break; }
                run_one(&bridge, &system, &prompt, &wire, &task_silence).await;
                if task_silence.is_cancelled() { break; }
            }
            warn!("apple-fm-worker: feed closed");
        });

        let (_tx_exit, rx_exit) = tokio::sync::oneshot::channel::<i32>();

        Ok(Cell {
            mark: None,
            feed: tx_feed,
            mmm: Arc::new(Mutex::new(rx_mmm)),
            emerged: rx_exit,
            ephemeral: false,
            silence,
        })
    }
}

/// Spawn the bridge for one prompt and forward its NDJSON events as hum tones.
async fn run_one(
    bridge: &PathBuf,
    system: &Option<String>,
    prompt: &str,
    wire: &tokio::sync::mpsc::Sender<Value>,
    silence: &CancellationToken,
) {
    // Capability gate: if the on-device model is unavailable (Apple Intelligence
    // off, or unsupported host), report a graceful error and stop. humd will
    // simply stop routing to this model — exactly like ollama-worker idling when
    // no Ollama server is up.
    if !probe_available(bridge, silence).await {
        let _ = wire
            .send(json!({
                "chi": "error",
                "code": "apple_fm_unavailable",
                "message": "on-device FoundationModel unavailable (Apple Intelligence not enabled)",
            }))
            .await;
        return;
    }

    let mut child = match Command::new(bridge).stdin(std::process::Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = wire
                .send(json!({
                    "chi": "error",
                    "code": "apple_fm_spawn",
                    "message": format!("failed to spawn bridge {bridge:?}: {e}"),
                }))
                .await;
            return;
        }
    };

    // Write the request to the bridge's stdin, then close it.
    let request = json!({
        "system": system,
        "prompt": prompt,
    });
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(request.to_string().as_bytes()).await;
        let _ = stdin.flush().await;
        drop(stdin);
    }

    let stdout = child.stdout.take();
    let mut child = child;
    let out = match stdout {
        Some(o) => o,
        None => return,
    };

    // Read NDJSON lines from the bridge's stdout, forwarding deltas/finish/errors.
    // `tokio::io::BufReader` + line reading keeps this simple.
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut reader = BufReader::new(out);
    let mut buf = String::new();
    loop {
        if silence.is_cancelled() { break; }
        buf.clear();
        match reader.read_line(&mut buf).await {
            Ok(0) => break, // EOF — bridge finished
            Ok(_) => {}
            Err(e) => {
                let _ = wire
                    .send(json!({
                        "chi": "error",
                        "code": "apple_fm_read",
                        "message": format!("bridge stdout read error: {e}"),
                    }))
                    .await;
                break;
            }
        }
        let line = buf.trim();
        if line.is_empty() { continue; }

        let obj: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                warn!(err = %e, line = line.chars().take(200).collect::<String>(), "apple-fm-worker.parse");
                continue;
            }
        };
        match obj.get("event").and_then(|e| e.as_str()) {
            Some("delta") => {
                let text = obj.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if !text.is_empty() {
                    let _ = wire.send(json!({
                        "chi": "chunk",
                        "chunkType": "text_delta",
                        "delta": text,
                    })).await;
                }
            }
            Some("finish") => {
                let _ = wire.send(json!({
                    "chi": "finish",
                    "finishReason": "stop",
                })).await;
            }
            Some("error") => {
                let msg = obj.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                let _ = wire.send(json!({
                    "chi": "error",
                    "code": "apple_fm_bridge",
                    "message": msg,
                })).await;
            }
            other => warn!(other = ?other, "apple-fm-worker: unexpected bridge event"),
        }
    }

    // If the loop ended without a finish tone (e.g. bridge died or we were
    // cancelled), close out cleanly so humd gets a terminal event.
    let _ = child.wait().await;
}

/// Ask the bridge whether the on-device model is currently available.
async fn probe_available(bridge: &PathBuf, _silence: &CancellationToken) -> bool {
    let out = Command::new(bridge)
        .arg("--probe")
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {
            // Best-effort parse of the "available" flag; default to true on success.
            let s = String::from_utf8_lossy(&o.stdout);
            !s.contains("\"available\":false")
        }
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            if s.contains("\"available\":false") { false } else { true }
        }
        Err(e) => {
            warn!(err = %e, "apple-fm-worker: probe spawn failed");
            false
        }
    }
}
