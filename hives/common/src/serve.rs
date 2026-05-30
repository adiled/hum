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

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use lru::LruCache;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{debug, info, trace, warn};

use ensemble::HidPrefix;
use mcp::protocol::ToolDef;
use nest::{encode_cancel, encode_prompt, encode_tool_result, Cell, SpawnSpec, WorkerBee};
use tokio::sync::mpsc;

use crate::identity::load_or_mint_bee_key;
use crate::mcp_bridge::{spawn_local_mcp, McpBridge};

fn default_socket_path() -> PathBuf {
    hum_paths::thrum_sock()
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
    // Connection cycling is normal: launchd/systemd start the worker and
    // humd in parallel, so the first dials lose the race until humd binds
    // the socket. That is not a warning. Only escalate to WARN once a bee
    // that has *never* connected keeps failing past a grace window (humd
    // genuinely down / wrong socket path), so a clean boot stays quiet.
    let mut ever_connected = false;
    let mut consecutive_fails = 0u32;
    loop {
        match dial_and_serve(&path, worker.clone(), &advert).await {
            Ok(()) => {
                ever_connected = true;
                consecutive_fails = 0;
                trace!("serve_worker: clean exit, reconnecting");
            }
            Err(e) => {
                consecutive_fails += 1;
                if ever_connected {
                    info!(err = %e, "serve_worker: thrum dropped, reconnecting");
                } else if consecutive_fails >= 8 {
                    // ~16s of never connecting — humd likely down or the
                    // socket path is wrong. Now it is worth a warning.
                    warn!(err = %e, attempts = consecutive_fails,
                        "serve_worker: still cannot reach humd — check it is running and HUM_THRUM_SOCK matches");
                } else {
                    debug!(err = %e, "serve_worker: waiting for humd socket");
                }
            }
        }
        let jitter = rand::random::<f32>() * 0.75;
        tokio::time::sleep(std::time::Duration::from_secs_f32(2.0 + jitter)).await;
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
    let cells: Arc<Mutex<LruCache<String, CellBundle>>> =
        Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(MAX_CELLS).unwrap())));

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
                    bridge.set_catalogue(forager_tools, nestler_tools, &provided);
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
                    let mut r = cells.lock().await;
                    if let Some(bundle) = r.get(&sid) {
                        if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                            let _ = bundle.stdin.send(encode_cancel(rid)).await;
                        }
                        bundle.cancel.cancel();
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
                    let mut r = cells.lock().await;
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
    cancel: tokio_util::sync::CancellationToken,
    finish_sent: Arc<AtomicBool>,
    tool_use_blocks: Arc<Mutex<std::collections::BTreeSet<i64>>>,
    last_touched: Arc<AtomicU64>,
}

impl Drop for CellBundle {
    fn drop(&mut self) { self.cancel.cancel(); }
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

/// Fixed namespace so a hum sid maps to a stable claude session UUID
/// (uuid5). Deterministic: the same sid always derives the same id, so
/// it survives worker restarts without persisting anything.
const HUM_SESSION_NS: uuid::Uuid = uuid::Uuid::from_bytes([
    0x68, 0x75, 0x6d, 0x2d, 0x73, 0x65, 0x73, 0x73, 0x69, 0x6f, 0x6e, 0x2d, 0x6e, 0x73, 0x00, 0x01,
]);

/// Derive the claude session id for a hum sid. claude's `--session-id`
/// requires a UUID, so we can't pass the sid verbatim; uuid5 maps it
/// deterministically.
fn sid_to_session(sid: &str) -> String {
    uuid::Uuid::new_v5(&HUM_SESSION_NS, sid.as_bytes()).to_string()
}

/// True if `v` is claude's terminal `result` event flagged `is_error`.
/// As the *first* event it means a pre-flight failure (bad/absent
/// session on `--resume`, id clash on `--session-id`) rather than a
/// mid-turn error, which arrives only after content.
fn is_preflight_error(v: &Value) -> bool {
    let m = if v.get("type").and_then(Value::as_str) == Some("stream_event") {
        v.get("event").unwrap_or(v)
    } else { v };
    m.get("type").and_then(Value::as_str) == Some("result")
        && m.get("is_error").and_then(Value::as_bool).unwrap_or(false)
}

/// Spawn one attempt: launch the cell, send the prompt, and wait for the
/// first stream-json event. Returns the cell + that event if it's live;
/// returns None (after killing the cell) if claude fails pre-flight
/// (first event is an `is_error` result) or produces nothing.
async fn attempt_spawn<W: WorkerBee + 'static>(
    worker: &Arc<W>,
    spec: SpawnSpec,
    content: &str,
) -> Option<(Cell, Value)> {
    let cell = worker.spawn(spec).await.ok()?;
    if cell.stdin.send(encode_prompt(content)).await.is_err() {
        cell.cancel.cancel();
        return None;
    }
    let first = {
        let mut ev = cell.events.lock().await;
        match tokio::time::timeout(std::time::Duration::from_secs(180), ev.recv()).await {
            Ok(Some(v)) => Some(v),
            _ => None,
        }
    };
    match first {
        Some(v) if is_preflight_error(&v) => { cell.cancel.cancel(); None }
        Some(v) => Some((cell, v)),
        None => { cell.cancel.cancel(); None }
    }
}

async fn handle_prompt<W: WorkerBee + 'static>(
    worker: Arc<W>,
    write_half: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    cells: Arc<Mutex<LruCache<String, CellBundle>>>,
    hive: String,
    mcp_url: String,
    tone: Value,
) -> Result<()> {
    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
    if sid.is_empty() { anyhow::bail!("prompt.no-sid"); }
    // Model: the prompt's modelId, else fall back to the first model
    // this worker advertises (CLAUDE_MODELS leads with the install
    // default), else opus 4.7. Without this an asker that omits modelId
    // would spawn `claude --model ""`.
    let model = tone.get("modelId").and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| std::env::var("CLAUDE_MODELS").ok()
            .and_then(|m| m.split(',').next().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "claude-opus-4-7".into()));
    let cwd = tone.get("cwd").and_then(Value::as_str).unwrap_or("/").to_string();
    let content = tone.get("content").and_then(Value::as_str)
        .or_else(|| tone.get("text").and_then(Value::as_str))
        .unwrap_or("").to_string();
    let system_prompt = tone.get("systemPrompt").and_then(Value::as_str).map(str::to_string);
    let explicit_resume = tone.get("resume").and_then(Value::as_str).map(str::to_string);

    // claude `-p --input-format stream-json` reads newline-delimited
    // user messages until stdin EOF, emitting a `result` event per
    // turn. Reuse the warm cell for the sid; per-turn state lives in
    // finish_sent + tool_use_blocks and must reset before re-entry.
    {
        let mut g = cells.lock().await;
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

    {
        let mut g = cells.lock().await;
        if g.len() >= MAX_CELLS {
            if let Some((esid, _evicted)) = g.pop_lru() {
                warn!(evicted_sid = %esid, "worker.cell.evict.lru");
                metrics::counter!("hum_cell_evictions_total", "reason" => "lru").increment(1);
            }
        }
        metrics::gauge!("hum_cell_count").set(g.len() as f64);
    }

    let mut base = SpawnSpec::new(sid.clone(), model.clone(), cwd);
    base.system_prompt = system_prompt;
    base.mcp_url = Some(mcp_url);
    if let Some(arr) = tone.get("allowedTools").and_then(Value::as_array) {
        base.allowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if let Some(arr) = tone.get("disallowedTools").and_then(Value::as_array) {
        base.disallowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }

    // A hum sid is one conversation. `claude -p` exits after each turn,
    // so the warm cell is gone before the next (tick-spaced) prompt; to
    // keep continuity we bind the sid to a deterministic claude session
    // (uuid5 of the sid) and resume it. Resume-first so the common case
    // (an ongoing sid) is one spawn; on the turn where the session does
    // not exist yet, claude's `--resume` fails fast (a `result` is_error
    // with no output), and we fall back to `--session-id` to create it.
    let cid = sid_to_session(&sid);
    let (cell, first_event) = {
        let mut s1 = base.clone();
        s1.resume_id = Some(explicit_resume.clone().unwrap_or_else(|| cid.clone()));
        match attempt_spawn(&worker, s1, &content).await {
            Some(pair) => pair,
            None => {
                trace!(sid = %sid, "worker.resume.miss.creating");
                let mut s2 = base.clone();
                s2.session_id = Some(cid.clone()); // resume_id None -> --session-id
                attempt_spawn(&worker, s2, &content).await
                    .ok_or_else(|| anyhow::anyhow!("spawn failed (resume and create both)"))?
            }
        }
    };
    let stdin = cell.stdin.clone();
    let events = cell.events.clone();
    let cancel = cell.cancel.clone();
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

    // The probe in attempt_spawn already consumed (and proved live) the
    // first event; forward it before the loop picks up the rest.
    listener.forward_raw(first_event).await;

    // Cell-lifetime dispatch: each remaining stream-json event flows
    // through the listener until the child exits.
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
    let sid_for_idle = sid.clone();
    let last_for_idle = last_touched.clone();
    let idle_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let last = last_for_idle.load(Ordering::SeqCst);
            let age = now_ms().saturating_sub(last);
            if age >= IDLE_TIMEOUT_MS {
                let mut g = cells_for_idle.lock().await;
                if g.pop(&sid_for_idle).is_some() {
                    warn!(sid = %sid_for_idle, age_ms = age, "worker.cell.idle.kill");
                }
                return;
            }
        }
    });

    cells.lock().await.put(sid.clone(), CellBundle {
        stdin: stdin.clone(),
        cancel: cancel.clone(),
        finish_sent: finish_sent.clone(),
        tool_use_blocks: tool_use_blocks.clone(),
        last_touched: last_touched.clone(),
    });

    // Prompt was already sent inside attempt_spawn; nothing to send here.
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
        cells_for_cleanup.lock().await.pop(&sid_for_cleanup);
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
                    "text" => self.chunk("text_start", json!({"blockIdx": idx})).await,
                    "thinking" => self.chunk("reasoning_start", json!({"blockIdx": idx})).await,
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
                let block_idx_v = msg.get("index").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "thinking_delta" => self.chunk("reasoning_delta", json!({"blockIdx": block_idx_v, "delta": delta.get("thinking")})).await,
                    "text_delta" => self.chunk("text_delta", json!({"blockIdx": block_idx_v, "delta": delta.get("text")})).await,
                    "input_json_delta" if !is_tool_block => {
                        self.chunk("tool_input_delta", json!({"blockIdx": block_idx_v, "partialJson": delta.get("partial_json")})).await
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
                let subtype = msg.get("subtype").and_then(Value::as_str).unwrap_or("success");
                let usage = msg.get("usage").cloned().unwrap_or(Value::Null);
                // claude signals failure inside the result event, not on
                // stderr: `is_error:true` (or an `error_*` subtype) with
                // the human-readable reason in `result`. Surfacing it is
                // the difference between a visible auth/model/credit
                // error and a silent zero-token finish that looks like
                // the worker is dead. (This was the macOS "claude exits 1
                // with no output" wall.)
                let is_error = msg.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                    || subtype.starts_with("error");
                if is_error {
                    let detail = msg.get("result").and_then(Value::as_str)
                        .or_else(|| msg.get("error").and_then(Value::as_str))
                        .filter(|s| !s.is_empty())
                        .unwrap_or("claude returned an error result with no detail (check auth / model / credit)");
                    warn!(sid = %self.sid, subtype, detail, "worker.result.error");
                    let mut err = json!({
                        "chi": "error",
                        "sid": self.sid,
                        "code": "worker_error",
                        "subtype": subtype,
                        "message": detail,
                    });
                    if let Some(obj) = err.as_object_mut() {
                        obj.insert("usage".into(), usage);
                    }
                    self.send(err).await;
                } else {
                    let mut body = json!({
                        "chi": "finish",
                        "sid": self.sid,
                        "finishReason": subtype,
                    });
                    if let Some(obj) = body.as_object_mut() {
                        obj.insert("usage".into(), usage);
                    }
                    self.send(body).await;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn bundle(cancel: CancellationToken) -> CellBundle {
        let (tx_in, _rx_in) = mpsc::channel::<String>(1);
        CellBundle {
            stdin: tx_in,
            cancel,
            finish_sent: Arc::new(AtomicBool::new(false)),
            tool_use_blocks: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
            last_touched: Arc::new(AtomicU64::new(0)),
        }
    }

    #[tokio::test]
    async fn drop_cancels_token() {
        let cancel = CancellationToken::new();
        let watch = cancel.clone();
        let b = bundle(cancel);
        assert!(!watch.is_cancelled());
        drop(b);
        assert!(watch.is_cancelled());
    }

    #[tokio::test]
    async fn lru_pop_drops_bundle_and_cancels() {
        let mut cache: LruCache<String, CellBundle> = LruCache::new(NonZeroUsize::new(2).unwrap());
        let c1 = CancellationToken::new();
        let watch1 = c1.clone();
        let c2 = CancellationToken::new();
        let watch2 = c2.clone();
        cache.put("a".into(), bundle(c1));
        cache.put("b".into(), bundle(c2));
        assert!(!watch1.is_cancelled());
        assert!(!watch2.is_cancelled());

        let popped = cache.pop_lru().expect("non-empty");
        assert_eq!(popped.0, "a");
        drop(popped);
        assert!(watch1.is_cancelled(), "evicted bundle should have cancelled on drop");
        assert!(!watch2.is_cancelled(), "remaining bundle untouched");
    }

    #[tokio::test]
    async fn map_clear_cancels_all() {
        let mut cache: LruCache<String, CellBundle> = LruCache::new(NonZeroUsize::new(4).unwrap());
        let watchers: Vec<_> = (0..3).map(|i| {
            let c = CancellationToken::new();
            let w = c.clone();
            cache.put(format!("s{i}"), bundle(c));
            w
        }).collect();
        cache.clear();
        for w in watchers {
            assert!(w.is_cancelled());
        }
    }
}
