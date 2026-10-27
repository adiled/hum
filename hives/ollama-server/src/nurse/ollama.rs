//! The Nurse that calls Ollama's HTTP API.
//!
//! A bloom grows through `/api/chat`. The graft's blooms and then the
//! prompt's own petals become the messages, a bloom's tendrils and grasps
//! among them as tool calls and tool answers, and its images ride with its
//! words. How the bloom grows becomes Ollama's options (`temperature`,
//! `top_p` and `top_k` from `prune`, `seed`, `stop` from `frost`,
//! `num_predict` from `season`), its `format` from `trellis`, and `think`
//! from `sepal`. The reply streams as it comes: its words unfurl as a text
//! petal, its thinking as a sepal, and its counts wilt the bloom as nectar,
//! with `max_tokens` as the reason when the season ran out.
//!
//! A prompt with no words grows on from its graft through `/api/generate`:
//! a graft that is plain text, grown by no one's prompt, is the text to
//! grow on from as it stands, and a `scion` is the suffix the growth meets.
//! A graft that holds a conversation cannot grow on that way, and is
//! refused with `cannot-grow-on`.
//!
//! A cell lives for its sigil: it grows each prompt the nest feeds it, in
//! turn, each from that prompt's own graft, until the nest stills it. A
//! still wilts the growing bloom `cancel`, and a prompt still waiting is
//! told it was not grown.
//!
//! Environment, an empty value being no value:
//!   OLLAMA_NURSE_URL      Ollama API base URL (default: http://127.0.0.1:11434)
//!   OLLAMA_NURSE_MODEL    default model (default: smollm:1.7b)
//!   OLLAMA_NURSE_CTX      context length (default: 8192)

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::info;

use hum_nest::{Cell, Egg, Nurse, Propensity};
use thrum_core::{Bloom, Petal, PetalKind, PromptBody};

pub struct OllamaNurse {
    client: reqwest::Client,
    url: String,
    default_model: String,
    default_ctx: u64,
}

impl Default for OllamaNurse {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            url: crate::var("OLLAMA_NURSE_URL").unwrap_or_else(|| "http://127.0.0.1:11434".into()),
            default_model: crate::var("OLLAMA_NURSE_MODEL").unwrap_or_else(|| "smollm:1.7b".into()),
            default_ctx: crate::var("OLLAMA_NURSE_CTX").and_then(|s| s.trim().parse().ok()).unwrap_or(8192),
        }
    }
}

fn is(petal: &Petal, kind: PetalKind) -> bool {
    petal.kind == kind.as_str()
}

fn words(petals: &[Petal]) -> String {
    petals
        .iter()
        .filter(|p| is(p, PetalKind::Text))
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

fn images(petals: &[Petal]) -> Vec<String> {
    petals
        .iter()
        .filter(|p| is(p, PetalKind::Pollen))
        .filter_map(|p| p.grain.as_ref())
        .filter(|g| g.kind == "image")
        .filter_map(|g| g.data.clone())
        .collect()
}

fn said(petals: &[Petal]) -> Value {
    let mut message = json!({ "role": "user", "content": words(petals) });
    let seen = images(petals);
    if !seen.is_empty() {
        message["images"] = json!(seen);
    }
    message
}

fn grown(petals: &[Petal], into: &mut Vec<Value>) {
    let mut text = String::new();
    let mut calls: Vec<Value> = Vec::new();
    let flush = |text: &mut String, calls: &mut Vec<Value>, into: &mut Vec<Value>| {
        if text.is_empty() && calls.is_empty() {
            return;
        }
        let mut message = json!({ "role": "assistant", "content": std::mem::take(text) });
        if !calls.is_empty() {
            message["tool_calls"] = json!(std::mem::take(calls));
        }
        into.push(message);
    };
    for petal in petals {
        if is(petal, PetalKind::Text) {
            if !calls.is_empty() {
                flush(&mut text, &mut calls, into);
            }
            text.push_str(petal.text.as_deref().unwrap_or(""));
        } else if is(petal, PetalKind::Tendril) {
            calls.push(json!({ "function": {
                "name": petal.name.clone().unwrap_or_default(),
                "arguments": petal.args.clone().unwrap_or_else(|| json!({})),
            }}));
        } else if is(petal, PetalKind::Grasp) {
            flush(&mut text, &mut calls, into);
            let answer = match &petal.result {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            into.push(json!({ "role": "tool", "content": answer }));
        }
    }
    flush(&mut text, &mut calls, into);
}

pub fn messages(system: Option<&str>, graft: &[Bloom], content: &[Petal]) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(system) = system.filter(|s| !s.is_empty()) {
        out.push(json!({ "role": "system", "content": system }));
    }
    for bloom in graft {
        if let Some(content) = bloom.content.as_deref().filter(|c| !c.is_empty()) {
            out.push(said(content));
        }
        grown(bloom.grew.as_deref().unwrap_or(&[]), &mut out);
    }
    if !content.is_empty() {
        out.push(said(content));
    }
    out
}

pub fn plain_text(graft: &[Bloom]) -> Option<String> {
    let plain = graft.iter().all(|b| {
        b.content.as_deref().is_none_or(|c| c.is_empty())
            && b.grew.as_deref().unwrap_or(&[]).iter().all(|p| is(p, PetalKind::Text))
    });
    if !plain || graft.is_empty() {
        return None;
    }
    Some(graft.iter().map(|b| words(b.grew.as_deref().unwrap_or(&[]))).collect::<Vec<_>>().join(""))
}

pub fn options(prompt: &PromptBody, ctx: u64) -> Value {
    let mut o = Map::new();
    o.insert("num_ctx".into(), json!(ctx));
    if let Some(t) = prompt.temperature {
        o.insert("temperature".into(), json!(t));
    }
    if let Some(prune) = &prompt.prune {
        if let Some(p) = prune.p {
            o.insert("top_p".into(), json!(p));
        }
        if let Some(k) = prune.k {
            o.insert("top_k".into(), json!(k));
        }
    }
    if let Some(seed) = prompt.seed {
        o.insert("seed".into(), json!(seed));
    }
    if let Some(frost) = prompt.frost.as_ref().filter(|f| !f.is_empty()) {
        o.insert("stop".into(), json!(frost));
    }
    if let Some(season) = prompt.season {
        o.insert("num_predict".into(), json!(season));
    }
    Value::Object(o)
}

pub fn request(model: &str, system: Option<&str>, prompt: &PromptBody, ctx: u64) -> (&'static str, Value) {
    let graft = prompt.graft.as_deref().unwrap_or(&[]);
    let content = prompt.content.as_deref().unwrap_or(&[]);
    let opts = options(prompt, ctx);
    if content.is_empty() && prompt.text.as_deref().is_none_or(str::is_empty) {
        let text = plain_text(graft).unwrap_or_default();
        let mut body = json!({ "model": model, "prompt": text, "stream": true, "options": opts });
        match prompt.scion.as_ref() {
            Some(scion) => body["suffix"] = json!(scion),
            None => body["raw"] = json!(true),
        }
        return ("/api/generate", body);
    }
    let content: Vec<Petal> = if content.is_empty() {
        vec![Petal { kind: PetalKind::Text.as_str().into(), text: prompt.text.clone(), ..Petal::default() }]
    } else {
        content.to_vec()
    };
    let mut body = json!({
        "model": model,
        "messages": messages(system, graft, &content),
        "stream": true,
        "options": opts,
    });
    match &prompt.trellis {
        Some(Value::String(s)) if s == "json" => body["format"] = json!("json"),
        Some(schema @ Value::Object(_)) => body["format"] = schema.clone(),
        _ => {}
    }
    if let Some(sepal) = &prompt.sepal {
        body["think"] = match sepal.effort.as_deref() {
            Some("none") => json!(false),
            Some(effort @ ("low" | "medium" | "high")) => json!(effort),
            _ => json!(true),
        };
    }
    ("/api/chat", body)
}

struct Unfurler {
    tx: mpsc::Sender<Value>,
    petals: BTreeMap<&'static str, u64>,
}

impl Unfurler {
    async fn grow(&mut self, kind: PetalKind, text: &str) {
        if text.is_empty() {
            return;
        }
        let next = self.petals.len() as u64;
        let petal = *self.petals.entry(kind.as_str()).or_insert(next);
        let _ = self.tx.send(json!({ "chi": "unfurl", "petal": petal, "kind": kind.as_str(), "text": text })).await;
    }

    async fn close(&mut self) {
        for (kind, petal) in std::mem::take(&mut self.petals) {
            let _ = self.tx.send(json!({ "chi": "unfurl", "petal": petal, "kind": kind, "closed": true })).await;
        }
    }
}

#[async_trait]
impl Nurse for OllamaNurse {
    fn ephemeral(&self) -> bool { false }

    fn propensity(&self) -> Propensity {
        Propensity::StatelessPerCall
    }

    fn grows_on(&self) -> bool {
        true
    }

    async fn raise(&self, egg: Egg) -> Result<Cell> {
        let model = if egg.model_id.is_empty() { self.default_model.clone() } else { egg.model_id.clone() };
        info!(model = %model, "ollama-nurse.raise");
        let grower = Grower { client: self.client.clone(), url: self.url.clone(), model, ctx: self.default_ctx };

        let (tx_feed, mut rx_feed) = mpsc::channel::<Value>(64);
        let (tx_mmm, rx_mmm) = mpsc::channel::<Value>(256);
        let (tx_exit, rx_exit) = oneshot::channel::<i32>();
        let silence = CancellationToken::new();
        let stilled = silence.clone();

        tokio::spawn(async move {
            loop {
                let tone = tokio::select! {
                    biased;
                    _ = stilled.cancelled() => break,
                    tone = rx_feed.recv() => match tone {
                        Some(tone) => tone,
                        None => break,
                    },
                };
                if tone.get("chi").and_then(Value::as_str) != Some("prompt") {
                    continue;
                }
                let prompt: PromptBody = serde_json::from_value(tone).unwrap_or_default();
                grower.grow(&prompt, &tx_mmm, &stilled).await;
            }
            rx_feed.close();
            while let Ok(tone) = rx_feed.try_recv() {
                if tone.get("chi").and_then(Value::as_str) == Some("prompt") {
                    let _ = tx_mmm.send(fail("ollama_cell_stilled", "the cell was stilled before it grew this prompt".into())).await;
                }
            }
            let _ = tx_exit.send(0);
        });

        Ok(Cell {
            mark: None,
            feed: tx_feed,
            mmm: Arc::new(Mutex::new(rx_mmm)),
            emerged: rx_exit,
            ephemeral: false,
            silence,
        })
    }
}

fn fail(code: &str, message: String) -> Value {
    json!({ "chi": "error", "code": code, "message": message })
}

struct Grower {
    client: reqwest::Client,
    url: String,
    model: String,
    ctx: u64,
}

impl Grower {
    async fn grow(&self, prompt: &PromptBody, tx: &mpsc::Sender<Value>, stilled: &CancellationToken) {
        let model = prompt.model_id.as_deref().filter(|m| !m.is_empty()).unwrap_or(&self.model);
        let words_given = prompt.content.as_deref().is_some_and(|c| !c.is_empty())
            || prompt.text.as_deref().is_some_and(|t| !t.is_empty());
        if !words_given && plain_text(prompt.graft.as_deref().unwrap_or(&[])).is_none() {
            let _ = tx.send(fail("cannot-grow-on", "ollama grows on only from plain text, and this graft holds a conversation".into())).await;
            return;
        }
        let (path, body) = request(model, prompt.system_prompt.as_deref(), prompt, self.ctx);
        let url = &self.url;
        let sent = tokio::select! {
            biased;
            _ = stilled.cancelled() => {
                let _ = tx.send(json!({ "chi": "wilt", "reason": "cancel" })).await;
                return;
            }
            sent = self.client.post(format!("{url}{path}")).header(crate::hop::FROM_NURSE, "1").json(&body).send() => sent,
        };
        let resp = match sent {
            Ok(resp) if resp.status().is_success() => resp,
            Ok(resp) => {
                let status = resp.status();
                let detail = resp.text().await.unwrap_or_default();
                let _ = tx.send(fail("ollama_api_error", format!("Ollama returned {status}: {detail}"))).await;
                return;
            }
            Err(e) => {
                let _ = tx.send(fail("ollama_connection_error", format!("Failed to reach Ollama at {url}: {e}"))).await;
                return;
            }
        };
        let mut unfurler = Unfurler { tx: tx.clone(), petals: BTreeMap::new() };
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = stilled.cancelled() => {
                    unfurler.close().await;
                    let _ = tx.send(json!({ "chi": "wilt", "reason": "cancel" })).await;
                    return;
                }
                chunk = stream.next() => chunk,
            };
            let Some(Ok(chunk)) = chunk else { break };
            buf.extend_from_slice(&chunk);
            while let Some(at) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=at).collect();
                let Ok(event) = serde_json::from_slice::<Value>(&line) else { continue };
                if let Some(error) = event.get("error").and_then(Value::as_str) {
                    let _ = tx.send(fail("ollama_api_error", error.to_string())).await;
                    return;
                }
                let thinking = event.pointer("/message/thinking").or_else(|| event.get("thinking")).and_then(Value::as_str).unwrap_or("");
                unfurler.grow(PetalKind::Sepal, thinking).await;
                let text = event.pointer("/message/content").or_else(|| event.get("response")).and_then(Value::as_str).unwrap_or("");
                unfurler.grow(PetalKind::Text, text).await;
                if event.get("done").and_then(Value::as_bool).unwrap_or(false) {
                    unfurler.close().await;
                    let reason = match event.get("done_reason").and_then(Value::as_str) {
                        Some("length") => "max_tokens",
                        _ => "stop",
                    };
                    let _ = tx.send(json!({
                        "chi": "wilt",
                        "reason": reason,
                        "nectar": {
                            "input_tokens": event.get("prompt_eval_count").and_then(Value::as_u64).unwrap_or(0),
                            "output_tokens": event.get("eval_count").and_then(Value::as_u64).unwrap_or(0),
                        },
                    })).await;
                    return;
                }
            }
        }
        unfurler.close().await;
        let _ = tx.send(fail("ollama_stream_ended", "Ollama's reply ended before it was done".into())).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn petal(v: Value) -> Petal {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_graft_becomes_the_conversation_before_the_prompt() {
        let graft = vec![Bloom {
            content: Some(vec![petal(json!({"kind": "text", "text": "hi"}))]),
            grew: Some(vec![
                petal(json!({"kind": "tendril", "callId": "c", "name": "look", "args": {"at": "sky"}})),
                petal(json!({"kind": "grasp", "callId": "c", "result": "blue"})),
                petal(json!({"kind": "text", "text": "It is blue."})),
            ]),
        }];
        let prompt = PromptBody {
            content: Some(vec![petal(json!({"kind": "text", "text": "and now?"}))]),
            graft: Some(graft),
            temperature: Some(0.2),
            season: Some(64),
            frost: Some(vec!["END".into()]),
            trellis: Some(json!("json")),
            ..PromptBody::default()
        };
        let (path, body) = request("m", Some("be brief"), &prompt, 4096);
        assert_eq!(path, "/api/chat");
        let roles: Vec<&str> = body["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["system", "user", "assistant", "tool", "assistant", "user"]);
        assert_eq!(body["messages"][2]["tool_calls"][0]["function"]["name"], "look");
        assert_eq!(body["messages"][3]["content"], "blue");
        assert_eq!(body["options"]["temperature"], 0.2);
        assert_eq!(body["options"]["num_predict"], 64);
        assert_eq!(body["options"]["stop"][0], "END");
        assert_eq!(body["format"], "json");
    }

    #[test]
    fn plain_text_grows_on_and_meets_its_scion() {
        let prompt = PromptBody {
            graft: Some(vec![Bloom { content: None, grew: Some(vec![petal(json!({"kind": "text", "text": "def add(a, b):\n    "}))]) }]),
            scion: Some("\n\nprint(add(1, 2))".into()),
            ..PromptBody::default()
        };
        let (path, body) = request("coder", None, &prompt, 4096);
        assert_eq!(path, "/api/generate");
        assert_eq!(body["prompt"], "def add(a, b):\n    ");
        assert_eq!(body["suffix"], "\n\nprint(add(1, 2))");
        assert!(body.get("raw").is_none(), "a suffix needs the model's own template");

        let bare = PromptBody { scion: None, ..prompt };
        assert_eq!(request("coder", None, &bare, 4096).1["raw"], true);
    }

    #[test]
    fn a_conversation_is_not_plain_text() {
        let graft = vec![Bloom {
            content: Some(vec![petal(json!({"kind": "text", "text": "hi"}))]),
            grew: Some(vec![petal(json!({"kind": "text", "text": "hello"}))]),
        }];
        assert!(plain_text(&graft).is_none());
    }

    async fn ollama() -> String {
        use axum::{routing::post, Json, Router};
        let app = Router::new().route("/api/chat", post(|Json(body): Json<Value>| async move {
            let said = body["messages"].as_array().and_then(|m| m.last()).and_then(|m| m["content"].as_str()).unwrap_or("").to_string();
            let (tx, rx) = mpsc::channel::<Result<String, std::io::Error>>(4);
            tokio::spawn(async move {
                let _ = tx.send(Ok(format!("{}\n", json!({"message": {"content": format!("heard {said}. ")}, "done": false})))).await;
                if let Some(ms) = said.strip_prefix("slow ").and_then(|w| w.split(' ').next()).and_then(|ms| ms.parse().ok()) {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
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

    fn fed(words: &str) -> Value {
        json!({"chi": "prompt", "sid": "s", "content": [{"kind": "text", "text": words}]})
    }

    async fn raised(words: &str) -> Cell {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let nurse = OllamaNurse { client, url: ollama().await, default_model: "m".into(), default_ctx: 512 };
        let mut egg = Egg::new(hum_identity::HumId::mint(), "m", "/tmp");
        egg.prompt = serde_json::from_value(fed(words)).unwrap();
        let cell = nurse.raise(egg).await.unwrap();
        cell.feed.send(fed(words)).await.unwrap();
        cell
    }

    async fn closing(cell: &Cell, blooms: usize) -> Vec<Value> {
        let mut mmm = cell.mmm.lock().await;
        let mut heard = Vec::new();
        while heard.iter().filter(|t: &&Value| t["chi"] == "wilt" || t["chi"] == "error").count() < blooms {
            let tone = tokio::time::timeout(std::time::Duration::from_secs(5), mmm.recv()).await;
            heard.push(tone.expect("the cell answers").expect("the cell lives on"));
        }
        heard
    }

    fn said(tones: &[Value]) -> String {
        tones.iter().filter(|t| t["kind"] == "text").filter_map(|t| t["text"].as_str()).collect()
    }

    #[tokio::test]
    async fn a_live_cell_grows_every_prompt_it_is_fed() {
        let cell = raised("slow 200 one").await;
        cell.feed.send(fed("two")).await.unwrap();
        assert_eq!(said(&closing(&cell, 2).await), "heard slow 200 one. heard two. ", "a prompt fed while one grows is grown next");
        cell.feed.send(fed("three")).await.unwrap();
        assert_eq!(said(&closing(&cell, 1).await), "heard three. ", "and one fed after its wilt");
    }

    #[tokio::test]
    async fn a_stilled_cell_closes_what_it_was_fed() {
        let cell = raised("slow 5000 one").await;
        cell.feed.send(fed("two")).await.unwrap();
        {
            let mut mmm = cell.mmm.lock().await;
            let first = tokio::time::timeout(std::time::Duration::from_secs(5), mmm.recv()).await.unwrap().unwrap();
            assert_eq!(first["text"], "heard slow 5000 one. ");
        }
        cell.still();
        let tones = closing(&cell, 2).await;
        let ends: Vec<(&str, &str)> = tones.iter()
            .filter(|t| t["chi"] == "wilt" || t["chi"] == "error")
            .map(|t| (t["chi"].as_str().unwrap(), t["reason"].as_str().or(t["code"].as_str()).unwrap()))
            .collect();
        assert_eq!(ends, [("wilt", "cancel"), ("error", "ollama_cell_stilled")], "the growing bloom wilts, the waiting one is told");
        assert!(cell.mmm.lock().await.recv().await.is_none(), "then the cell ends");
        assert_eq!(cell.emerged.await, Ok(0));
    }
}
