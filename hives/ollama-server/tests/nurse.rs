//! The nurse as its nest runs it: a prompt fed to a cell still live on its
//! sigil is grown, and a cell that outlives its wall clock mid-bloom wilts
//! the bloom, the sigil's next prompt growing in a fresh cell.
//!
//! Ollama here is a stand-in that says `heard <words>. ` and, for words
//! that start `slow <ms>`, waits that long before it is done.

mod common;

use std::time::{Duration, Instant};

use axum::{routing::post, Json, Router};
use common::*;
use hum_thrum::{Bee, Event, Options};
use serde_json::{json, Value};
use thrum_core::Role;

const GROWN: &str = "ollama-model";
const WALL_CLOCK_MS: u64 = 1500;

async fn ollama() -> String {
    let app = Router::new().route("/api/chat", post(|Json(body): Json<Value>| async move {
        let said = body["messages"].as_array().and_then(|m| m.last()).and_then(|m| m["content"].as_str()).unwrap_or("").to_string();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(4);
        tokio::spawn(async move {
            let _ = tx.send(Ok(format!("{}\n", json!({"message": {"content": format!("heard {said}. ")}, "done": false})))).await;
            if let Some(ms) = said.strip_prefix("slow ").and_then(|w| w.split(' ').next()).and_then(|ms| ms.parse().ok()) {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            let _ = tx.send(Ok(format!("{}\n", json!({"done": true, "done_reason": "stop", "prompt_eval_count": 1, "eval_count": 1})))).await;
        });
        axum::body::Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn nurse(url: &str) -> tokio::process::Child {
    configure("ollama-server", json!({"nest": {"bounds": {"wallClockMs": WALL_CLOCK_MS}}}));
    spawn_nurse(&[("OLLAMA_NURSE_URL", url), ("OLLAMA_NURSE_MODELS", GROWN), ("OLLAMA_NURSE_MODEL", "")]).await
}

struct Probe {
    bee: Bee,
}

impl Probe {
    async fn link() -> Probe {
        world();
        let bee = Bee::link(Options::new("ollama-server", "probe", Role::Forager).hello(json!({"version": "1.0.0"}))).await.unwrap();
        let mut probe = Probe { bee };
        let since = Instant::now();
        loop {
            assert!(since.elapsed() < WAIT, "humd never breathed that a nurse grows {GROWN}");
            let breath = probe.next().await;
            if breath["chi"] == "breath" && breath["models"].as_array().is_some_and(|m| m.iter().any(|m| m["id"] == GROWN)) {
                return probe;
            }
        }
    }

    fn prompt(&self, sid: &str, words: &str) {
        self.bee.send(json!({"chi": "prompt", "sid": sid, "modelId": GROWN, "content": [{"kind": "text", "text": words}]}));
    }

    async fn next(&mut self) -> Value {
        loop {
            match tokio::time::timeout(WAIT, self.bee.next()).await.expect("a tone in time").expect("the link lives") {
                Event::Tone(tone) => return tone,
                _ => continue,
            }
        }
    }

    async fn on(&mut self, sid: &str) -> Value {
        loop {
            let tone = self.next().await;
            if tone["sid"] == sid {
                return tone;
            }
        }
    }

    async fn bloom(&mut self, sid: &str) -> (String, String) {
        let mut said = String::new();
        loop {
            let tone = self.on(sid).await;
            match tone["chi"].as_str() {
                Some("unfurl") => said.push_str(tone["text"].as_str().unwrap_or("")),
                Some("wilt") => return (said, tone["reason"].as_str().unwrap_or("").into()),
                Some("error") => return (said, format!("error {}", tone["message"])),
                _ => {}
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_cell_grows_what_its_nest_feeds_it_until_its_wall_clock() {
    let _nurse = nurse(&ollama().await).await;
    let mut probe = Probe::link().await;

    probe.prompt("fed", "slow 300 one");
    let first = probe.on("fed").await;
    assert_eq!((first["chi"].clone(), first["text"].clone()), (json!("unfurl"), json!("heard slow 300 one. ")), "the cell is live");
    probe.prompt("fed", "two");
    assert_eq!(probe.bloom("fed").await, ("".into(), "stop".into()));
    assert_eq!(probe.bloom("fed").await, ("heard two. ".into(), "stop".into()), "the prompt fed to the live cell grows next");

    let since = Instant::now();
    probe.prompt("walled", "slow 10000 three");
    assert_eq!(probe.bloom("walled").await, ("heard slow 10000 three. ".into(), "cancel".into()), "the wall clock wilts the bloom");
    let took = since.elapsed();
    assert!(took >= Duration::from_millis(WALL_CLOCK_MS) && took < Duration::from_millis(5000), "{took:?}");
    probe.prompt("walled", "four");
    assert_eq!(probe.bloom("walled").await, ("heard four. ".into(), "stop".into()), "the next prompt grows in a fresh cell");
}
