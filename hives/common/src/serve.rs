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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use ensemble::HidPrefix;
use mcp::protocol::ToolDef;
use nest::{encode_prompt, Listener, SpawnSpec, WorkerBee};

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

    // Spawn the worker-local MCP bridge. Compute spawned by this
    // worker (e.g. claude binary) dials it for tools/list +
    // tools/call. The bridge ships chi:"tool-call" tones via the
    // worker's thrum write half; humd routes them by toolName +
    // (sid-pinned) fs_hid. chi:"tool-result" tones arriving back
    // get resolved by callId.
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
                        // stdin is closed after prompt send (forces
                        // claude's EOF-on-finish). encode_cancel via
                        // stdin no longer reaches the child — kill
                        // the process is the only stop signal we
                        // have. Worker MCP bridge in-flight calls
                        // are aborted by the bridge HTTP timeout.
                        (bundle.kill)();
                    }
                }
            }
            "tool-result" => {
                let call_id = tone.get("callId").and_then(Value::as_str).map(str::to_string);
                // All tool round-trips now resolve through the
                // worker MCP bridge (R4 architecture). Anything
                // arriving here with a matching callId resolves;
                // un-matched chi:tool-result tones are orphans we
                // can't ship to claude (stdin closed) — log + drop.
                let resolved_by_bridge = call_id.as_deref()
                    .map(|cid| bridge.resolve(cid, tone.clone()))
                    .unwrap_or(false);
                if !resolved_by_bridge {
                    if let Some(cid) = call_id.as_deref() {
                        trace!(call_id = cid, "worker.tool-result.orphan");
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
    kill: Arc<dyn Fn() + Send + Sync>,
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

    // Build SpawnSpec — only the common knobs; worker-specific extras
    // (sampling block, cli flags) ride along on the tone if the
    // worker wants them. mcp_url points at THIS worker's local
    // bridge — the compute (e.g. claude) sees a single MCP
    // catalogue that humfs + asker-shipped tools both feed.
    let mut spec = SpawnSpec::new(sid.clone(), model.clone(), cwd);
    spec.system_prompt = system_prompt;
    spec.mcp_url = Some(mcp_url);
    // Resume id flows from the asker shim (openai-server's
    // /v1/responses captures claude's session_id from the
    // chi:"session-ready" tone on turn N, then on turn N+1 attaches
    // it here). When set, claude-cli is invoked with
    // `--resume <id>` so the model sees the full prior conversation
    // — including tool calls + results — as native MCP history,
    // not as a text-marker pastiche.
    spec.resume_id = tone.get("resume").and_then(Value::as_str).map(str::to_string);
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

    // First user turn — murmur the prompt content as a single user
    // message. Then immediately drop both stdin Sender clones so
    // the channel closes, the stdin pump exits, and the child's
    // stdin pipe gets EOF. Current claude `-p` requires the EOF
    // even in stream-json mode — without it, claude blocks on
    // epoll_wait forever after emitting its `result` event, and
    // child processes pile up (52 alive in production before this
    // fix).
    //
    // Side effect: chi:"cancel" and non-MCP chi:"tool-result"
    // arms can no longer push down stdin since the channel's
    // gone. That's the right tradeoff under the worker MCP
    // bridge architecture — all tool round-trips resolve via
    // bridge HTTP, not stdin. The CellBundle still carries the
    // kill handle so chi:"cancel" can force-exit the child.
    cells.lock().await.insert(sid.clone(), CellBundle { kill: kill.clone() });
    stdin.send(encode_prompt(&content)).await
        .map_err(|e| anyhow::anyhow!("stdin closed: {e}"))?;
    drop(stdin);
    drop(cell.stdin);

    // Dispatch loop — Cell.events → chi:"chunk" tones over thrum.
    // `finish_sent` lets the dispatch path claim the finish emit when
    // it sees claude's `result` event; the post-exit fallback below
    // only fires on crash paths.
    let finish_sent = Arc::new(AtomicBool::new(false));
    let listener = Arc::new(WireListener {
        sid: sid.clone(),
        hive,
        write_half: write_half.clone(),
        finish_sent: finish_sent.clone(),
        tool_use_blocks: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
    });

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

    // Wait for exit — but with a deadline. claude `-p` should exit
    // after emitting its `result` event (one turn). When it doesn't
    // (stale MCP bridge URL after worker restart, tool-call hang,
    // streaming wait), nothing reaped the child and processes
    // accumulated — 52 alive simultaneously in production. Now:
    //
    // 1. Wait at most KILL_AFTER_FINISH_MS after finish_sent fires
    //    before sending SIGKILL via the cell's kill handle.
    // 2. If no finish ever fires, KILL_NO_FINISH_MS is the absolute
    //    ceiling. Crash-path fallback finish is still emitted so
    //    askers don't hang on /v1/{responses,chat/completions} SSE.
    //
    // Tuned high enough that healthy turns finish well within the
    // grace window; tuned low enough that orphans never pile up.
    const KILL_AFTER_FINISH_MS: u64 = 5_000;
    const KILL_NO_FINISH_MS: u64 = 600_000;

    let finish_watcher = finish_sent.clone();
    let kill_for_timeout = kill.clone();
    let sid_for_timeout = sid.clone();
    let exit_killer = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let mut killed = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if killed { return; }
            let elapsed = start.elapsed();
            if finish_watcher.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(KILL_AFTER_FINISH_MS)).await;
                if !killed {
                    warn!(sid = %sid_for_timeout, "worker.cell.kill.after-finish.grace");
                    (kill_for_timeout)();
                    killed = true;
                    return;
                }
            }
            if elapsed.as_millis() as u64 > KILL_NO_FINISH_MS {
                warn!(sid = %sid_for_timeout, "worker.cell.kill.no-finish.ceiling");
                (kill_for_timeout)();
                killed = true;
                return;
            }
        }
    });

    let exit_code = cell.exited.await.unwrap_or(1);
    exit_killer.abort();
    let _ = dispatch.await;
    if !finish_sent.load(Ordering::SeqCst) {
        let finish = json!({
            "chi": "finish",
            "sid": sid,
            "finishReason": if exit_code == 0 { "stop" } else { "error" },
            "exitCode": exit_code,
        });
        let line = format!("{}\n", finish);
        let _ = write_half.lock().await.write_all(line.as_bytes()).await;
    }
    cells.lock().await.remove(&sid);
    Ok(())
}

/// Translates raw claude-stream-json events into chi:"chunk" tones.
/// Lives in the helper so each worker crate doesn't reinvent it.
struct WireListener {
    sid: String,
    hive: String,
    write_half: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    /// Set once `result` is forwarded as `chi:"finish"`. handle_prompt
    /// checks this before emitting its fallback finish so consumers
    /// never see two finishes per prompt.
    finish_sent: Arc<AtomicBool>,
    /// Block indices the model emitted as `tool_use`. We resolve all
    /// tools internally via the worker's MCP bridge (humd routes the
    /// tone to a forager, the result feeds back into claude before
    /// the model continues) — the asking nestler never sees nor can
    /// execute these calls. So we suppress the tool_input_* /
    /// content_block_stop chi:chunks for any block index in this set
    /// to prevent openai-server from leaking them as OpenAI
    /// `tool_calls` (OC then reports `tool: invalid` because the
    /// bare humfs_* name isn't in its registry, the file is already
    /// written, and the UI stalls on an empty trailing message).
    tool_use_blocks: Arc<Mutex<std::collections::BTreeSet<i64>>>,
}

impl WireListener {
    async fn send(&self, tone: Value) {
        let line = format!("{}\n", tone);
        let _ = self.write_half.lock().await.write_all(line.as_bytes()).await;
    }

    async fn forward_raw(&self, value: Value) {
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
                        // Track the index so the corresponding
                        // input_json_delta + content_block_stop get
                        // suppressed. Worker resolves tools inline via
                        // the MCP bridge; the bridge ships a single
                        // chi:"chunk" tool_executed with full args +
                        // output once the call completes. Emitting
                        // tool_input_* here would leak the in-flight
                        // call to nestlings (the bug that landed
                        // tool: invalid parts in OC).
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
