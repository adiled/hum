//! Nest — the roost pool. Mirrors TS `nest/nest.ts` (Nest class).
//!
//! Owns a map of `pool_key -> Roost`, dispatches stdin writes (`murmur`,
//! `reply`, `interrupt`), evicts on idle, enforces `max_procs`, and routes
//! events from each roost to a set of `Listener`s. Stream parsing — the
//! dispatchLine switch — lives here so listeners get typed petal callbacks.
//!
//! v0 simplifications:
//!   - no `needsRespawn` handling (no Hum table in the rust crate)
//!   - no permission ask hold (the daemon binary owns permits)
//!   - no fading-roost coordination
//!   - drift/drone hooks omitted

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, trace};

use crate::{encode_cancel, encode_prompt, encode_tool_result, Listener, Perch, SpawnSpec, Roost};

pub struct NestConfig {
    pub max_procs: usize,
    pub idle_timeout: Duration,
}

impl Default for NestConfig {
    fn default() -> Self {
        Self {
            max_procs: 8,
            idle_timeout: Duration::from_secs(300),
        }
    }
}

struct RoostSlot {
    roost: Roost,
    listeners: Mutex<HashMap<String, Arc<dyn Listener>>>,
    active_sid: Mutex<Option<String>>,
    #[allow(dead_code)]
    pool_key: String,
    /// Held so the dispatch task drops cleanly when the slot is removed.
    _dispatch: JoinHandle<()>,
    idle_handle: Mutex<Option<JoinHandle<()>>>,
}

pub struct Nest {
    cfg: NestConfig,
    perch_pipe: Arc<dyn Perch>,
    perch_pty: Arc<dyn Perch>,
    slots: RwLock<HashMap<String, Arc<RoostSlot>>>,
}

impl Nest {
    pub fn new(cfg: NestConfig, pipe: Arc<dyn Perch>, pty: Arc<dyn Perch>) -> Self {
        Self {
            cfg,
            perch_pipe: pipe,
            perch_pty: pty,
            slots: RwLock::new(HashMap::new()),
        }
    }

    /// Subscribe `listener` to the roost at `pool_key`, spawning one if absent.
    /// `use_pty` picks the perch. Mirrors TS `awaken`.
    pub async fn awaken(
        self: &Arc<Self>,
        pool_key: &str,
        listener: Arc<dyn Listener>,
        spec: SpawnSpec,
        use_pty: bool,
    ) -> Result<()> {
        // Fast path: existing roost.
        {
            let slots = self.slots.read().await;
            if let Some(slot) = slots.get(pool_key) {
                slot.listeners
                    .lock()
                    .await
                    .insert(listener.session_id().to_string(), listener.clone());
                if let Some(h) = slot.idle_handle.lock().await.take() {
                    h.abort();
                }
                listener.on_petal("stream_start", json!({})).await;
                return Ok(());
            }
        }

        // Spawn path. Enforce max_procs; evict an idle slot if needed.
        self.maybe_evict_one().await;

        let perch = if use_pty {
            self.perch_pty.clone()
        } else {
            self.perch_pipe.clone()
        };
        let roost = perch.spawn(spec).await?;
        let ephemeral = roost.ephemeral;
        let pool_key_owned = pool_key.to_string();

        // Move the events receiver out for the dispatch task.
        let events = roost.events.clone();
        let slot_pre = SlotInner {
            pool_key: pool_key_owned.clone(),
            listeners: Arc::new(Mutex::new(HashMap::new())),
            active_sid: Arc::new(Mutex::new(None)),
            ephemeral,
        };
        slot_pre
            .listeners
            .lock()
            .await
            .insert(listener.session_id().to_string(), listener.clone());

        let dispatch = tokio::spawn(dispatch_loop(slot_pre.clone(), events));

        let slot = Arc::new(RoostSlot {
            roost,
            listeners: Mutex::new({
                let mut m = HashMap::new();
                m.insert(listener.session_id().to_string(), listener.clone());
                m
            }),
            active_sid: Mutex::new(None),
            pool_key: pool_key_owned.clone(),
            _dispatch: dispatch,
            idle_handle: Mutex::new(None),
        });

        self.slots
            .write()
            .await
            .insert(pool_key_owned.clone(), slot);

        info!(target: "nest", pool_key = %pool_key_owned, ephemeral, "nest.awakened");
        listener.on_petal("stream_start", json!({})).await;
        Ok(())
    }

    /// Send a user prompt to the active roost (TS `murmur`).
    pub async fn murmur(&self, session_id: &str, pool_key: &str, content: &str) -> Result<()> {
        let slots = self.slots.read().await;
        let slot = slots.get(pool_key).ok_or_else(|| anyhow!("no roost"))?;
        *slot.active_sid.lock().await = Some(session_id.to_string());
        slot.roost
            .stdin
            .send(encode_prompt(content))
            .await
            .map_err(|e| anyhow!("stdin closed: {e}"))?;
        trace!(target: "nest", %pool_key, sid = %session_id, "nest.murmured");
        Ok(())
    }

    /// Send a tool_result back to the active roost (TS `reply`).
    pub async fn reply(
        &self,
        session_id: &str,
        pool_key: &str,
        tool_use_id: &str,
        result: &str,
    ) -> Result<()> {
        let slots = self.slots.read().await;
        let slot = slots.get(pool_key).ok_or_else(|| anyhow!("no roost"))?;
        *slot.active_sid.lock().await = Some(session_id.to_string());
        slot.roost
            .stdin
            .send(encode_tool_result(tool_use_id, result))
            .await
            .map_err(|e| anyhow!("stdin closed: {e}"))?;
        Ok(())
    }

    /// Mid-turn interrupt. Pipe-mode only; PTY ignores per TS.
    pub async fn interrupt(&self, pool_key: &str, request_id: &str) -> Result<()> {
        let slots = self.slots.read().await;
        let slot = slots.get(pool_key).ok_or_else(|| anyhow!("no roost"))?;
        if slot.roost.ephemeral {
            trace!(target: "nest", %pool_key, "pty.stdin.ignored type=control_cancel_request");
            return Ok(());
        }
        slot.roost
            .stdin
            .send(encode_cancel(request_id))
            .await
            .map_err(|e| anyhow!("stdin closed: {e}"))?;
        Ok(())
    }

    /// Detach `session_id` from `pool_key`. If no listeners remain and an
    /// idle_timeout is configured, schedule eviction.
    pub async fn hush(self: &Arc<Self>, session_id: &str, pool_key: &str) {
        let slots = self.slots.read().await;
        let Some(slot) = slots.get(pool_key).cloned() else { return };
        drop(slots);
        slot.listeners.lock().await.remove(session_id);
        {
            let mut active = slot.active_sid.lock().await;
            if active.as_deref() == Some(session_id) {
                *active = None;
            }
        }

        if !slot.listeners.lock().await.is_empty() {
            return;
        }
        if self.cfg.idle_timeout.is_zero() {
            return;
        }

        let nest_w = Arc::downgrade(self);
        let pk = pool_key.to_string();
        let timeout = self.cfg.idle_timeout;
        let handle = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let Some(nest) = nest_w.upgrade() else { return };
            let mut slots = nest.slots.write().await;
            if let Some(slot) = slots.get(&pk) {
                if slot.listeners.lock().await.is_empty() {
                    trace!(target: "nest", pool_key = %pk, "nest.idle");
                    (slot.roost.kill)();
                    slots.remove(&pk);
                }
            }
        });
        *slot.idle_handle.lock().await = Some(handle);
    }

    /// Force-kill a roost (TS `fell` when last listener gone).
    pub async fn fell(&self, pool_key: &str) {
        let mut slots = self.slots.write().await;
        if let Some(slot) = slots.remove(pool_key) {
            trace!(target: "nest", %pool_key, "nest.felled");
            (slot.roost.kill)();
        }
    }

    /// Kill every roost (TS `silence`).
    pub async fn silence(&self) {
        let mut slots = self.slots.write().await;
        for (_, slot) in slots.drain() {
            (slot.roost.kill)();
        }
    }

    async fn maybe_evict_one(&self) {
        let mut slots = self.slots.write().await;
        if slots.len() < self.cfg.max_procs {
            return;
        }
        let mut evict_key: Option<String> = None;
        for (key, slot) in slots.iter() {
            let listeners = slot.listeners.lock().await;
            let active = slot.active_sid.lock().await;
            if listeners.is_empty() && active.is_none() {
                evict_key = Some(key.clone());
                break;
            }
        }
        if let Some(k) = evict_key {
            if let Some(slot) = slots.remove(&k) {
                trace!(target: "nest", pool_key = %k, "nest.evicted reason=maxProcs");
                (slot.roost.kill)();
            }
        }
    }
}

/// Lightweight handle the dispatch task uses to find listeners. Avoids
/// looping in `RoostSlot` (which holds the JoinHandle of this task — would
/// be a self-reference).
#[derive(Clone)]
struct SlotInner {
    pool_key: String,
    listeners: Arc<Mutex<HashMap<String, Arc<dyn Listener>>>>,
    active_sid: Arc<Mutex<Option<String>>>,
    ephemeral: bool,
}

async fn dispatch_loop(slot: SlotInner, events: Arc<Mutex<tokio::sync::mpsc::Receiver<Value>>>) {
    loop {
        let next = {
            let mut guard = events.lock().await;
            guard.recv().await
        };
        let Some(mut msg) = next else { break };

        // Flatten {type:"stream_event", event:{...}} → {...}
        if msg.get("type").and_then(|v| v.as_str()) == Some("stream_event") {
            if let Some(inner) = msg.get("event").cloned() {
                msg = inner;
            }
        }

        let mtype = msg.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
        trace!(target: "nest", pool_key = %slot.pool_key, ty = %mtype, "stream.msg.received");

        // system.init — broadcast to every listener so they learn the
        // upstream Claude session_id and the model.
        if mtype == "system" && msg.get("subtype").and_then(|v| v.as_str()) == Some("init") {
            let sid = msg.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let model = msg.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tools: Vec<String> = msg
                .get("tools")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let listeners = slot.listeners.lock().await.clone();
            for (_, l) in listeners {
                l.on_roost(&sid, &model, tools.clone()).await;
            }
            continue;
        }

        // Pick the listener: active sid first, else any.
        let listener = {
            let active = slot.active_sid.lock().await.clone();
            let map = slot.listeners.lock().await;
            active
                .as_ref()
                .and_then(|sid| map.get(sid).cloned())
                .or_else(|| map.values().next().cloned())
        };
        let Some(listener) = listener else { continue };

        match mtype.as_str() {
            "content_block_start" => {
                let idx = msg.get("index").cloned().unwrap_or(Value::Null);
                let block = msg.get("content_block").cloned().unwrap_or_else(|| json!({}));
                let bt = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match bt {
                    "thinking" => listener.on_petal("reasoning_start", json!({"id": idx})).await,
                    "text" => listener.on_petal("text_start", json!({"id": idx})).await,
                    "tool_use" => {
                        listener
                            .on_petal(
                                "tool_input_start",
                                json!({
                                    "toolCallId": block.get("id"),
                                    "toolName": block.get("name"),
                                }),
                            )
                            .await;
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = msg.get("delta").cloned().unwrap_or_else(|| json!({}));
                match delta.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                    "thinking_delta" => {
                        listener
                            .on_petal("reasoning_delta", json!({"delta": delta.get("thinking")}))
                            .await
                    }
                    "text_delta" => {
                        listener
                            .on_petal("text_delta", json!({"delta": delta.get("text")}))
                            .await
                    }
                    "input_json_delta" => {
                        listener
                            .on_petal(
                                "tool_input_delta",
                                json!({"partialJson": delta.get("partial_json")}),
                            )
                            .await
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                listener
                    .on_petal("content_block_stop", json!({"blockIdx": msg.get("index")}))
                    .await;
            }
            "assistant" => {
                if let Some(content) = msg
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            listener
                                .on_petal(
                                    "tool_call",
                                    json!({
                                        "toolCallId": block.get("id"),
                                        "toolName": block.get("name"),
                                        "input": block.get("input"),
                                    }),
                                )
                                .await;
                        }
                    }
                }
            }
            "user" => {
                if let Some(content) = msg
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                            let tool_use_id =
                                block.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
                            let result = match block.get("content") {
                                Some(Value::String(s)) => s.clone(),
                                Some(Value::Array(parts)) => parts
                                    .iter()
                                    .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            listener
                                .on_petal(
                                    "tool_result",
                                    json!({"toolUseId": tool_use_id, "result": result}),
                                )
                                .await;
                        }
                    }
                }
            }
            "result" => {
                let finish = msg
                    .get("stop_reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("stop")
                    .to_string();
                let usage = msg.get("usage").cloned();
                let meta = json!({
                    "sessionId": msg.get("session_id"),
                    "cost": msg.get("total_cost_usd"),
                });
                listener.on_wilt(&finish, usage, meta).await;
                let mut active = slot.active_sid.lock().await;
                if let Some(sid) = active.take() {
                    slot.listeners.lock().await.remove(&sid);
                }
                if slot.ephemeral {
                    trace!(target: "nest", pool_key = %slot.pool_key, "nest.turn.end.kept-alive");
                }
            }
            _ => {}
        }
    }
    trace!(target: "nest", pool_key = %slot.pool_key, "nest.dispatch.eof");
}
