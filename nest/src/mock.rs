//! mock — a test-only WorkerBee that emits canned stream-json events.
//!
//! Sim tests can't spawn real subprocesses. `MockWorkerBee` returns a
//! [`Cell`] whose `events` channel emits a deterministic sequence shaped
//! exactly like `claude -p --output-format stream-json` would, so the
//! daemon's listener bridge fires through the same code path.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::{Cell, SpawnSpec, WorkerBee};

/// A WorkerBee that produces canned events instead of running a subprocess.
pub struct MockWorkerBee {
    /// Override the canned output text. Defaults to "HELLO".
    pub text: String,
    /// Optional artificial latency between events (Duration::ZERO default).
    pub event_delay: Duration,
    /// If true, also push a `tool_use` block. Defaults false.
    pub with_tool: bool,
}

impl Default for MockWorkerBee {
    fn default() -> Self {
        Self {
            text: "HELLO".to_string(),
            event_delay: Duration::ZERO,
            with_tool: false,
        }
    }
}

#[async_trait]
impl WorkerBee for MockWorkerBee {
    fn ephemeral(&self) -> bool { false }

    async fn spawn(&self, spec: SpawnSpec) -> Result<Cell> {
        let (tx_in, mut rx_in) = mpsc::channel::<String>(64);
        let (tx_evt, rx_evt) = mpsc::channel::<Value>(256);
        let (tx_exit, rx_exit) = oneshot::channel::<i32>();

        // stdin drain — the worker doesn't care what the daemon writes back.
        tokio::spawn(async move {
            while rx_in.recv().await.is_some() { /* /dev/null */ }
        });

        let text = self.text.clone();
        let delay = self.event_delay;
        let with_tool = self.with_tool;
        let sid = spec.sid.clone();
        let model = spec.model_id.clone();

        tokio::spawn(async move {
            // Build the sequence first so we can iterate uniformly.
            let mut events: Vec<Value> = Vec::new();
            events.push(json!({
                "type": "system",
                "subtype": "init",
                "session_id": sid,
                "model": model,
                "tools": [],
            }));
            events.push(json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text" },
            }));
            events.push(json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": text },
            }));
            events.push(json!({
                "type": "content_block_stop",
                "index": 0,
            }));
            if with_tool {
                events.push(json!({
                    "type": "content_block_start",
                    "index": 1,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_mock_1",
                        "name": "mock_tool",
                        "input": {},
                    },
                }));
                events.push(json!({
                    "type": "content_block_stop",
                    "index": 1,
                }));
            }
            events.push(json!({
                "type": "result",
                "subtype": "success",
                "stop_reason": "end_turn",
                "session_id": sid,
                "usage": {},
            }));

            for evt in events {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if tx_evt.send(evt).await.is_err() {
                    // Receiver dropped — bail; the test is gone.
                    let _ = tx_exit.send(0);
                    return;
                }
            }
            let _ = tx_exit.send(0);
        });

        let kill: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});

        Ok(Cell {
            pid: None,
            stdin: tx_in,
            events: Arc::new(Mutex::new(rx_evt)),
            exited: rx_exit,
            ephemeral: false,
            kill,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emits_canned_sequence_with_default_text() {
        let bee = MockWorkerBee::default();
        let spec = SpawnSpec::new("sid-mock", "claude-haiku-4-5", "/tmp");
        let cell = bee.spawn(spec).await.unwrap();

        let mut events = Vec::new();
        {
            let mut rx = cell.events.lock().await;
            while let Some(v) = rx.recv().await {
                events.push(v);
            }
        }

        // 5 events in default mode.
        assert_eq!(events.len(), 5);
        assert_eq!(events[0]["type"], "system");
        assert_eq!(events[0]["session_id"], "sid-mock");
        assert_eq!(events[0]["model"], "claude-haiku-4-5");

        assert_eq!(events[1]["type"], "content_block_start");
        assert_eq!(events[1]["content_block"]["type"], "text");

        assert_eq!(events[2]["type"], "content_block_delta");
        assert_eq!(events[2]["delta"]["type"], "text_delta");
        assert_eq!(events[2]["delta"]["text"], "HELLO");

        assert_eq!(events[3]["type"], "content_block_stop");

        assert_eq!(events[4]["type"], "result");
        assert_eq!(events[4]["stop_reason"], "end_turn");

        let code = cell.exited.await.unwrap();
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn custom_text_appears_in_delta() {
        let bee = MockWorkerBee { text: "ahoy world".into(), ..Default::default() };
        let spec = SpawnSpec::new("s", "m", "/");
        let cell = bee.spawn(spec).await.unwrap();
        let mut rx = cell.events.lock().await;
        let mut saw_text = None;
        while let Some(v) = rx.recv().await {
            if v["type"] == "content_block_delta" {
                saw_text = v["delta"]["text"].as_str().map(|s| s.to_string());
            }
        }
        assert_eq!(saw_text.as_deref(), Some("ahoy world"));
    }

    #[tokio::test]
    async fn with_tool_inserts_tool_use_block() {
        let bee = MockWorkerBee { with_tool: true, ..Default::default() };
        let spec = SpawnSpec::new("s", "m", "/");
        let cell = bee.spawn(spec).await.unwrap();
        let mut rx = cell.events.lock().await;
        let mut kinds = Vec::new();
        while let Some(v) = rx.recv().await {
            kinds.push(v["type"].as_str().unwrap_or("").to_string());
            if v["type"] == "content_block_start" {
                if let Some(cb) = v.get("content_block") {
                    if cb["type"] == "tool_use" {
                        assert_eq!(cb["name"], "mock_tool");
                    }
                }
            }
        }
        // Two extra blocks vs default.
        assert_eq!(kinds.len(), 7);
        assert_eq!(kinds.last().map(String::as_str), Some("result"));
    }

    #[tokio::test]
    async fn stdin_drains_silently() {
        let bee = MockWorkerBee::default();
        let cell = bee.spawn(SpawnSpec::new("s", "m", "/")).await.unwrap();
        // Should not panic / block.
        cell.stdin.send("ignored line".into()).await.unwrap();
        cell.stdin.send(crate::encode_prompt("hi")).await.unwrap();
    }

    /// End-to-end with a Listener — the daemon's bridge sees text-delta + finish.
    #[tokio::test]
    async fn listener_bridge_sees_delta_and_finish() {
        use crate::Listener;
        use std::sync::Mutex as StdMutex;

        struct Captor {
            sid: String,
            petals: StdMutex<Vec<(String, Value)>>,
            wilted: StdMutex<Option<String>>,
        }

        #[async_trait::async_trait]
        impl Listener for Captor {
            fn session_id(&self) -> &str { &self.sid }
            async fn on_petal(&self, kind: &str, payload: Value) {
                self.petals.lock().unwrap().push((kind.into(), payload));
            }
            async fn on_cell(&self, _nest_id: &str, _model: &str, _tools: Vec<String>) {}
            async fn on_wilt(&self, finish_reason: &str, _usage: Option<Value>, _meta: Value) {
                *self.wilted.lock().unwrap() = Some(finish_reason.into());
            }
            async fn on_thorn(&self, _wound: &str) {}
        }

        let captor = Arc::new(Captor {
            sid: "sid-bridge".into(),
            petals: StdMutex::new(Vec::new()),
            wilted: StdMutex::new(None),
        });
        let listener: Arc<dyn Listener> = captor.clone();

        let bee = MockWorkerBee::default();
        let cell = bee.spawn(SpawnSpec::new("sid-bridge", "claude-sonnet-4-6", "/")).await.unwrap();

        // Minimal bridge — mirrors what the daemon binary does: pump events
        // off the cell, route by `type`, and call the listener.
        let events_arc = cell.events.clone();
        let mut rx = events_arc.lock().await;
        while let Some(v) = rx.recv().await {
            let kind = v["type"].as_str().unwrap_or("").to_string();
            match kind.as_str() {
                "content_block_delta" => {
                    listener.on_petal("text_delta", v.clone()).await;
                }
                "result" => {
                    let reason = v["stop_reason"].as_str().unwrap_or("").to_string();
                    listener.on_wilt(&reason, v.get("usage").cloned(), Value::Null).await;
                }
                _ => {}
            }
        }

        let petals = captor.petals.lock().unwrap().clone();
        assert_eq!(petals.len(), 1);
        assert_eq!(petals[0].0, "text_delta");
        assert_eq!(petals[0].1["delta"]["text"], "HELLO");

        let wilted = captor.wilted.lock().unwrap().clone();
        assert_eq!(wilted.as_deref(), Some("end_turn"));
    }
}
