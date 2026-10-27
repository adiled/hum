//! `serve_nurse` — wraps a Nurse impl into a standalone process that
//! handshakes with humd via thrum and grows the blooms it is sent.
//!
//! The nurse process owns its own lifecycle; humd is just a router. The
//! wire contract:
//!
//! - **Hello**: announce the hive it belongs to, its role, and the models
//!   it serves. humd routes to it only once that hive is installed there.
//! - **Prompt in**: humd forwards `chi:"prompt"` tones whose `modelId`
//!   matches one of the nurse's advertised models; a prompt that names
//!   none is grown with the first. The nurse calls `Nurse::raise(egg)`,
//!   the egg carrying the prompt, then feeds the prompt tone to the cell.
//!   The prompt's `tools` are what its cell may reach, offered over the
//!   sigil's [`mcp`](crate::mcp) path, the egg's `mcp_url`: the sid as the
//!   wire carries it, so the tools a cell lists are its sigil's and its
//!   reaches carry its sigil's sid.
//! - **A live cell**: a cell is raised for how its blooms grow, its
//!   prompt's `trellis` and `sepal`. It is fed the next prompt of its
//!   sigil only while it is not cancelled, its nurse is not
//!   [`ephemeral`], and that prompt grows the same way, has words, and its
//!   `graft` holds no bloom the cell did not grow; otherwise a fresh cell
//!   is raised. A sigil raises one cell at a time: its next prompt waits
//!   for the cell being raised. A cell that outlives its nest's
//!   `bounds.wallClockMs` is evicted.
//! - **Growing on**: a prompt with no words grows on from its graft, so it
//!   goes only to a nurse that [`grows_on`]; any other refuses it with
//!   `cannot-grow-on`, and a prompt with neither words nor a graft is
//!   refused as `empty-prompt`.
//! - **The bloom out**: a cell gives back the tones of what it grew,
//!   unfurls, a wilt or an error, pulses, and the nest sends them on as its
//!   sigil's. A cell that ends without wilting or erring wilts its bloom,
//!   with its exit code.
//! - **Cancel**: `chi:"cancel"` is fed to the cell, then stills it.
//! - **Tendril result**: `chi:"tendril-result"` answers the cell's MCP
//!   call by its `callId`; one the MCP bridge is not waiting on answers a
//!   call the cell made outside its sigil's tools, and is fed to the cell.
//! - **Cleanup**: `chi:"cleanup"` forgets the sigil's tools.
//!
//! [`grows_on`]: crate::Nurse::grows_on
//! [`ephemeral`]: crate::Nurse::ephemeral
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

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use lru::LruCache;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{info, trace, warn};

use crate::mcp::{self, Bridge, ToolDef};
use crate::{Egg, NestConfig, Nurse};
use thrum_core::Role;
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
    let first_model = advert.models.first().cloned().unwrap_or_default();

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
    let gates = Arc::new(Gates::default());

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
                let gates = gates.clone();
                let mcp_url = mcp_url.clone();
                let nest = nest.clone();
                let home = home.clone();
                let first_model = first_model.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_prompt(nurse, write_half, cells, gates, nest, mcp_url, home, first_model, tone).await {
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
                    tokio::spawn(async move {
                        let mut r = cells.lock().await;
                        if let Some(bundle) = r.get(&sid) {
                            let _ = bundle.feed.send(tone).await;
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
                let resolved_by_bridge = tone.get("callId").and_then(Value::as_str)
                    .is_some_and(|cid| bridge.resolve(cid, tone.clone()));
                if !resolved_by_bridge && !sid.is_empty() {
                    let cells = cells.clone();
                    tokio::spawn(async move {
                        let mut r = cells.lock().await;
                        if let Some(bundle) = r.get(&sid) {
                            let _ = bundle.feed.send(tone).await;
                        }
                    });
                }
            }
            "breath" | "echo" | "" => {}
            other => {
                trace!(chi = other, "nurse.unknown.chi");
            }
        }
    }
    Ok(())
}

trait Sounds: Clone + Send + Sync + 'static {
    fn send(&self, tone: Value);
}

impl Sounds for hum_thrum::Sender {
    fn send(&self, tone: Value) {
        hum_thrum::Sender::send(self, tone);
    }
}

#[derive(Default)]
struct Gates(parking_lot::Mutex<HashMap<String, Arc<Mutex<()>>>>);

impl Gates {
    fn of(&self, sid: &str) -> Arc<Mutex<()>> {
        let mut gates = self.0.lock();
        gates.retain(|_, gate| Arc::strong_count(gate) > 1);
        gates.entry(sid.to_string()).or_default().clone()
    }
}

static RAISED: AtomicU64 = AtomicU64::new(0);

struct CellBundle {
    id: u64,
    feed: mpsc::Sender<Value>,
    cancel: tokio_util::sync::CancellationToken,
    wilted: Arc<AtomicBool>,
    last_touched: Arc<AtomicU64>,
    grown: Arc<AtomicU64>,
    grows: String,
}

impl Drop for CellBundle {
    fn drop(&mut self) { self.cancel.cancel(); }
}

async fn pulse(
    write_half: &impl Sounds,
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

fn grows_of(body: &thrum_core::PromptBody) -> String {
    json!({ "trellis": body.trellis, "sepal": body.sepal }).to_string()
}

#[allow(clippy::too_many_arguments)]
async fn handle_prompt<N: Nurse + 'static, W: Sounds>(
    nurse: Arc<N>,
    write_half: W,
    cells: Arc<Mutex<LruCache<String, CellBundle>>>,
    gates: Arc<Gates>,
    nest: Arc<NestConfig>,
    mcp_url: String,
    home: String,
    first_model: String,
    tone: Value,
) -> Result<()> {
    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
    if sid.is_empty() { anyhow::bail!("prompt.no-sid"); }
    let body: thrum_core::PromptBody = serde_json::from_value(tone.clone()).unwrap_or_default();
    let model = body.model_id.clone().filter(|m| !m.is_empty()).unwrap_or(first_model);
    let grafted = body.graft.as_ref().map_or(0, Vec::len) as u64;
    let grows = grows_of(&body);
    let worded = body.content.as_ref().is_some_and(|c| !c.is_empty()) || body.text.as_ref().is_some_and(|t| !t.is_empty());
    if !worded {
        let refusal = if grafted == 0 {
            Some(("empty-prompt", "a prompt with no words grows on from its graft, and this one carries none"))
        } else if !nurse.grows_on() {
            Some(("cannot-grow-on", "this nurse grows a bloom only from new words, and cannot grow one on from its graft"))
        } else {
            None
        };
        if let Some((code, message)) = refusal {
            write_half.send(json!({ "chi": "error", "sid": sid, "code": code, "message": message }));
            return Ok(());
        }
    }

    let gate = gates.of(&sid);
    let _raising = gate.lock().await;
    {
        let mut g = cells.lock().await;
        let stale = g.peek(&sid).is_some_and(|bundle| {
            nurse.ephemeral() || bundle.cancel.is_cancelled() || !worded || bundle.grows != grows
                || (grafted > 0 && bundle.grown.load(Ordering::SeqCst) != grafted)
        });
        if stale {
            trace!(sid = %sid, grafted, "nurse.cell.raised-anew");
            g.pop(&sid);
        }
        if let Some(bundle) = g.get(&sid) {
            bundle.wilted.store(false, Ordering::SeqCst);
            bundle.last_touched.store(now_ms(), Ordering::SeqCst);
            let feed = bundle.feed.clone();
            drop(g);
            feed.send(tone).await
                .map_err(|e| anyhow::anyhow!("the reused cell stopped listening: {e}"))?;
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
    let mut egg = Egg::new(hum_sid, model, home);
    egg.prompt = body;
    egg.mcp_url = Some(format!("{mcp_url}/s/{}", path_safe(&sid)));
    egg.bounds = nest.bounds.clone();

    let cell = match nurse.raise(egg).await {
        Ok(cell) => cell,
        Err(e) => {
            write_half.send(json!({ "chi": "error", "sid": sid, "code": "cell-not-raised", "message": format!("the cell could not be raised: {e}") }));
            return Ok(());
        }
    };
    let feed = cell.feed.clone();
    let events = cell.mmm.clone();
    let cancel = cell.silence.clone();
    let wilted = Arc::new(AtomicBool::new(false));
    let last_touched = Arc::new(AtomicU64::new(now_ms()));
    let grown = Arc::new(AtomicU64::new(grafted));
    let id = RAISED.fetch_add(1, Ordering::SeqCst);

    let dispatch = {
        let sid = sid.clone();
        let write_half = write_half.clone();
        let wilted = wilted.clone();
        let grown = grown.clone();
        let last_touched = last_touched.clone();
        tokio::spawn(async move {
            let mut events = events.lock().await;
            while let Some(mut grew) = events.recv().await {
                last_touched.store(now_ms(), Ordering::SeqCst);
                let chi = grew.get("chi").and_then(Value::as_str).unwrap_or("").to_string();
                if chi.is_empty() {
                    continue;
                }
                if chi == "wilt" {
                    grown.fetch_add(1, Ordering::SeqCst);
                }
                if chi == "wilt" || chi == "error" {
                    wilted.store(true, Ordering::SeqCst);
                }
                if let Some(obj) = grew.as_object_mut() {
                    obj.insert("sid".into(), Value::String(sid.clone()));
                }
                write_half.send(grew);
            }
            trace!(sid = %sid, "nurse.dispatch.exit");
        })
    };

    if feed.send(tone).await.is_err() {
        warn!(sid = %sid, "nurse.cell.feed.closed");
    }

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
                    pop_own(&mut *cells_for_idle.lock().await, &sid_for_idle, id)
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
            id,
            feed,
            cancel: cancel.clone(),
            wilted: wilted.clone(),
            last_touched: last_touched.clone(),
            grown: grown.clone(),
            grows,
        });
        g.len()
    };
    pulse(&write_half, "cell-spawned", nest.headroom(live)).await;

    if let Some(ms) = nest.bounds.wall_clock_ms {
        let outlived = outlived(cells.clone(), sid.clone(), id, cancel, std::time::Duration::from_millis(ms));
        let write_for_wall = write_half.clone();
        let nest_for_wall = nest.clone();
        let sid_for_wall = sid.clone();
        tokio::spawn(async move {
            if let Some(live) = outlived.await {
                warn!(sid = %sid_for_wall, wall_clock_ms = ms, "nurse.cell.wall-clock.kill");
                pulse(&write_for_wall, "cell-evicted", nest_for_wall.headroom(live)).await;
            }
        });
    }

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
            pop_own(&mut *cells_for_cleanup.lock().await, &sid_for_cleanup, id)
        };
        if let Some(live) = live {
            pulse(&write_for_cleanup, "cell-died", nest_for_cleanup.headroom(live)).await;
        }
        trace!(sid = %sid_for_cleanup, exit_code, "nurse.cell.exit");
    });

    Ok(())
}

fn pop_own(cells: &mut LruCache<String, CellBundle>, sid: &str, id: u64) -> Option<usize> {
    if cells.peek(sid)?.id != id {
        return None;
    }
    cells.pop(sid);
    Some(cells.len())
}

async fn outlived(
    cells: Arc<Mutex<LruCache<String, CellBundle>>>,
    sid: String,
    id: u64,
    cancel: tokio_util::sync::CancellationToken,
    wall_clock: std::time::Duration,
) -> Option<usize> {
    tokio::select! {
        _ = cancel.cancelled() => return None,
        _ = tokio::time::sleep(wall_clock) => {}
    }
    pop_own(&mut *cells.lock().await, &sid, id)
}

fn path_safe(sid: &str) -> String {
    sid.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn bundle(cancel: CancellationToken) -> CellBundle {
        let (tx_in, _rx_in) = mpsc::channel::<Value>(1);
        CellBundle {
            id: RAISED.fetch_add(1, Ordering::SeqCst),
            feed: tx_in,
            cancel,
            wilted: Arc::new(AtomicBool::new(false)),
            last_touched: Arc::new(AtomicU64::new(0)),
            grown: Arc::new(AtomicU64::new(0)),
            grows: String::new(),
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
    async fn a_cell_outliving_its_wall_clock_is_evicted() {
        let cells = Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(2).unwrap())));
        let cancel = CancellationToken::new();
        let cell = bundle(cancel.clone());
        let id = cell.id;
        cells.lock().await.put("a".to_string(), cell);
        let live = outlived(cells.clone(), "a".into(), id, cancel.clone(), std::time::Duration::from_millis(10)).await;
        assert_eq!(live, Some(0));
        assert!(cancel.is_cancelled(), "the outlived cell is stilled");
        assert!(cells.lock().await.peek("a").is_none());
    }

    #[tokio::test]
    async fn a_wall_clock_leaves_the_cell_raised_after_its_own() {
        let cells = Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(2).unwrap())));
        let first = CancellationToken::new();
        let cell = bundle(first.clone());
        let id = cell.id;
        cells.lock().await.put("a".to_string(), cell);
        let wall = tokio::spawn(outlived(cells.clone(), "a".into(), id, first.clone(), std::time::Duration::from_millis(50)));
        let next = CancellationToken::new();
        {
            let mut g = cells.lock().await;
            g.pop("a");
            g.put("a".to_string(), bundle(next.clone()));
        }
        assert_eq!(wall.await.unwrap(), None);
        assert!(!next.is_cancelled());
        assert!(cells.lock().await.peek("a").is_some());
    }

    #[tokio::test]
    async fn an_old_cells_exit_leaves_the_cell_raised_after_it() {
        use command_group::AsyncCommandGroup;
        let child = tokio::process::Command::new("sleep").arg("60")
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .group_spawn().expect("spawn sleep");
        let (emerged, stilled) = crate::lifecycle::tend(child);
        let old = bundle(stilled);
        let old_id = old.id;
        let mut cells: LruCache<String, CellBundle> = LruCache::new(NonZeroUsize::new(2).unwrap());
        cells.put("a".into(), old);

        cells.pop("a");
        let next = CancellationToken::new();
        let successor = bundle(next.clone());
        let next_id = successor.id;
        cells.put("a".into(), successor);
        tokio::time::timeout(std::time::Duration::from_secs(6), emerged).await
            .expect("the old cell exits").expect("exit code");

        assert_eq!(pop_own(&mut cells, "a", old_id), None);
        assert!(!next.is_cancelled(), "the old cell's exit leaves its successor alive");
        assert!(cells.peek("a").is_some());
        assert_eq!(pop_own(&mut cells, "a", next_id), Some(0));
        assert!(next.is_cancelled());
    }

    #[derive(Clone, Default)]
    struct Heard(Arc<std::sync::Mutex<Vec<Value>>>);

    impl Sounds for Heard {
        fn send(&self, tone: Value) { self.0.lock().unwrap().push(tone); }
    }

    struct Raised {
        stilled: CancellationToken,
        fed: std::sync::Mutex<mpsc::Receiver<Value>>,
        _alive: (mpsc::Sender<Value>, tokio::sync::oneshot::Sender<i32>),
    }

    impl Raised {
        fn fed(&self) -> Vec<String> {
            let mut fed = self.fed.lock().unwrap();
            std::iter::from_fn(|| fed.try_recv().ok())
                .filter_map(|tone| tone.get("text").and_then(Value::as_str).map(str::to_string))
                .collect()
        }
    }

    #[derive(Default)]
    struct Fake {
        ephemeral: bool,
        raising: std::time::Duration,
        raised: std::sync::Mutex<Vec<Arc<Raised>>>,
    }

    #[async_trait::async_trait]
    impl Nurse for Fake {
        fn ephemeral(&self) -> bool { self.ephemeral }

        async fn raise(&self, _egg: Egg) -> Result<crate::Cell> {
            tokio::time::sleep(self.raising).await;
            let (tx_feed, rx_feed) = mpsc::channel(8);
            let (tx_mmm, rx_mmm) = mpsc::channel(8);
            let (tx_exit, rx_exit) = tokio::sync::oneshot::channel();
            let silence = CancellationToken::new();
            self.raised.lock().unwrap().push(Arc::new(Raised {
                stilled: silence.clone(),
                fed: std::sync::Mutex::new(rx_feed),
                _alive: (tx_mmm, tx_exit),
            }));
            Ok(crate::Cell { mark: None, feed: tx_feed, mmm: Arc::new(Mutex::new(rx_mmm)), emerged: rx_exit, ephemeral: self.ephemeral, silence })
        }
    }

    struct Hosted {
        nurse: Arc<Fake>,
        cells: Arc<Mutex<LruCache<String, CellBundle>>>,
        gates: Arc<Gates>,
    }

    impl Hosted {
        fn new(nurse: Fake) -> Self {
            Self {
                nurse: Arc::new(nurse),
                cells: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(8).unwrap()))),
                gates: Arc::new(Gates::default()),
            }
        }

        fn prompt(&self, text: &str) -> impl std::future::Future<Output = Result<()>> + 'static {
            handle_prompt(
                self.nurse.clone(), Heard::default(), self.cells.clone(), self.gates.clone(),
                Arc::new(NestConfig::default()), "http://127.0.0.1:0".into(),
                std::env::temp_dir().to_string_lossy().into_owned(), "m".into(),
                json!({ "chi": "prompt", "sid": "s", "text": text }),
            )
        }

        fn raised(&self) -> Vec<Arc<Raised>> {
            self.nurse.raised.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn a_live_cell_is_fed_its_sigils_next_prompt() {
        let nest = Hosted::new(Fake::default());
        nest.prompt("one").await.unwrap();
        nest.prompt("two").await.unwrap();
        let raised = nest.raised();
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].fed(), ["one", "two"]);
    }

    #[tokio::test]
    async fn an_ephemeral_nurses_cell_grows_one_prompt() {
        let nest = Hosted::new(Fake { ephemeral: true, ..Fake::default() });
        nest.prompt("one").await.unwrap();
        nest.prompt("two").await.unwrap();
        let raised = nest.raised();
        assert_eq!(raised.len(), 2, "each prompt raises its own cell");
        assert!(raised[0].stilled.is_cancelled());
        assert_eq!(raised[0].fed(), ["one"]);
        assert_eq!(raised[1].fed(), ["two"]);
    }

    #[tokio::test]
    async fn two_prompts_on_one_sid_raise_one_cell() {
        let nest = Hosted::new(Fake { raising: std::time::Duration::from_millis(50), ..Fake::default() });
        let one = tokio::spawn(nest.prompt("one"));
        let two = tokio::spawn(nest.prompt("two"));
        one.await.unwrap().unwrap();
        two.await.unwrap().unwrap();
        let raised = nest.raised();
        assert_eq!(raised.len(), 1, "the second prompt waits for the cell the first raises");
        assert!(!raised[0].stilled.is_cancelled());
        let mut fed = raised[0].fed();
        fed.sort();
        assert_eq!(fed, ["one", "two"]);
    }

    #[tokio::test]
    async fn a_cancelled_cell_is_not_fed_again() {
        let nest = Hosted::new(Fake::default());
        nest.prompt("one").await.unwrap();
        nest.cells.lock().await.peek("s").expect("a live cell").cancel.cancel();
        nest.prompt("two").await.unwrap();
        let raised = nest.raised();
        assert_eq!(raised.len(), 2, "a fresh cell grows the prompt after a cancel");
        assert_eq!(raised[0].fed(), ["one"]);
        assert_eq!(raised[1].fed(), ["two"]);
    }

    #[tokio::test]
    async fn a_sigils_gate_is_forgotten_once_none_waits_on_it() {
        let gates = Gates::default();
        let held = gates.of("a");
        drop(gates.of("b"));
        let again = gates.of("a");
        assert!(Arc::ptr_eq(&held, &again));
        assert_eq!(gates.0.lock().len(), 1, "b's gate went with its last holder");
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
