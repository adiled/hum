//! `serve_worker` — wraps a WorkerBee impl into a standalone process
//! that handshakes with humd via thrum and routes prompts over the wire.
//!
//! Mirrors the forager-bee client pattern: the worker process owns its own
//! lifecycle, humd is just a router. The wire contract:
//!
//! - **Hello**: announce as `bee:["worker"]`, advertise `models` +
//!   `propensity`. humd registers `{model_id → client_id}` mappings.
//! - **Prompt in**: humd forwards `chi:"prompt"` tones whose `modelId`
//!   matches one of the worker's advertised models. The worker calls
//!   `WorkerBee::spawn(spec)`, then murmurs the prompt text on the
//!   cell's stdin.
//! - **Chunks out**: each event from `Cell.events` becomes a
//!   `chi:"chunk"` tone tagged with `chunkType` + the original sid.
//! - **Cancel**: `chi:"cancel"` triggers `Cell.kill()` for the sid.
//! - **Tool result**: `chi:"tool-result"` feeds into the cell stdin
//!   via the worker's tool-result encoder (currently
//!   `nest::encode_tool_result`).
//!
//! Reconnect is built in — humd restarts don't strand workers; they
//! re-handshake.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use ensemble::HidPrefix;
use mcp::protocol::ToolDef;
use nest::{encode_cancel, encode_prompt, encode_tool_result, Listener, SpawnSpec, WorkerBee};
use tokio::sync::mpsc;

use crate::identity::load_or_mint_bee_key;
use crate::mcp_bridge::{spawn_local_mcp, McpBridge};

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
/// `model_id → client_id`) and observability (downstream foragers can
/// reason about propensity without inferring).
#[derive(Debug, Clone)]
pub struct HiveAdvert {
    /// Hive name, e.g. "claude-cli", "ollama". The bee carries this in
    /// hello as the kind it's commissioned by. Used as the broadcast
    /// tag for sigil routing.
    pub hive: String,
    /// Crate version string (cargo or otherwise). Free-form.
    pub version: String,
    /// Model ids this worker can serve. humd's prompt arm looks here
    /// for `modelId → worker` routing.
    pub models: Vec<String>,
    /// Optional source URL the mesh can use to discover the worker's
    /// repo. Carried verbatim into the gossiped manifest.
    pub source: Option<String>,
}

/// Run the worker service loop. Blocks until shutdown.
pub async fn serve_worker<W: WorkerBee + 'static>(worker: Arc<W>, advert: HiveAdvert) -> Result<()> {
    let path = default_socket_path();
    loop {
        match dial_and_serve(&path, worker.clone(), &advert).await {
            Ok(()) => {
                trace!("serve_worker: clean exit, reconnecting");
            }
            Err(e) => {
                warn!(err = %e, "serve_worker: connection failed, retrying");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn dial_and_serve<W: WorkerBee + 'static>(
    path: &Path,
    worker: Arc<W>,
    advert: &HiveAdvert,
) -> Result<()> {
    info!(socket = %path.display(), hive = %advert.hive, "worker.connecting");
    let stream = UnixStream::connect(path).await
        .with_context(|| format!("connect to thrum at {}", path.display()))?;
    let (read_half, write_half) = stream.into_split();
    let write_half = Arc::new(Mutex::new(write_half));

    // Load (or mint) the persistent worker-bee identity. The wbee_
    // hid survives reconnect + restart; humd indexes manifests by it
    // so the worker stays the "same" bee across thrum connections.
    let bee_key = load_or_mint_bee_key(&advert.hive, HidPrefix::Wbee)
        .with_context(|| format!("load/mint wbee key for hive {}", advert.hive))?;

    // Hello — register as `bee:["worker"]`. humd reads:
    //   hid, bee, hive (kind), models, propensity, version,
    //   protoVersion, source, chis.
    let propensity_str = match worker.propensity() {
        nest::Propensity::StatefulSession => "stateful_session",
        nest::Propensity::StatelessPerCall => "stateless_per_call",
        nest::Propensity::EphemeralPerCall => "ephemeral_per_call",
    };
    let hello = json!({
        "chi": "hello",
        "bee": ["worker"],
        "hid": bee_key.hid.to_hex(),
        "from": bee_key.hid.to_hex(),
        "hive": &advert.hive,
        "version": &advert.version,
        "protoVersion": thrum_core::THRUM_VERSION,
        "models": &advert.models,
        "propensity": { "statefulness": propensity_str, "wire": &advert.hive },
        "chis": ["hello", "prompt", "cancel", "tool-result", "chunk", "finish", "error", "tool-call"],
        "source": advert.source.clone().unwrap_or_default(),
    });
    write_half.lock().await.write_all(format!("{}\n", hello).as_bytes()).await?;
    info!(hive = %advert.hive, hid = %bee_key.hid.short(), models = ?advert.models, "worker.hello.sent");

    // Worker-local MCP bridge. The compute (e.g. claude) dials it
    // for tools/list + tools/call; the bridge ships chi:"tool-call"
    // over thrum and resolves the pending HTTP response when the
    // matching chi:"tool-result" arrives.
    let write_for_bridge = write_half.clone();
    let bridge = McpBridge::new(Arc::new(move |tone: Value| {
        let write_half = write_for_bridge.clone();
        tokio::spawn(async move {
            let line = format!("{}\n", tone);
            if let Err(e) = write_half.lock().await.write_all(line.as_bytes()).await {
                warn!(err = %e, "mcp.bridge.tool-call.write.failed");
            }
        });
    }));
    let mcp_addr = spawn_local_mcp(bridge.clone()).await
        .context("spawn local mcp bridge")?;
    let mcp_url = format!("http://{}", mcp_addr);
    info!(%mcp_url, "worker.mcp.bridge.up");

    // Per-sid cell handles + a kill-fn registry so chi:"cancel" can
    // reach the right child.
    let cells: Arc<Mutex<HashMap<String, CellBundle>>> = Arc::new(Mutex::new(HashMap::new()));

    let mut reader = BufReader::new(read_half).lines();
    while let Some(line) = reader.next_line().await? {
        if line.is_empty() { continue; }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => { trace!(err = %e, "worker.parse.skip"); continue; }
        };
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
        let sid = tone.get("sid").and_then(Value::as_str).map(str::to_string).unwrap_or_default();

        match chi {
            "prompt" => {
                // Update the MCP bridge's catalogue from the forager
                // tools humd merged + any tools the asker shipped.
                let provided: Vec<String> = tone.get("provided")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                let forager_tools: Vec<ToolDef> = tone.get("foragerTools")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(parse_tool_def).collect())
                    .unwrap_or_default();
                let nestler_tools: Vec<ToolDef> = tone.get("tools")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(parse_tool_def).collect())
                    .unwrap_or_default();
                if !forager_tools.is_empty() || !nestler_tools.is_empty() {
                    bridge.set_catalogue(&sid, forager_tools, nestler_tools, &provided);
                }
                let worker = worker.clone();
                let write_half = write_half.clone();
                let cells = cells.clone();
                let hive = advert.hive.clone();
                let mcp_url = mcp_url.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_prompt(worker, write_half, cells, hive, mcp_url, tone).await {
                        warn!(err = %e, "worker.prompt.handle.failed");
                    }
                });
            }
            "cancel" => {
                if !sid.is_empty() {
                    let r = cells.lock().await;
                    if let Some(bundle) = r.get(&sid) {
                        if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                            let _ = bundle.stdin.send(encode_cancel(rid)).await;
                        }
                        (bundle.kill)();
                    }
                }
            }
            "tool-result" => {
                let call_id = tone.get("callId").and_then(Value::as_str).map(str::to_string);
                // First try the worker MCP bridge — humfs_* tools
                // route there. If callId isn't pending in the bridge,
                // it's a nestler-native tool-result for a call the
                // model made outside our MCP catalogue; forward via
                // stdin so claude consumes it.
                let resolved_by_bridge = call_id.as_deref()
                    .map(|cid| bridge.resolve(cid, tone.clone()))
                    .unwrap_or(false);
                if !resolved_by_bridge && !sid.is_empty() {
                    let r = cells.lock().await;
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
                trace!(chi = other, "worker.unknown.chi");
            }
        }
    }
    Ok(())
}

struct CellBundle {
    stdin: mpsc::Sender<String>,
    kill: Arc<dyn Fn() + Send + Sync>,
    finish_sent: Arc<AtomicBool>,
    tool_use_blocks: Arc<Mutex<std::collections::BTreeSet<i64>>>,
    last_touched: Arc<AtomicU64>,
}

/// Build a `ToolDef` from a wire tone entry. MCP standard field is
/// `inputSchema`; tolerate legacy `parameters` (some hum-side
/// shims still emit it). Drop entries with no name OR no usable
/// schema — claude's mcp client rejects the entire tools/list if
/// any entry has `inputSchema: null`, so one bad apple kills all
/// tools for the session.
fn parse_tool_def(v: &Value) -> Option<ToolDef> {
    let name = v.get("name").and_then(Value::as_str)?.to_string();
    let description = v.get("description").and_then(Value::as_str).unwrap_or("").to_string();
    let schema = v.get("inputSchema")
        .or_else(|| v.get("parameters"))
        .cloned();
    let schema = match schema {
        Some(s) if s.is_object() => s,
        _ => serde_json::json!({ "type": "object", "properties": {} }),
    };
    Some(ToolDef { name, description, input_schema: schema })
}

/// Max quiet window before a warm cell gets reaped.
const IDLE_TIMEOUT_MS: u64 = 300_000;
/// Per-worker cap on concurrent claude processes. LRU eviction
/// when full.
const MAX_CELLS: usize = 8;

async fn handle_prompt<W: WorkerBee + 'static>(
    worker: Arc<W>,
    write_half: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    cells: Arc<Mutex<HashMap<String, CellBundle>>>,
    hive: String,
    mcp_url: String,
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
    let resume = tone.get("resume").and_then(Value::as_str).map(str::to_string);

    // claude `-p --input-format stream-json` reads newline-delimited
    // user messages until stdin EOF, emitting a `result` event per
    // turn. Reuse the warm cell for the sid; per-turn state lives in
    // finish_sent + tool_use_blocks and must reset before re-entry.
    {
        let g = cells.lock().await;
        if let Some(bundle) = g.get(&sid) {
            bundle.finish_sent.store(false, Ordering::SeqCst);
            bundle.tool_use_blocks.lock().await.clear();
            bundle.last_touched.store(now_ms(), Ordering::SeqCst);
            let send = bundle.stdin.clone();
            drop(g);
            send.send(encode_prompt(&content)).await
                .map_err(|e| anyhow::anyhow!("stdin closed on reused cell: {e}"))?;
            trace!(sid = %sid, "worker.cell.reused");
            return Ok(());
        }
    }

    // No warm cell — evict LRU if at cap, then spawn fresh.
    {
        let mut g = cells.lock().await;
        if g.len() >= MAX_CELLS {
            let evict_sid = g.iter()
                .min_by_key(|(_, b)| b.last_touched.load(Ordering::SeqCst))
                .map(|(k, _)| k.clone());
            if let Some(esid) = evict_sid {
                if let Some(bundle) = g.remove(&esid) {
                    warn!(evicted_sid = %esid, "worker.cell.evict.lru");
                    (bundle.kill)();
                }
            }
        }
    }

    let mut spec = SpawnSpec::new(sid.clone(), model.clone(), cwd);
    spec.system_prompt = system_prompt;
    spec.mcp_url = Some(mcp_url);
    spec.resume_id = resume;
    if let Some(arr) = tone.get("allowedTools").and_then(Value::as_array) {
        spec.allowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if let Some(arr) = tone.get("disallowedTools").and_then(Value::as_array) {
        spec.disallowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }

    let cell = worker.spawn(spec).await?;
    let stdin = cell.stdin.clone();
    let events = cell.events.clone();
    let kill = cell.kill.clone();
    let finish_sent = Arc::new(AtomicBool::new(false));
    let tool_use_blocks = Arc::new(Mutex::new(std::collections::BTreeSet::new()));
    let last_touched = Arc::new(AtomicU64::new(now_ms()));

    let listener = Arc::new(WireListener {
        sid: sid.clone(),
        hive,
        write_half: write_half.clone(),
        finish_sent: finish_sent.clone(),
        tool_use_blocks: tool_use_blocks.clone(),
        last_touched: last_touched.clone(),
    });

    // Cell-lifetime dispatch: each stream-json event flows through
    // the listener until the child exits.
    let listener_clone = listener.clone();
    let sid_for_loop = sid.clone();
    let events_for_loop = events.clone();
    let dispatch = tokio::spawn(async move {
        let mut guard = events_for_loop.lock().await;
        while let Some(value) = guard.recv().await {
            listener_clone.forward_raw(value).await;
        }
        trace!(sid = %sid_for_loop, "worker.dispatch.exit");
    });

    // Idle reaper — kills the cell if last_touched stays below the
    // IDLE_TIMEOUT_MS threshold.
    let cells_for_idle = cells.clone();
    let kill_for_idle = kill.clone();
    let sid_for_idle = sid.clone();
    let last_for_idle = last_touched.clone();
    let idle_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let last = last_for_idle.load(Ordering::SeqCst);
            let age = now_ms().saturating_sub(last);
            if age >= IDLE_TIMEOUT_MS {
                let mut g = cells_for_idle.lock().await;
                if g.remove(&sid_for_idle).is_some() {
                    warn!(sid = %sid_for_idle, age_ms = age, "worker.cell.idle.kill");
                    (kill_for_idle)();
                }
                return;
            }
        }
    });

    cells.lock().await.insert(sid.clone(), CellBundle {
        stdin: stdin.clone(),
        kill: kill.clone(),
        finish_sent: finish_sent.clone(),
        tool_use_blocks: tool_use_blocks.clone(),
        last_touched: last_touched.clone(),
    });

    stdin.send(encode_prompt(&content)).await
        .map_err(|e| anyhow::anyhow!("stdin closed: {e}"))?;
    trace!(sid = %sid, "worker.cell.spawned");

    // On child exit: drain dispatch, emit a finish if the listener
    // never saw `result`, and drop the cells entry.
    let cells_for_cleanup = cells.clone();
    let write_for_cleanup = write_half.clone();
    let sid_for_cleanup = sid.clone();
    let finish_for_cleanup = finish_sent.clone();
    tokio::spawn(async move {
        let exit_code = cell.exited.await.unwrap_or(1);
        let _ = dispatch.await;
        idle_task.abort();
        if !finish_for_cleanup.load(Ordering::SeqCst) {
            let finish = json!({
                "chi": "finish",
                "sid": sid_for_cleanup,
                "finishReason": if exit_code == 0 { "stop" } else { "error" },
                "exitCode": exit_code,
            });
            let line = format!("{}\n", finish);
            let _ = write_for_cleanup.lock().await.write_all(line.as_bytes()).await;
        }
        cells_for_cleanup.lock().await.remove(&sid_for_cleanup);
        trace!(sid = %sid_for_cleanup, exit_code, "worker.cell.exit");
    });

    Ok(())
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Translates raw claude-stream-json events into chi:"chunk" tones.
/// Lives in the helper so each worker crate doesn't reinvent it.
struct WireListener {
    sid: String,
    hive: String,
    write_half: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    /// True once `result` was forwarded as chi:"finish" for the
    /// current turn. handle_prompt resets it per-turn; the cell's
    /// cleanup checks it to avoid a redundant crash-path finish.
    finish_sent: Arc<AtomicBool>,
    /// Block indices the model emitted as `tool_use`. tool_input_* /
    /// content_block_stop for these indices are suppressed since
    /// the worker MCP bridge resolves the call inline and the
    /// canonical surface is chi:"chunk" chunkType="tool_executed".
    tool_use_blocks: Arc<Mutex<std::collections::BTreeSet<i64>>>,
    /// Updated on every stream-json event. The idle reaper compares
    /// against this to distinguish a quiet cell from a stalled one.
    last_touched: Arc<AtomicU64>,
}

impl WireListener {
    async fn send(&self, tone: Value) {
        let line = format!("{}\n", tone);
        let _ = self.write_half.lock().await.write_all(line.as_bytes()).await;
    }

    async fn forward_raw(&self, value: Value) {
        self.last_touched.store(now_ms(), Ordering::SeqCst);
        // claude emits stream-json events. The relevant chunk events
        // arrive wrapped as `{"type":"stream_event","event":{...inner...}}`;
        // unwrap to inspect the inner type. Mirrors the dispatch
        // humd used to do in-process (nest::pool::dispatch_loop).
        let mut msg = value;
        if msg.get("type").and_then(Value::as_str) == Some("stream_event") {
            if let Some(inner) = msg.get("event").cloned() {
                msg = inner;
            }
        }
        let typ = msg.get("type").and_then(Value::as_str).unwrap_or("").to_string();

        match typ.as_str() {
            "system" if msg.get("subtype").and_then(Value::as_str) == Some("init") => {
                // Session readiness signal. Carries claude's session_id
                // and model so consumers can attach.
                let mut body = json!({
                    "chi": "session-ready",
                    "sid": self.sid,
                });
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("nestId".into(), msg.get("session_id").cloned().unwrap_or(Value::Null));
                    obj.insert("model".into(), msg.get("model").cloned().unwrap_or(Value::Null));
                    obj.insert("tools".into(), msg.get("tools").cloned().unwrap_or(json!([])));
                }
                self.send(body).await;
            }
            "content_block_start" => {
                let idx = msg.get("index").cloned().unwrap_or(Value::Null);
                let block = msg.get("content_block").cloned().unwrap_or(json!({}));
                let bt = block.get("type").and_then(Value::as_str).unwrap_or("");
                match bt {
                    "text" => self.chunk("text_start", json!({"id": idx})).await,
                    "thinking" => self.chunk("reasoning_start", json!({"id": idx})).await,
                    "tool_use" => {
                        // Track for downstream suppression — bridge
                        // emits the canonical chi:"chunk" tool_executed
                        // once resolved.
                        if let Some(i) = idx.as_i64() {
                            self.tool_use_blocks.lock().await.insert(i);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = msg.get("delta").cloned().unwrap_or(json!({}));
                let idx = msg.get("index").and_then(Value::as_i64);
                let is_tool_block = match idx {
                    Some(i) => self.tool_use_blocks.lock().await.contains(&i),
                    None => false,
                };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "thinking_delta" => self.chunk("reasoning_delta", json!({"delta": delta.get("thinking")})).await,
                    "text_delta" => self.chunk("text_delta", json!({"delta": delta.get("text")})).await,
                    "input_json_delta" if !is_tool_block => {
                        self.chunk("tool_input_delta", json!({"partialJson": delta.get("partial_json")})).await
                    }
                    _ => {} // tool-block input_json_delta: suppressed (bridge ships chunk:tool_executed instead)
                }
            }
            "content_block_stop" => {
                let idx = msg.get("index").and_then(Value::as_i64);
                let is_tool_block = match idx {
                    Some(i) => self.tool_use_blocks.lock().await.remove(&i),
                    None => false,
                };
                if !is_tool_block {
                    self.chunk("content_block_stop", json!({"blockIdx": msg.get("index")})).await;
                }
            }
            "result" => {
                // Claude's wilt event — terminal of one prompt cycle.
                // Forward as chi:"finish" so the nestler receives the
                // canonical finish signal + usage.
                let finish_reason = msg.get("subtype")
                    .and_then(Value::as_str)
                    .unwrap_or("stop")
                    .to_string();
                let usage = msg.get("usage").cloned().unwrap_or(Value::Null);
                let mut body = json!({
                    "chi": "finish",
                    "sid": self.sid,
                    "finishReason": finish_reason,
                });
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("usage".into(), usage);
                }
                self.send(body).await;
                self.finish_sent.store(true, Ordering::SeqCst);
            }
            _ => {
                // Structural / unrecognized — drop. Mirrors humd's
                // historical filter; richer chi values can plug in here
                // (perf-mark, drone, etc.) when workers grow them.
                let _ = &self.hive;
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
    async fn on_cell(&self, _nest_id: &str, _model: &str, _tools: Vec<String>) {}
    async fn on_wilt(&self, _finish_reason: &str, _usage: Option<Value>, _provider_meta: Value) {}
    async fn on_thorn(&self, _wound: &str) {}
}
