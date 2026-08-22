//! ollama-worker — WorkerBee that calls Ollama's HTTP API.
//!
//! Sends prompts to Ollama's `/api/chat` endpoint and streams the
//! response back over thrum. Run any local LLM Ollama supports.
//!
//! Environment:
//!   OLLAMA_WORKER_URL      — Ollama API base URL (default: http://127.0.0.1:11434)
//!   OLLAMA_WORKER_MODEL    — default model (default: smollm:1.7b)
//!   OLLAMA_WORKER_CTX      — context length (default: 8192)

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, warn};

use nest::{Cell, Egg, Propensity, WorkerBee};

/// Ollama chat request body — the exact shape Ollama's API expects.
#[derive(Debug, Clone, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    options: Option<OllamaOptions>,
}

#[derive(Debug, Clone, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
struct OllamaOptions {
    num_ctx: Option<u64>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u64>,
    seed: Option<u64>,
    stop: Option<Vec<String>>,
}

pub struct OllamaWorker {
    url: String,
    default_model: String,
    default_ctx: u64,
}

impl Default for OllamaWorker {
    fn default() -> Self {
        Self {
            url: std::env::var("OLLAMA_WORKER_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:11434".into()),
            default_model: std::env::var("OLLAMA_WORKER_MODEL")
                .unwrap_or_else(|_| "smollm:1.7b".into()),
            default_ctx: std::env::var("OLLAMA_WORKER_CTX")
                .ok().and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(8192),
        }
    }
}

#[async_trait]
impl WorkerBee for OllamaWorker {
    fn ephemeral(&self) -> bool { false }

    fn propensity(&self) -> Propensity {
        // Ollama API is stateless per call — every request carries full history.
        // The cell lives for one streaming response; humd can reuse it
        // for follow-up turns in the same sid.
        Propensity::StatefulSession
    }

    async fn raise(&self, egg: Egg) -> Result<Cell> {
        let model = if egg.model_id.is_empty() {
            self.default_model.clone()
        } else {
            egg.model_id.clone()
        };

        info!(model = %model, cwd = %egg.cwd, "ollama-worker.raise");
        let client = reqwest::Client::new();
        let url = self.url.clone();
        let default_ctx = self.default_ctx;

        let (tx_feed, mut rx_feed) = tokio::sync::mpsc::channel::<String>(64);
        let (tx_mmm, rx_mmm) = tokio::sync::mpsc::channel::<Value>(256);
        let silence = tokio_util::sync::CancellationToken::new();
        let silence_clone = silence.clone();

        let tx_mmm_wire = tx_mmm.clone();
        tokio::spawn(async move {
            let prompt = match rx_feed.recv().await {
                Some(p) => p,
                None => {
                    warn!("ollama-worker: feed closed before first prompt");
                    return;
                }
            };

            let mut msgs = Vec::new();
            if let Some(sp) = &egg.system_prompt {
                msgs.push(OllamaMessage {
                    role: "system".to_string(),
                    content: sp.clone(),
                });
            }
            msgs.push(OllamaMessage {
                role: "user".to_string(),
                content: prompt,
            });

            let request = OllamaRequest {
                model: model.clone(),
                messages: msgs,
                stream: true,
                options: Some(OllamaOptions {
                    num_ctx: Some(default_ctx),
                    temperature: None,
                    top_p: None,
                    top_k: None,
                    seed: None,
                    stop: None,
                }),
            };

            let req_body = serde_json::to_string(&request).unwrap();
            let req = client
                .post(format!("{}/api/chat", url))
                .header("Content-Type", "application/json")
                .body(req_body);

            match req.send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        let err = json!({
                            "chi": "error",
                            "code": "ollama_api_error",
                            "message": format!("Ollama returned {status}: {body}"),
                        });
                        let _ = tx_mmm_wire.send(err).await;
                        return;
                    }

                    let lines = resp.text().await.unwrap_or_default();

                    for line in lines.lines() {
                        if line.trim().is_empty() { continue; }
                        if silence_clone.is_cancelled() { break; }

                        match serde_json::from_str::<Value>(line) {
                            Ok(chunk) => {
                                let done = chunk.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
                                let content = chunk.get("message")
                                    .and_then(|m| m.get("content"))
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("");

                                if !content.is_empty() {
                                    let delta = json!({
                                        "chi": "chunk",
                                        "chunkType": "text_delta",
                                        "delta": content,
                                    });
                                    let _ = tx_mmm_wire.send(delta).await;
                                }

                                if done {
                                    let eval_count = chunk.get("eval_count")
                                        .and_then(|c| c.as_u64())
                                        .unwrap_or(0);
                                    let prompt_eval_count = chunk.get("prompt_eval_count")
                                        .and_then(|c| c.as_u64())
                                        .unwrap_or(0);

                                    let finish = json!({
                                        "chi": "finish",
                                        "finishReason": "stop",
                                        "usage": {
                                            "input_tokens": prompt_eval_count,
                                            "output_tokens": eval_count,
                                        },
                                    });
                                    let _ = tx_mmm_wire.send(finish).await;
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!(err = %e, line = line.chars().take(200).collect::<String>(), "ollama-worker.parse");
                            }
                        }
                    }
                }
                Err(e) => {
                    let err = json!({
                        "chi": "error",
                        "code": "ollama_connection_error",
                        "message": format!("Failed to reach Ollama at {url}: {e}"),
                    });
                    let _ = tx_mmm_wire.send(err).await;
                }
            }
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
