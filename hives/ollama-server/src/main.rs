//! `ollama-server` — Ollama-compatible HTTP surface for hum.
//!
//! Three endpoints, exact Ollama shape:
//!   POST /api/chat      — multi-message; NDJSON streaming response
//!   POST /api/generate  — single prompt;  NDJSON streaming response
//!   GET  /api/tags      — list of available models (synthesized)
//!
//! Ollama's streaming response is already line-delimited JSON, which
//! matches thrum's frame format — every `chi:"chunk"` tone maps to
//! one output line. No SSE re-framing.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use hum_thrum::{Bee, Event, Options};
use thrum_core::Chi;
use tokio::sync::mpsc;
use tracing::info;

const HIVE_NAME: &str = "ollama-server";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    models: Option<Vec<String>>,
}

fn config_file_path() -> PathBuf {
    hum_paths::hive_config(HIVE_NAME)
}

fn read_file_config() -> FileConfig {
    let path = config_file_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return FileConfig::default(),
    };
    serde_json::from_str::<FileConfig>(&raw).unwrap_or_default()
}

#[derive(Debug, Clone)]
struct Config {
    listen: String,
    models: Vec<String>,
    bind: Option<SocketAddr>,
}

impl Config {
    fn load() -> Self {
        let file = read_file_config();
        let port: u16 = match std::env::var("OLLAMA_SERVER_PORT") {
            Ok(s) => s.parse().unwrap_or(11434),
            Err(_) => file.port.unwrap_or(11434),
        };
        let host = std::env::var("OLLAMA_SERVER_HOST")
            .ok()
            .or(file.host)
            .unwrap_or_else(|| "127.0.0.1".into());
        let models: Vec<String> = match std::env::var("OLLAMA_SERVER_MODELS") {
            Ok(raw) => raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            Err(_) => file.models.unwrap_or_else(|| {
                vec![
                    "claude-sonnet-4".into(),
                    "claude-haiku-4.5".into(),
                    "claude-opus-4.7".into(),
                ]
            }),
        };
        Self {
            listen: format!("{host}:{port}"),
            models,
            bind: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ToolFunction {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    kind: String,
    function: ToolFunction,
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    #[serde(default)]
    stream: Option<bool>,
    #[serde(default)]
    tools: Option<Vec<OllamaTool>>,
}

#[derive(Debug, Deserialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    stream: Option<bool>,
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn messages_to_prompt(messages: &[OllamaMessage]) -> (Option<String>, String) {
    let mut system_pieces = Vec::new();
    let mut user_prompt = String::new();
    for msg in messages {
        match msg.role.as_str() {
            "system" => system_pieces.push(msg.content.clone()),
            "user" => user_prompt = msg.content.clone(),
            _ => {}
        }
    }
    let system = if system_pieces.is_empty() {
        None
    } else {
        Some(system_pieces.join("\n\n"))
    };
    (system, user_prompt)
}

fn tools_to_thrum(tools: Option<Vec<OllamaTool>>) -> Option<Vec<Value>> {
    let tools = tools?;
    if tools.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for t in tools {
        if t.kind != "function" {
            continue;
        }
        let mut obj = serde_json::Map::new();
        obj.insert("name".into(), Value::String(t.function.name));
        if let Some(d) = t.function.description {
            obj.insert("description".into(), Value::String(d));
        }
        if let Some(p) = t.function.parameters {
            obj.insert("parameters".into(), p);
        }
        out.push(Value::Object(obj));
    }
    if out.is_empty() { None } else { Some(out) }
}

async fn open_prompt(
    cfg: &Config,
    sid: &str,
    text: &str,
    model: &str,
    system: Option<&str>,
    tools: Option<&[Value]>,
) -> Result<(mpsc::Receiver<Value>, tokio::task::JoinHandle<()>)> {
    let mut hello = serde_json::Map::new();
    hello.insert("roles".into(), json!(["forager"]));
    hello.insert("version".into(), Value::String(NESTLING_VERSION.into()));
    hello.insert(
        "propensity".into(),
        json!({ "statefulness": "convention-stateful", "richness": "medium", "wire": "ollama/api" }),
    );
    hello.insert(
        "chis".into(),
        json!(["hello", "prompt", "cancel", "chunk", "finish", "error", "tool-call", "tool-result"]),
    );
    hello.insert(
        "source".into(),
        Value::String("https://github.com/adiled/hum/tree/main/hives/ollama-server".into()),
    );
    if let Some(addr) = cfg.bind {
        hello.insert(
            "bind".into(),
            json!({
                "host": addr.ip().to_string(),
                "port": addr.port(),
                "scheme": "http",
            }),
        );
    }
    let mut bee = Bee::link(Options::new(HIVE_NAME, "main").hello(Value::Object(hello)))
        .await
        .context("link the ollama-server bee")?;

    let mut prompt = serde_json::Map::new();
    prompt.insert("chi".into(), json!(Chi::Prompt));
    prompt.insert("rid".into(), Value::String(format!("prompt-{sid}")));
    prompt.insert("sid".into(), Value::String(sid.to_string()));
    prompt.insert("text".into(), Value::String(text.to_string()));
    prompt.insert("modelId".into(), Value::String(model.to_string()));
    if let Some(sys) = system {
        prompt.insert("systemPrompt".into(), Value::String(sys.to_string()));
    }
    if let Some(t) = tools {
        prompt.insert("tools".into(), Value::Array(t.to_vec()));
    }
    bee.send(Value::Object(prompt));

    let (tx, rx) = mpsc::channel::<Value>(128);
    let sid_owned = sid.to_string();
    let pump = tokio::spawn(async move {
        while let Some(event) = bee.next().await {
            let tone = match event {
                Event::Tone(tone) => tone,
                Event::Lost => json!({"chi": "error", "sid": sid_owned, "message": "humd went away mid-turn"}),
                Event::Refused(why) => json!({"chi": "error", "sid": sid_owned, "message": why}),
                Event::Nestled | Event::Advisory(_) => continue,
            };
            if tone.get("sid").and_then(Value::as_str) != Some(sid_owned.as_str()) {
                continue;
            }
            let ends = tone.get("chi").and_then(Value::as_str) == Some("error");
            if tx.send(tone).await.is_err() || ends {
                break;
            }
        }
        bee.close();
    });
    Ok((rx, pump))
}

async fn chat(
    State(cfg): State<Arc<Config>>,
    Json(req): Json<ChatRequest>,
) -> Response {
    let stream = req.stream.unwrap_or(true);
    let (system, user) = messages_to_prompt(&req.messages);
    let tools = tools_to_thrum(req.tools);
    let sid = hum_identity::HumId::mint().to_string();

    let (rx, _pump) = match open_prompt(
        &cfg, &sid, &user, &req.model, system.as_deref(), tools.as_deref(),
    ).await {
        Ok(p) => p,
        Err(e) => return error_resp(StatusCode::BAD_GATEWAY, &e.to_string()),
    };
    if stream {
        chat_stream(req.model, sid, rx).await
    } else {
        chat_collect(req.model, sid, rx).await
    }
}

async fn generate(
    State(cfg): State<Arc<Config>>,
    Json(req): Json<GenerateRequest>,
) -> Response {
    let stream = req.stream.unwrap_or(true);
    let sid = hum_identity::HumId::mint().to_string();
    let (rx, _pump) = match open_prompt(
        &cfg, &sid, &req.prompt, &req.model, req.system.as_deref(), None,
    ).await {
        Ok(p) => p,
        Err(e) => return error_resp(StatusCode::BAD_GATEWAY, &e.to_string()),
    };
    if stream {
        generate_stream(req.model, sid, rx).await
    } else {
        generate_collect(req.model, sid, rx).await
    }
}

fn extract_text_part(tone: &Value) -> Option<String> {
    let part = tone.get("part")?;
    if part.get("type")?.as_str()? != "text" {
        return None;
    }
    Some(part.get("text")?.as_str()?.to_string())
}

fn extract_tool_use(tone: &Value) -> Option<(String, Value)> {
    let part = tone.get("part")?;
    if part.get("type")?.as_str()? != "tool_use" {
        return None;
    }
    let tc = part.get("toolCall")?;
    Some((
        tc.get("name")?.as_str().unwrap_or("").to_string(),
        tc.get("input").cloned().unwrap_or(json!({})),
    ))
}

async fn chat_stream(model: String, _sid: String, mut rx: mpsc::Receiver<Value>) -> Response {
    let (tx, body_rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(128);
    tokio::spawn(async move {
        while let Some(tone) = rx.recv().await {
            let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
            match chi {
                "chunk" => {
                    if let Some(text) = extract_text_part(&tone) {
                        let line = json!({
                            "model": model,
                            "created_at": now_iso(),
                            "message": { "role": "assistant", "content": text },
                            "done": false,
                        });
                        let _ = tx.send(Ok(format!("{line}\n").into())).await;
                    } else if let Some((name, args)) = extract_tool_use(&tone) {
                        let line = json!({
                            "model": model,
                            "created_at": now_iso(),
                            "message": {
                                "role": "assistant",
                                "content": "",
                                "tool_calls": [{ "function": { "name": name, "arguments": args } }],
                            },
                            "done": false,
                        });
                        let _ = tx.send(Ok(format!("{line}\n").into())).await;
                    }
                }
                "finish" => {
                    let finish_reason = tone.get("finishReason").and_then(Value::as_str).unwrap_or("stop");
                    let mut out = serde_json::Map::new();
                    out.insert("model".into(), Value::String(model.clone()));
                    out.insert("created_at".into(), Value::String(now_iso()));
                    out.insert("message".into(), json!({ "role": "assistant", "content": "" }));
                    out.insert("done".into(), Value::Bool(true));
                    out.insert("done_reason".into(), Value::String(finish_reason.into()));
                    if let Some(usage) = tone.get("usage").cloned() {
                        if let Some(map) = usage.as_object() {
                            for (k, v) in map {
                                out.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    let _ = tx.send(Ok(format!("{}\n", Value::Object(out)).into())).await;
                    break;
                }
                "error" => {
                    let msg = tone.get("message").and_then(Value::as_str).unwrap_or("stream error");
                    let line = json!({ "error": msg, "done": true });
                    let _ = tx.send(Ok(format!("{line}\n").into())).await;
                    break;
                }
                _ => {}
            }
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(body_rx)))
        .unwrap()
}

async fn chat_collect(model: String, _sid: String, mut rx: mpsc::Receiver<Value>) -> Response {
    let mut buf = String::new();
    let mut finish_reason = String::from("stop");
    let mut usage: serde_json::Map<String, Value> = serde_json::Map::new();
    while let Some(tone) = rx.recv().await {
        match tone.get("chi").and_then(Value::as_str) {
            Some("chunk") => {
                if let Some(t) = extract_text_part(&tone) {
                    buf.push_str(&t);
                }
            }
            Some("finish") => {
                if let Some(r) = tone.get("finishReason").and_then(Value::as_str) {
                    finish_reason = r.into();
                }
                if let Some(u) = tone.get("usage").and_then(|v| v.as_object()) {
                    usage = u.clone();
                }
                break;
            }
            Some("error") => break,
            _ => {}
        }
    }
    let mut out = serde_json::Map::new();
    out.insert("model".into(), Value::String(model));
    out.insert("created_at".into(), Value::String(now_iso()));
    out.insert("message".into(), json!({ "role": "assistant", "content": buf }));
    out.insert("done".into(), Value::Bool(true));
    out.insert("done_reason".into(), Value::String(finish_reason));
    for (k, v) in usage {
        out.insert(k, v);
    }
    Json(Value::Object(out)).into_response()
}

async fn generate_stream(model: String, _sid: String, mut rx: mpsc::Receiver<Value>) -> Response {
    let (tx, body_rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(128);
    tokio::spawn(async move {
        while let Some(tone) = rx.recv().await {
            match tone.get("chi").and_then(Value::as_str) {
                Some("chunk") => {
                    if let Some(text) = extract_text_part(&tone) {
                        let line = json!({
                            "model": model,
                            "created_at": now_iso(),
                            "response": text,
                            "done": false,
                        });
                        let _ = tx.send(Ok(format!("{line}\n").into())).await;
                    }
                }
                Some("finish") => {
                    let finish_reason = tone.get("finishReason").and_then(Value::as_str).unwrap_or("stop");
                    let line = json!({
                        "model": model,
                        "created_at": now_iso(),
                        "response": "",
                        "done": true,
                        "done_reason": finish_reason,
                    });
                    let _ = tx.send(Ok(format!("{line}\n").into())).await;
                    break;
                }
                Some("error") => {
                    let msg = tone.get("message").and_then(Value::as_str).unwrap_or("stream error");
                    let line = json!({ "error": msg, "done": true });
                    let _ = tx.send(Ok(format!("{line}\n").into())).await;
                    break;
                }
                _ => {}
            }
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(body_rx)))
        .unwrap()
}

async fn generate_collect(model: String, _sid: String, mut rx: mpsc::Receiver<Value>) -> Response {
    let mut buf = String::new();
    let mut finish_reason = String::from("stop");
    while let Some(tone) = rx.recv().await {
        match tone.get("chi").and_then(Value::as_str) {
            Some("chunk") => {
                if let Some(t) = extract_text_part(&tone) {
                    buf.push_str(&t);
                }
            }
            Some("finish") => {
                if let Some(r) = tone.get("finishReason").and_then(Value::as_str) {
                    finish_reason = r.into();
                }
                break;
            }
            Some("error") => break,
            _ => {}
        }
    }
    Json(json!({
        "model": model,
        "created_at": now_iso(),
        "response": buf,
        "done": true,
        "done_reason": finish_reason,
    }))
    .into_response()
}

async fn tags(State(cfg): State<Arc<Config>>) -> Response {
    let models: Vec<Value> = cfg.models.iter().map(|name| {
        json!({
            "name": name,
            "model": name,
            "modified_at": now_iso(),
            "size": 0,
            "digest": "",
            "details": {
                "parent_model": "",
                "format": "hum",
                "family": "claude",
                "families": ["claude"],
                "parameter_size": "",
                "quantization_level": "",
            }
        })
    }).collect();
    Json(json!({ "models": models })).into_response()
}

async fn root() -> &'static str {
    "Ollama is running\n"
}

fn error_resp(code: StatusCode, msg: &str) -> Response {
    let body = Json(json!({ "error": msg })).into_response();
    (code, body).into_response()
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
    let mut cfg = Config::load();
    let listen = cfg.listen.clone();
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    let bound = listener.local_addr()?;
    cfg.bind = Some(bound);
    info!(listen = %listen, bound = %bound, "ollama-server.start");

    let cfg = Arc::new(cfg);
    let app = Router::new()
        .route("/", get(root))
        .route("/api/tags", get(tags))
        .route("/api/chat", post(chat))
        .route("/api/generate", post(generate))
        .with_state(cfg);

    axum::serve(listener, app).await?;
    Ok(())
}
