//! `serve_nurse` — wraps a Nurse impl into a standalone process that
//! handshakes with humd via thrum and grows the blooms it is sent.
//!
//! The nurse process owns its own lifecycle; humd is just a router. The
//! wire contract:
//!
//! - **Hello**: announce the hive it belongs to, its role, and the models
//!   it serves. humd routes to it only once that hive is installed there.
//! - **Prompt in**: humd forwards `chi:"prompt"` tones whose `modelId`
//!   matches one of the nurse's advertised models. The nurse calls
//!   `Nurse::raise(egg)`, then feeds the prompt text on `cell.feed`. The
//!   prompt's `tools` are what its cell may reach, offered over the
//!   sigil's [`mcp`](crate::mcp) path.
//! - **The bloom out**: each content block from `cell.mmm` unfurls as one
//!   petal, by its index: text as `text`, thinking as `sepal` sealed by
//!   its `propolis`. The cell's init is a `cell-ready` pulse, and the
//!   result wilts the bloom with its nectar.
//! - **Cancel**: `chi:"cancel"` triggers `cell.still()` for the sid.
//! - **Tendril result**: `chi:"tendril-result"` answers the cell's MCP
//!   call by its `callId`, or, for a call outside the sigil's tools, feeds
//!   into the cell via the tool-result encoder (`hum_nest::encode_tool_result`).
//! - **Cleanup**: `chi:"cleanup"` forgets the sigil's tools.
//!
//! A cell runs in its nest's directory, `hives/<hive>/nest` in the state
//! root, never where its bloom builds: that host is a builder's, reached
//! through `bash`.
//!
//! The connection is hum-thrum's: the nurse links its `main` bee and
//! reads events. A humd that goes away takes the nurse's cells with it,
//! since their sigils went with that humd; the client dials again,
//! quietly while humd is still booting, and the nurse carries on. A
//! refused bee ends `serve_nurse` with the refusal, so the process exits
//! non-zero and its supervisor decides.
//!
//! A prompt's model comes from its `modelId`, else the first model the
//! nurse advertises (`CLAUDE_MODELS` leads with the install default),
//! else opus 4.7. Without a fallback a forager that omits `modelId`
//! would spawn `claude --model ""`.
//!
//! `claude -p --input-format stream-json` reads newline-delimited user
//! messages until stdin EOF, emitting a `result` event per bloom, and
//! exits after each bloom. The warm cell is gone by the next prompt, so
//! continuity comes from the sid-derived session UUID that claude-cli
//! computes from the sid.
//!
//! A `result` event flagged `is_error` as the *first* event means a
//! pre-flight failure — a bad or absent session on `--resume`, an id
//! clash on `--session-id` — rather than a mid-bloom error, which arrives
//! only after content. On the first bloom `--resume` fails fast that
//! way, so the nurse retries with `fresh: true`.
//!
//! claude reports failure inside that result event, not on stderr:
//! `is_error:true` (or an `error_*` subtype) with the human-readable
//! reason in `result`. Surfacing it is the difference between a visible
//! auth, model, or credit error and a silent zero-token wilt that
//! looks like the nurse is dead.
//!
//! A tendril-result whose callId is not pending in the MCP bridge came
//! from a call the model made outside the sigil's tools, so it is
//! forwarded to claude over stdin rather than dropped.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use lru::LruCache;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use crate::mcp::{self, Bridge, ToolDef};
use crate::{encode_cancel, encode_petals, encode_tool_result, Cell, Egg, NestConfig, Nurse};
use thrum_core::{Petal, PetalKind, Role};
use tokio::sync::mpsc;

use hum_thrum::{Bee, Event, Options, Refused};

#[derive(Debug, Clone)]
pub struct NurseAdvert {
    pub hive: String,
    pub label: String,
    pub version: String,
    pub models: Vec<String>,
    pub source: Option<String>,
}

pub async fn serve_nurse<N: Nurse + 'static>(nurse: Arc<N>, advert: NurseAdvert) -> Result<()> {
    let nest = Arc::new(NestConfig::for_hive(&advert.hive));
    let home = hum_paths::hive_state_dir(&advert.hive).join("nest");
    std::fs::create_dir_all(&home).with_context(|| format!("make the nest at {}", home.display()))?;
    let home = home.to_string_lossy().to_string();
    let propensity_str = match nurse.propensity() {
        crate::Propensity::StatefulSession => "stateful_session",
        crate::Propensity::StatelessPerCall => "stateless_per_call",
        crate::Propensity::EphemeralPerCall => "ephemeral_per_call",
    };
    let hello = json!({
        "version": &advert.version,
        "models": &advert.models,
        "propensity": { "statefulness": propensity_str, "wire": &advert.hive },
        "chis": ["hello", "prompt", "cancel", "cleanup", "curate", "tendril-result", "unfurl", "wilt", "error", "tendril-reach", "tendril-info", "pulse"],
        "headroom": nest.headroom(0),
        "source": advert.source.clone().unwrap_or_default(),
    });
    info!(hive = %advert.hive, models = ?advert.models, "nurse.linking");
    let mut bee = Bee::link(Options::new(&advert.hive, &advert.label, Role::Nurse).hello(hello))
        .await
        .with_context(|| format!("link the {} bee of {}", advert.label, advert.hive))?;
    info!(hive = %advert.hive, bee = %bee.id(), models = ?advert.models, "nurse.nestled");
    let write_half = bee.sender();

    let pool = NonZeroUsize::new(nest.max_cells()).unwrap_or(NonZeroUsize::MIN);
    let sigils = pool.saturating_mul(NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN));
    let write_for_bridge = write_half.clone();
    let bridge = Bridge::new(sigils, Arc::new(move |tone: Value| {
        write_for_bridge.send(tone);
    }));
    let mcp_addr = mcp::serve(bridge.clone()).await.context("serve the nest's mcp")?;
    let mcp_url = format!("http://{}", mcp_addr);
    info!(%mcp_url, "nurse.mcp.up");

    let cells: Arc<Mutex<LruCache<String, CellBundle>>> = Arc::new(Mutex::new(LruCache::new(pool)));

    while let Some(event) = bee.next().await {
        let tone = match event {
            Event::Tone(tone) => tone,
            Event::Astray => {
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
                if !sid.is_empty() {
                    bridge.offer(&sid, ToolDef::all_in(tone.get("tools")));
                }
                let nurse = nurse.clone();
                let write_half = write_half.clone();
                let cells = cells.clone();
                let mcp_url = mcp_url.clone();
                let nest = nest.clone();
                let home = home.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_prompt(nurse, write_half, cells, nest, mcp_url, home, tone).await {
                        warn!(err = %e, "nurse.prompt.handle.failed");
                    }
                });
            }
            "cleanup" => {
                if !sid.is_empty() {
                    bridge.forget(&sid);
                }
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
                    let nurse = nurse.clone();
                    let home = home.clone();
                    let sid = sid.clone();
                    tokio::spawn(async move {
                        let hum_sid = hum_identity::HumId::parse(&sid)
                            .unwrap_or_else(|_| hum_identity::HumId::from_foreign(&sid));
                        match nurse.curate(&hum_sid, &home).await {
                            Ok(report) => trace!(sid = %sid, trimmed = report.trimmed(), "nurse.curate.done"),
                            Err(e) => warn!(sid = %sid, err = %e, "nurse.curate.failed"),
                        }
                    });
                }
            }
            "tendril-result" => {
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
                            && let Some(call_id) = tone.get("callId").and_then(Value::as_str)
                        {
                            let grasped = crate::ToolResult::of(&tone);
                            let _ = bundle.stdin.send(encode_tool_result(call_id, &grasped.output)).await;
                        }
                    });
                }
            }
            "breath" | "echo" | "" => {
                        }
            other => {
                trace!(chi = other, "nurse.unknown.chi");
            }
        }
    }
    Ok(())
}

struct CellBundle {
    stdin: mpsc::Sender<String>,
    cancel: tokio_util::sync::CancellationToken,
    wilted: Arc<AtomicBool>,
    petals: Arc<Mutex<BTreeMap<u64, PetalKind>>>,
    last_touched: Arc<AtomicU64>,
}

impl Drop for CellBundle {
    fn drop(&mut self) { self.cancel.cancel(); }
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

async fn attempt_spawn<N: Nurse + 'static>(
    nurse: &Arc<N>,
    spec: Egg,
    message: &str,
) -> Option<(Cell, Value)> {
    let cell = nurse.raise(spec).await.ok()?;
    if cell.feed.send(message.to_string()).await.is_err() {
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

async fn handle_prompt<N: Nurse + 'static>(
    nurse: Arc<N>,
    write_half: hum_thrum::Sender,
    cells: Arc<Mutex<LruCache<String, CellBundle>>>,
    nest: Arc<NestConfig>,
    mcp_url: String,
    home: String,
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
    let petals: Vec<Petal> = tone.get("content").cloned()
        .and_then(|c| serde_json::from_value(c).ok())
        .filter(|p: &Vec<Petal>| !p.is_empty())
        .unwrap_or_else(|| vec![Petal {
            kind: PetalKind::Text.as_str().into(),
            text: tone.get("text").and_then(Value::as_str).map(str::to_string),
            ..Petal::default()
        }]);
    let message = encode_petals(&petals);
    let system_prompt = tone.get("systemPrompt").and_then(Value::as_str).map(str::to_string);
    let explicit_resume = tone.get("resume").and_then(Value::as_str).map(str::to_string);

    {
        let mut g = cells.lock().await;
        if let Some(bundle) = g.get(&sid) {
            bundle.wilted.store(false, Ordering::SeqCst);
            bundle.petals.lock().await.clear();
            bundle.last_touched.store(now_ms(), Ordering::SeqCst);
            let send = bundle.stdin.clone();
            drop(g);
            send.send(message).await
                .map_err(|e| anyhow::anyhow!("stdin closed on reused cell: {e}"))?;
            trace!(sid = %sid, "nurse.cell.reused");
            return Ok(());
        }
    }

    let evicted = {
        let mut g = cells.lock().await;
        let evicted = if g.len() >= nest.max_cells()
            && let Some((esid, _evicted)) = g.pop_lru()
        {
            warn!(evicted_sid = %esid, "nurse.cell.evict.lru");
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
    let mut base = Egg::new(hum_sid, model.clone(), home);
    base.system_prompt = system_prompt;
    base.mcp_url = Some(mcp_url);
    base.bounds = nest.bounds.clone();
    if let Some(arr) = tone.get("allowedTools").and_then(Value::as_array) {
        base.allowed_tools = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
    }

    let (cell, first_event) = {
        let mut s1 = base.clone();
        s1.resume_id = explicit_resume.clone();
        match attempt_spawn(&nurse, s1, &message).await {
            Some(pair) => pair,
            None => {
                trace!(sid = %sid, "nurse.resume.miss.creating");
                let mut s2 = base.clone();
                s2.fresh = true;
                attempt_spawn(&nurse, s2, &message).await
                    .ok_or_else(|| anyhow::anyhow!("spawn failed (resume and create both)"))?
            }
        }
    };
    let stdin = cell.feed.clone();
    let events = cell.mmm.clone();
    let cancel = cell.silence.clone();
    let wilted = Arc::new(AtomicBool::new(false));
    let petals = Arc::new(Mutex::new(BTreeMap::new()));
    let last_touched = Arc::new(AtomicU64::new(now_ms()));

    let listener = Arc::new(WireListener {
        sid: sid.clone(),
        write_half: write_half.clone(),
        wilted: wilted.clone(),
        petals: petals.clone(),
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
        trace!(sid = %sid_for_loop, "nurse.dispatch.exit");
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
                    warn!(sid = %sid_for_idle, age_ms = age, "nurse.cell.idle.kill");
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
            wilted: wilted.clone(),
            petals: petals.clone(),
            last_touched: last_touched.clone(),
        });
        g.len()
    };
    pulse(&write_half, "cell-spawned", nest.headroom(live)).await;

    trace!(sid = %sid, "nurse.cell.spawned");

    let cells_for_cleanup = cells.clone();
    let write_for_cleanup = write_half.clone();
    let sid_for_cleanup = sid.clone();
    let wilted_for_cleanup = wilted.clone();
    let nest_for_cleanup = nest.clone();
    tokio::spawn(async move {
        let exit_code = cell.emerged.await.unwrap_or(1);
        let _ = dispatch.await;
        idle_task.abort();
        if !wilted_for_cleanup.load(Ordering::SeqCst) {
            write_for_cleanup.send(json!({
                "chi": "wilt",
                "sid": sid_for_cleanup,
                "reason": if exit_code == 0 { "stop" } else { "error" },
                "exitCode": exit_code,
            }));
        }
        let live = {
            let mut g = cells_for_cleanup.lock().await;
            g.pop(&sid_for_cleanup).map(|_| g.len())
        };
        if let Some(live) = live {
            pulse(&write_for_cleanup, "cell-died", nest_for_cleanup.headroom(live)).await;
        }
        trace!(sid = %sid_for_cleanup, exit_code, "nurse.cell.exit");
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
    write_half: hum_thrum::Sender,
    wilted: Arc<AtomicBool>,
    petals: Arc<Mutex<BTreeMap<u64, PetalKind>>>,
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
        let index = msg.get("index").and_then(Value::as_u64);

        match (typ.as_str(), index) {
            ("system", _) if msg.get("subtype").and_then(Value::as_str) == Some("init") => {
                self.send(json!({
                    "chi": "pulse",
                    "sid": self.sid,
                    "kind": "cell-ready",
                    "cell": msg.get("session_id").cloned().unwrap_or(Value::Null),
                    "model": msg.get("model").cloned().unwrap_or(Value::Null),
                    "tools": msg.get("tools").cloned().unwrap_or(json!([])),
                }))
                .await;
            }
            ("content_block_start", Some(petal)) => {
                let kind = match msg.pointer("/content_block/type").and_then(Value::as_str) {
                    Some("text") => PetalKind::Text,
                    Some("thinking" | "redacted_thinking") => PetalKind::Sepal,
                    Some("tool_use") => PetalKind::Tendril,
                    _ => return,
                };
                self.petals.lock().await.insert(petal, kind);
                if kind != PetalKind::Tendril {
                    self.unfurl(petal, kind, json!({})).await;
                }
            }
            ("content_block_delta", Some(petal)) => {
                let reached_whole = self.petals.lock().await.get(&petal) == Some(&PetalKind::Tendril);
                let delta = msg.get("delta").cloned().unwrap_or(json!({}));
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => self.unfurl(petal, PetalKind::Text, json!({"text": delta.get("text")})).await,
                    "thinking_delta" => self.unfurl(petal, PetalKind::Sepal, json!({"text": delta.get("thinking")})).await,
                    "signature_delta" => self.unfurl(petal, PetalKind::Sepal, json!({"propolis": delta.get("signature")})).await,
                    "input_json_delta" if !reached_whole => {
                        self.unfurl(petal, PetalKind::Tendril, json!({"sap": delta.get("partial_json")})).await
                    }
                    _ => {}
                }
            }
            ("content_block_stop", Some(petal)) => {
                let kind = self.petals.lock().await.remove(&petal);
                if let Some(kind) = kind.filter(|k| *k != PetalKind::Tendril) {
                    self.unfurl(petal, kind, json!({"closed": true})).await;
                }
            }
            ("result", _) => {
                let subtype = msg.get("subtype").and_then(Value::as_str).unwrap_or("success");
                let nectar = msg.get("usage").cloned().unwrap_or(Value::Null);
                let is_error = msg.get("is_error").and_then(Value::as_bool).unwrap_or(false)
                    || subtype.starts_with("error");
                if is_error {
                    let detail = msg.get("result").and_then(Value::as_str)
                        .or_else(|| msg.get("error").and_then(Value::as_str))
                        .filter(|s| !s.is_empty())
                        .unwrap_or("claude returned an error result with no detail (check auth / model / credit)");
                    warn!(sid = %self.sid, subtype, detail, "nurse.result.error");
                    self.send(json!({
                        "chi": "error",
                        "sid": self.sid,
                        "code": "nurse_error",
                        "subtype": subtype,
                        "message": detail,
                        "nectar": nectar,
                    }))
                    .await;
                } else {
                    self.send(json!({
                        "chi": "wilt",
                        "sid": self.sid,
                        "reason": subtype,
                        "nectar": nectar,
                    }))
                    .await;
                }
                self.wilted.store(true, Ordering::SeqCst);
            }
            _ => {}
        }
    }

    async fn unfurl(&self, petal: u64, kind: PetalKind, grew: Value) {
        let mut body = json!({"chi": "unfurl", "sid": self.sid, "petal": petal, "kind": kind.as_str()});
        if let (Some(body), Value::Object(grew)) = (body.as_object_mut(), grew) {
            body.extend(grew);
        }
        self.send(body).await;
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
            wilted: Arc::new(AtomicBool::new(false)),
            petals: Arc::new(Mutex::new(BTreeMap::new())),
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
