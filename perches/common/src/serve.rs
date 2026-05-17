//! `serve_perch` — wraps a Perch impl into a standalone process that
//! handshakes with humd via thrum and routes prompts over the wire.
//!
//! Mirrors the nestling client pattern: the perch process owns its own
//! lifecycle, humd is just a router. The wire contract:
//!
//! - **Hello**: announce as `role:"perch"`, advertise `models` +
//!   `propensity`. humd registers `{model_id → client_id}` mappings.
//! - **Prompt in**: humd forwards `chi:"prompt"` tones whose `modelId`
//!   matches one of the perch's advertised models. The perch calls
//!   `Perch::spawn(spec)`, then murmurs the prompt text on the roost's
//!   stdin.
//! - **Chunks out**: each event from `Roost.events` becomes a
//!   `chi:"chunk"` tone tagged with `chunkType` + the original sid.
//! - **Cancel**: `chi:"cancel"` triggers `Roost.kill()` for the sid.
//! - **Tool result**: `chi:"tool-result"` feeds into the roost stdin
//!   via the perch's tool-result encoder (currently
//!   `nest::encode_tool_result`).
//!
//! Reconnect is built in — humd restarts don't strand perches; they
//! re-handshake.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use nest::{encode_cancel, encode_prompt, encode_tool_result, Listener, Perch, SpawnSpec};

/// Resolve the canonical thrum socket path. Mirrors `thrumd::default_socket_path`.
fn default_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("HUM_THRUM_SOCK") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(format!("/tmp/hum-{}", unsafe_uid())));
    runtime.join("hum").join("thrum.sock")
}

fn unsafe_uid() -> u32 {
    std::process::Command::new("id").arg("-u").output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// What the host advertises on hello. Drives both routing (humd maps
/// `model_id → client_id`) and observability (downstream nestlings can
/// reason about propensity without inferring).
#[derive(Debug, Clone)]
pub struct PerchAdvert {
    /// Perch kind name, e.g. "claude-cli", "ollama". Used as the
    /// broadcast tag for sigil routing.
    pub kind: String,
    /// Crate version string (cargo or otherwise). Free-form.
    pub version: String,
    /// Model ids this perch can serve. humd's prompt arm looks here
    /// for `modelId → perch` routing.
    pub models: Vec<String>,
    /// Optional source URL the mesh can use to discover the perch's
    /// repo. Carried verbatim into the gossiped manifest.
    pub source: Option<String>,
}

/// Run the perch service loop. Blocks until shutdown.
pub async fn serve_perch<P: Perch + 'static>(perch: Arc<P>, advert: PerchAdvert) -> Result<()> {
    let path = default_socket_path();
    loop {
        match dial_and_serve(&path, perch.clone(), &advert).await {
            Ok(()) => {
                trace!("serve_perch: clean exit, reconnecting");
            }
            Err(e) => {
                warn!(err = %e, "serve_perch: connection failed, retrying");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn dial_and_serve<P: Perch + 'static>(
    path: &Path,
    perch: Arc<P>,
    advert: &PerchAdvert,
) -> Result<()> {
    info!(socket = %path.display(), kind = %advert.kind, "perch.connecting");
    let stream = UnixStream::connect(path).await
        .with_context(|| format!("connect to thrum at {}", path.display()))?;
    let (read_half, write_half) = stream.into_split();
    let write_half = Arc::new(Mutex::new(write_half));

    // Hello — register as `role:"perch"`. humd reads:
    //   role, nestling (= kind), models, propensity, version,
    //   protoVersion, source, chis.
    let propensity_str = match perch.propensity() {
        nest::Propensity::StatefulSession => "stateful_session",
        nest::Propensity::StatelessPerCall => "stateless_per_call",
        nest::Propensity::EphemeralPerCall => "ephemeral_per_call",
    };
    let hello = json!({
        "chi": "hello",
        "role": "perch",
        "from": &advert.kind,
        "nestling": &advert.kind,
        "version": &advert.version,
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": &advert.models,
        "propensity": { "statefulness": propensity_str, "wire": &advert.kind },
        "chis": ["hello", "prompt", "cancel", "tool-result", "chunk", "finish", "error", "tool-call"],
        "source": advert.source.clone().unwrap_or_default(),
    });
    write_half.lock().await.write_all(format!("{}\n", hello).as_bytes()).await?;
    info!(kind = %advert.kind, models = ?advert.models, "perch.hello.sent");

    // Per-sid roost handles + a kill-fn registry so chi:"cancel" can
    // reach the right child.
    let roosts: Arc<Mutex<HashMap<String, RoostBundle>>> = Arc::new(Mutex::new(HashMap::new()));

    let mut reader = BufReader::new(read_half).lines();
    while let Some(line) = reader.next_line().await? {
        if line.is_empty() { continue; }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => { trace!(err = %e, "perch.parse.skip"); continue; }
        };
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
        let sid = tone.get("sid").and_then(Value::as_str).map(str::to_string).unwrap_or_default();

        match chi {
            "prompt" => {
                let perch = perch.clone();
                let write_half = write_half.clone();
                let roosts = roosts.clone();
                let kind = advert.kind.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_prompt(perch, write_half, roosts, kind, tone).await {
                        warn!(err = %e, "perch.prompt.handle.failed");
                    }
                });
            }
            "cancel" => {
                if !sid.is_empty() {
                    let r = roosts.lock().await;
                    if let Some(bundle) = r.get(&sid) {
                        if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                            let _ = bundle.stdin.send(encode_cancel(rid)).await;
                        }
                        (bundle.kill)();
                    }
                }
            }
            "tool-result" => {
                if !sid.is_empty() {
                    let r = roosts.lock().await;
                    if let Some(bundle) = r.get(&sid) {
                        if let (Some(call_id), Some(result)) = (
                            tone.get("callId").and_then(Value::as_str),
                            tone.get("result").and_then(Value::as_str),
                        ) {
                            let _ = bundle.stdin.send(encode_tool_result(call_id, result)).await;
                        }
                    }
                }
            }
            "breath" | "echo" | "" => {
                // Wire keepalive / ack — nothing to do.
            }
            other => {
                trace!(chi = other, "perch.unknown.chi");
            }
        }
    }
    Ok(())
}

struct RoostBundle {
    stdin: mpsc::Sender<String>,
    kill: Arc<dyn Fn() + Send + Sync>,
}

async fn handle_prompt<P: Perch + 'static>(
    perch: Arc<P>,
    write_half: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    roosts: Arc<Mutex<HashMap<String, RoostBundle>>>,
    kind: String,
    tone: Value,
) -> Result<()> {
    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
    if sid.is_empty() { anyhow::bail!("prompt.no-sid"); }
    let model = tone.get("modelId").and_then(Value::as_str).unwrap_or("").to_string();
    let cwd = tone.get("cwd").and_then(Value::as_str).unwrap_or("/").to_string();
    let content = tone.get("content").and_then(Value::as_str)
        .or_else(|| tone.get("text").and_then(Value::as_str))
        .unwrap_or("").to_string();
    let system_prompt = tone.get("systemPrompt").and_then(Value::as_str).map(str::to_string);
    let mcp_url = tone.get("mcpUrl").and_then(Value::as_str).map(str::to_string);

    // Build SpawnSpec — only the common knobs; perch-specific extras
    // (sampling block, cli flags) ride along on the tone if the
    // perch wants them.
    let mut spec = SpawnSpec::new(sid.clone(), model.clone(), cwd);
    spec.system_prompt = system_prompt;
    spec.mcp_url = mcp_url;
    if let Some(arr) = tone.get("allowedTools").and_then(Value::as_array) {
        spec.allowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if let Some(arr) = tone.get("disallowedTools").and_then(Value::as_array) {
        spec.disallowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }

    let roost = perch.spawn(spec).await?;
    let stdin = roost.stdin.clone();
    let events = roost.events.clone();
    let kill = roost.kill.clone();

    roosts.lock().await.insert(sid.clone(), RoostBundle { stdin: stdin.clone(), kill: kill.clone() });

    // First user turn — murmur the prompt content as a single user
    // message. Multi-turn continuity arrives as subsequent chi:"prompt"
    // tones with the same sid (the perch's pool decides whether to
    // reuse — but in the serve_perch loop each prompt currently
    // spawns its own roost; pooling is an optimization for later).
    stdin.send(encode_prompt(&content)).await
        .map_err(|e| anyhow::anyhow!("stdin closed: {e}"))?;

    // Dispatch loop — Roost.events → chi:"chunk" tones over thrum.
    let listener = Arc::new(WireListener {
        sid: sid.clone(),
        kind,
        write_half: write_half.clone(),
    });

    let listener_clone = listener.clone();
    let sid_for_loop = sid.clone();
    let events_for_loop = events.clone();
    let dispatch = tokio::spawn(async move {
        let mut guard = events_for_loop.lock().await;
        while let Some(value) = guard.recv().await {
            let typ = value.get("type").and_then(Value::as_str).unwrap_or("");
            let _ = typ; // hum events are claude-stream-json shape; let
                        // listener decide. For minimal viable serve,
                        // we forward the raw value as a tool-able chunk.
            listener_clone.forward_raw(value).await;
        }
        trace!(sid = %sid_for_loop, "perch.dispatch.exit");
    });

    // Wait for exit, then emit finish + cleanup.
    let exit_code = roost.exited.await.unwrap_or(1);
    let _ = dispatch.abort();
    let finish = json!({
        "chi": "finish",
        "sid": sid,
        "finishReason": if exit_code == 0 { "stop" } else { "error" },
        "exitCode": exit_code,
    });
    let line = format!("{}\n", finish);
    let _ = write_half.lock().await.write_all(line.as_bytes()).await;
    roosts.lock().await.remove(&sid);
    Ok(())
}

/// Translates raw claude-stream-json events into chi:"chunk" tones.
/// Lives in the helper so each perch crate doesn't reinvent it.
struct WireListener {
    sid: String,
    kind: String,
    write_half: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
}

impl WireListener {
    async fn send(&self, tone: Value) {
        let line = format!("{}\n", tone);
        let _ = self.write_half.lock().await.write_all(line.as_bytes()).await;
    }

    async fn forward_raw(&self, value: Value) {
        // Stream-json events have the shape produced by the claude CLI.
        // Translate the common ones to chi:"chunk" payloads matching
        // what humd's NestListener historically emitted.
        let typ = value.get("type").and_then(Value::as_str).unwrap_or("");
        match typ {
            "content_block_start" => {
                let block = value.get("content_block").cloned().unwrap_or(json!({}));
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => self.chunk("text_start", json!({"id": value.get("index")})).await,
                    "thinking" => self.chunk("reasoning_start", json!({"id": value.get("index")})).await,
                    "tool_use" => {
                        self.chunk("tool_input_start", json!({
                            "toolCallId": block.get("id"),
                            "toolName": block.get("name"),
                        })).await;
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = value.get("delta").cloned().unwrap_or(json!({}));
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "thinking_delta" => self.chunk("reasoning_delta", json!({"delta": delta.get("thinking")})).await,
                    "text_delta" => self.chunk("text_delta", json!({"delta": delta.get("text")})).await,
                    "input_json_delta" => self.chunk("tool_input_delta", json!({"partialJson": delta.get("partial_json")})).await,
                    _ => {}
                }
            }
            "content_block_stop" => {
                self.chunk("content_block_stop", json!({"blockIdx": value.get("index")})).await;
            }
            _ => {
                // Unknown / structural — drop. humd's previous parser
                // is the reference for what to expand here.
                let _ = &self.kind;
            }
        }
    }

    async fn chunk(&self, chunk_type: &str, payload: Value) {
        let mut body = serde_json::Map::new();
        body.insert("chi".into(), Value::String("chunk".into()));
        body.insert("sid".into(), Value::String(self.sid.clone()));
        body.insert("chunkType".into(), Value::String(chunk_type.into()));
        if let Some(obj) = payload.as_object() {
            for (k, v) in obj { body.insert(k.clone(), v.clone()); }
        }
        self.send(Value::Object(body)).await;
    }
}

#[async_trait::async_trait]
impl Listener for WireListener {
    fn session_id(&self) -> &str { &self.sid }
    async fn on_petal(&self, _kind: &str, _payload: Value) {}
    async fn on_roost(&self, _nest_id: &str, _model: &str, _tools: Vec<String>) {}
    async fn on_wilt(&self, _finish_reason: &str, _usage: Option<Value>, _provider_meta: Value) {}
    async fn on_thorn(&self, _wound: &str) {}
}
