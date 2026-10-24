//! `serve_worker` — wraps a Worker impl into a standalone process
//! that handshakes with humd via thrum and routes prompts over the wire.
//!
//! Mirrors the forager-bee client pattern: the worker process owns its own
//! lifecycle, humd is just a router. The wire contract:
//!
//! - **Hello**: announce the hive it belongs to and the models it serves.
//!   humd routes to it only once that hive is installed there.
//! - **Prompt in**: humd forwards `chi:"prompt"` tones whose `modelId`
//!   matches one of the worker's advertised models. The worker calls
//!   `Worker::raise(egg)`, then feeds the prompt text on `cell.feed`.
//! - **Chunks out**: each event from `cell.mmm` becomes a
//!   `chi:"chunk"` tone tagged with `chunkType` + the original sid.
//! - **Cancel**: `chi:"cancel"` triggers `cell.still()` for the sid.
//! - **Tool result**: `chi:"tool-result"` feeds into the cell via the
//!   worker's tool-result encoder (`hum_nest::encode_tool_result`).
//!
//! The connection is hum-thrum's: the worker links its `main` bee and
//! reads events. A humd that goes away takes the worker's cells with it,
//! since their sessions went with that humd; the client dials again,
//! quietly while humd is still booting, and the worker carries on. A
//! refused bee ends `serve_worker` with the refusal, so the process exits
//! non-zero and its supervisor decides.
//!
//! A prompt's model comes from its `modelId`, else the first model the
//! worker advertises (`CLAUDE_MODELS` leads with the install default),
//! else opus 4.7. Without a fallback an asker that omits `modelId`
//! would spawn `claude --model ""`.
//!
//! `claude -p --input-format stream-json` reads newline-delimited user
//! messages until stdin EOF, emitting a `result` event per turn, and
//! exits after each turn. The warm cell is gone by the next prompt, so
//! continuity comes from the sid-derived session UUID that claude-cli
//! computes from the sid.
//!
//! A `result` event flagged `is_error` as the *first* event means a
//! pre-flight failure — a bad or absent session on `--resume`, an id
//! clash on `--session-id` — rather than a mid-turn error, which arrives
//! only after content. On the first turn `--resume` fails fast that
//! way, so the worker retries with `fresh: true`.
//!
//! claude reports failure inside that result event, not on stderr:
//! `is_error:true` (or an `error_*` subtype) with the human-readable
//! reason in `result`. Surfacing it is the difference between a visible
//! auth, model, or credit error and a silent zero-token finish that
//! looks like the worker is dead.
//!
//! The tool catalogue is MCP: the standard field is `inputSchema`, and
//! the legacy `parameters` spelling is tolerated on the wire. An entry
//! with no name or no usable schema is dropped, because claude's mcp
//! client rejects the entire tools/list if any entry has
//! `inputSchema: null` — one bad apple kills all tools for the session.
//!
//! A tool-result whose callId is not pending in the worker MCP bridge
//! came from a call the model made outside our catalogue, so it is
//! forwarded to claude over stdin rather than dropped.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use lru::LruCache;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use hum_mcp::protocol::ToolDef;
use crate::{encode_cancel, encode_prompt, encode_tool_result, Cell, Egg, NestConfig, Worker};
use tokio::sync::mpsc;

use hum_thrum::{Bee, Event, Options, Refused};
use hum_mcp::{spawn_local_mcp, McpBridge};


#[derive(Debug, Clone)]
pub struct WorkerAdvert {
    pub hive: String,
    pub version: String,
    pub models: Vec<String>,
    pub source: Option<String>,
}

pub async fn serve_worker<W: Worker + 'static>(worker: Arc<W>, advert: WorkerAdvert) -> Result<()> {
    let nest = Arc::new(NestConfig::for_hive(&advert.hive));
    let propensity_str = match worker.propensity() {
        crate::Propensity::StatefulSession => "stateful_session",
        crate::Propensity::StatelessPerCall => "stateless_per_call",
        crate::Propensity::EphemeralPerCall => "ephemeral_per_call",
    };
    let hello = json!({
        "roles": ["worker"],
        "version": &advert.version,
        "models": &advert.models,
        "propensity": { "statefulness": propensity_str, "wire": &advert.hive },
        "chis": ["hello", "prompt", "cancel", "curate", "tool-result", "chunk", "finish", "error", "tool-call", "pulse"],
        "headroom": nest.headroom(0),
        "source": advert.source.clone().unwrap_or_default(),
    });
    info!(hive = %advert.hive, models = ?advert.models, "worker.linking");
    let mut bee = Bee::link(Options::new(&advert.hive, "main").hello(hello))
        .await
        .with_context(|| format!("link the main bee of {}", advert.hive))?;
    info!(hive = %advert.hive, bee = %bee.id(), models = ?advert.models, "worker.nestled");
    let write_half = bee.sender();

    let write_for_bridge = write_half.clone();
    let bridge = McpBridge::new(Arc::new(move |tone: Value| {
        write_for_bridge.send(tone);
    }));
    let mcp_addr = spawn_local_mcp(bridge.clone()).await
        .context("spawn local mcp bridge")?;
    let mcp_url = format!("http://{}", mcp_addr);
    info!(%mcp_url, "worker.mcp.bridge.up");

    let pool = NonZeroUsize::new(nest.max_cells()).unwrap_or(NonZeroUsize::MIN);
    let cells: Arc<Mutex<LruCache<String, CellBundle>>> = Arc::new(Mutex::new(LruCache::new(pool)));

    let sid_cwd: Arc<Mutex<LruCache<String, String>>> = Arc::new(Mutex::new(LruCache::new(pool)));

    while let Some(event) = bee.next().await {
        let tone = match event {
            Event::Tone(tone) => tone,
            Event::Lost => {
                cells.lock().await.clear();
                continue;
            }
            Event::Nestled | Event::Advisory(_) => continue,
            Event::Refused(why) => return Err(Refused(why).into()),
        };
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
        let sid = tone.get("sid").and_then(Value::as_str).map(str::to_string).unwrap_or_default();

        match chi {
            "prompt" => {
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
                let cwd = if !sid.is_empty() {
                    tone.get("cwd").and_then(Value::as_str).map(str::to_string)
                } else { None };
                let worker = worker.clone();
                let write_half = write_half.clone();
                let cells = cells.clone();
                let hive = advert.hive.clone();
                let mcp_url = mcp_url.clone();
                let sid_cwd = sid_cwd.clone();
                let nest = nest.clone();
                let sid2 = sid.clone();
                tokio::spawn(async move {
                    if let Some(cwd) = cwd {
                        sid_cwd.lock().await.put(sid2, cwd);
                    }
                    if let Err(e) = handle_prompt(worker, write_half, cells, nest, hive, mcp_url, tone).await {
                        warn!(err = %e, "worker.prompt.handle.failed");
                    }
                });
            }
            "cancel" => {
                if !sid.is_empty() {
                    let cells = cells.clone();
                    let tone = tone.clone();
                    tokio::spawn(async move {
                        let mut r = cells.lock().await;
                        if let Some(bundle) = r.get(&sid) {
                            if let Some(rid) = tone.get("rid").and_then(Value::as_str) {
                                let _ = bundle.stdin.send(encode_cancel(rid)).await;
                            }
                            bundle.cancel.cancel();
                        }
                    });
                }
            }
            "curate" => {
                if !sid.is_empty() {
                    let sid_cwd = sid_cwd.clone();
                    let worker = worker.clone();
                    let sid = sid.clone();
                    tokio::spawn(async move {
                        let cwd = sid_cwd.lock().await.get(&sid).cloned();
                        match cwd {
                            Some(cwd) => {
                                let hum_sid = hum_identity::HumId::parse(&sid)
                                    .unwrap_or_else(|_| hum_identity::HumId::from_foreign(&sid));
                                match worker.curate(&hum_sid, &cwd).await {
                                    Ok(report) => trace!(
                                        sid = %sid,
                                        trimmed = report.trimmed(),
                                        "worker.curate.done"
                                    ),
                                    Err(e) => warn!(sid = %sid, err = %e, "worker.curate.failed"),
                                }
                            }
                            None => trace!(sid = %sid, "worker.curate.unknown-sid"),
                        }
                    });
                }
            }
            "tool-result" => {
                let call_id = tone.get("callId").and_then(Value::as_str).map(str::to_string);
                let resolved_by_bridge = call_id.as_deref()
                    .map(|cid| bridge.resolve(cid, tone.clone()))
                    .unwrap_or(false);
                if !resolved_by_bridge && !sid.is_empty() {
                    let cells = cells.clone();
                    let tone = tone.clone();
                    tokio::spawn(async move {
                        let mut r = cells.lock().await;
                        if let Some(bundle) = r.get(&sid)
                            && let (Some(call_id), Some(result)) = (
                                tone.get("callId").and_then(Value::as_str),
                                tone.get("result").and_then(Value::as_str),
                            )
                        {
                            let _ = bundle.stdin.send(encode_tool_result(call_id, result)).await;
                        }
                    });
                }
            }
            "breath" | "echo" | "" => {
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

async fn pulse(
    write_half: &hum_thrum::Sender,
    kind: &str,
    headroom: Value,
) {
    let tone = json!({
        "chi": "pulse",
        "kind": kind,
        "headroom": headroom,
    });
    write_half.send(tone);
}

fn is_preflight_error(v: &Value) -> bool {
    let m = if v.get("type").and_then(Value::as_str) == Some("stream_event") {
        v.get("event").unwrap_or(v)
    } else { v };
    m.get("type").and_then(Value::as_str) == Some("result")
        && m.get("is_error").and_then(Value::as_bool).unwrap_or(false)
}

async fn attempt_spawn<W: Worker + 'static>(
    worker: &Arc<W>,
    spec: Egg,
    content: &str,
) -> Option<(Cell, Value)> {
    let cell = worker.raise(spec).await.ok()?;
    if cell.feed.send(encode_prompt(content)).await.is_err() {
        cell.silence.cancel();
        return None;
    }
    let first = {
        let mut ev = cell.mmm.lock().await;
        match tokio::time::timeout(std::time::Duration::from_secs(180), ev.recv()).await {
            Ok(Some(v)) => Some(v),
            _ => None,
        }
    };
    match first {
        Some(v) if is_preflight_error(&v) => { cell.silence.cancel(); None }
        Some(v) => Some((cell, v)),
        None => { cell.silence.cancel(); None }
    }
}

async fn handle_prompt<W: Worker + 'static>(
    worker: Arc<W>,
    write_half: hum_thrum::Sender,
    cells: Arc<Mutex<LruCache<String, CellBundle>>>,
    nest: Arc<NestConfig>,
    hive: String,
    mcp_url: String,
    tone: Value,
) -> Result<()> {
    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
    if sid.is_empty() { anyhow::bail!("prompt.no-sid"); }
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

    let evicted = {
        let mut g = cells.lock().await;
        let evicted = if g.len() >= nest.max_cells()
            && let Some((esid, _evicted)) = g.pop_lru()
        {
            warn!(evicted_sid = %esid, "worker.cell.evict.lru");
            metrics::counter!("hum_cell_evictions_total", "reason" => "lru").increment(1);
            Some(g.len())
        } else {
            None
        };
        metrics::gauge!("hum_cell_count").set(g.len() as f64);
        evicted
    };
    if let Some(live) = evicted {
        pulse(&write_half, "cell-evicted", nest.headroom(live)).await;
    }

    let hum_sid = hum_identity::HumId::parse(&sid).unwrap_or_else(|_| hum_identity::HumId::from_foreign(&sid));
    let mut base = Egg::new(hum_sid, model.clone(), cwd);
    base.system_prompt = system_prompt;
    base.mcp_url = Some(mcp_url);
    base.bounds = nest.bounds.clone();
    if let Some(arr) = tone.get("allowedTools").and_then(Value::as_array) {
        base.allowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }
    if let Some(arr) = tone.get("disallowedTools").and_then(Value::as_array) {
        base.disallowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }

    let (cell, first_event) = {
        let mut s1 = base.clone();
        s1.resume_id = explicit_resume.clone();
        match attempt_spawn(&worker, s1, &content).await {
            Some(pair) => pair,
            None => {
                trace!(sid = %sid, "worker.resume.miss.creating");
                let mut s2 = base.clone();
                s2.fresh = true;
                attempt_spawn(&worker, s2, &content).await
                    .ok_or_else(|| anyhow::anyhow!("spawn failed (resume and create both)"))?
            }
        }
    };
    let stdin = cell.feed.clone();
    let events = cell.mmm.clone();
    let cancel = cell.silence.clone();
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

    listener.forward_raw(first_event).await;

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

    let cells_for_idle = cells.clone();
    let sid_for_idle = sid.clone();
    let last_for_idle = last_touched.clone();
    let write_for_idle = write_half.clone();
    let nest_for_idle = nest.clone();
    let idle_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let last = last_for_idle.load(Ordering::SeqCst);
            let age = now_ms().saturating_sub(last);
            if age >= nest_for_idle.idle_ms {
                let live = {
                    let mut g = cells_for_idle.lock().await;
                    g.pop(&sid_for_idle).map(|_| g.len())
                };
                if let Some(live) = live {
                    warn!(sid = %sid_for_idle, age_ms = age, "worker.cell.idle.kill");
                    pulse(&write_for_idle, "cell-evicted", nest_for_idle.headroom(live)).await;
                }
                return;
            }
        }
    });

    let live = {
        let mut g = cells.lock().await;
        g.put(sid.clone(), CellBundle {
            stdin: stdin.clone(),
            cancel: cancel.clone(),
            finish_sent: finish_sent.clone(),
            tool_use_blocks: tool_use_blocks.clone(),
            last_touched: last_touched.clone(),
        });
        g.len()
    };
    pulse(&write_half, "cell-spawned", nest.headroom(live)).await;

    trace!(sid = %sid, "worker.cell.spawned");

    let cells_for_cleanup = cells.clone();
    let write_for_cleanup = write_half.clone();
    let sid_for_cleanup = sid.clone();
    let finish_for_cleanup = finish_sent.clone();
    let nest_for_cleanup = nest.clone();
    tokio::spawn(async move {
        let exit_code = cell.emerged.await.unwrap_or(1);
        let _ = dispatch.await;
        idle_task.abort();
        if !finish_for_cleanup.load(Ordering::SeqCst) {
            let finish = json!({
                "chi": "finish",
                "sid": sid_for_cleanup,
                "finishReason": if exit_code == 0 { "stop" } else { "error" },
                "exitCode": exit_code,
            });
            write_for_cleanup.send(finish);
        }
        let live = {
            let mut g = cells_for_cleanup.lock().await;
            g.pop(&sid_for_cleanup).map(|_| g.len())
        };
        if let Some(live) = live {
            pulse(&write_for_cleanup, "cell-died", nest_for_cleanup.headroom(live)).await;
        }
        trace!(sid = %sid_for_cleanup, exit_code, "worker.cell.exit");
    });

    Ok(())
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct WireListener {
    sid: String,
    hive: String,
    write_half: hum_thrum::Sender,
    finish_sent: Arc<AtomicBool>,
    tool_use_blocks: Arc<Mutex<std::collections::BTreeSet<i64>>>,
    last_touched: Arc<AtomicU64>,
}

impl WireListener {
    async fn send(&self, tone: Value) {
        self.write_half.send(tone);
    }

    async fn forward_raw(&self, value: Value) {
        self.last_touched.store(now_ms(), Ordering::SeqCst);

        if value.get("chi").and_then(Value::as_str).is_some() {
            let mut v = value.clone();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("sid".into(), Value::String(self.sid.clone()));
            }
            self.send(v).await;
            return;
        }

        let mut msg = value;
        if msg.get("type").and_then(Value::as_str) == Some("stream_event")
            && let Some(inner) = msg.get("event").cloned()
        {
            msg = inner;
        }
        let typ = msg.get("type").and_then(Value::as_str).unwrap_or("").to_string();

        match typ.as_str() {
            "system" if msg.get("subtype").and_then(Value::as_str) == Some("init") => {
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
                let bridge_reports_tool_whole = match idx {
                    Some(i) => self.tool_use_blocks.lock().await.contains(&i),
                    None => false,
                };
                let block_idx_v = msg.get("index").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "thinking_delta" => self.chunk("reasoning_delta", json!({"blockIdx": block_idx_v, "delta": delta.get("thinking")})).await,
                    "text_delta" => self.chunk("text_delta", json!({"blockIdx": block_idx_v, "delta": delta.get("text")})).await,
                    "input_json_delta" if !bridge_reports_tool_whole => {
                        self.chunk("tool_input_delta", json!({"blockIdx": block_idx_v, "partialJson": delta.get("partial_json")})).await
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = msg.get("index").and_then(Value::as_i64);
                let bridge_reports_tool_whole = match idx {
                    Some(i) => self.tool_use_blocks.lock().await.remove(&i),
                    None => false,
                };
                if !bridge_reports_tool_whole {
                    self.chunk("content_block_stop", json!({"blockIdx": msg.get("index")})).await;
                }
            }
            "result" => {
                let subtype = msg.get("subtype").and_then(Value::as_str).unwrap_or("success");
                let usage = msg.get("usage").cloned().unwrap_or(Value::Null);
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
