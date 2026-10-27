//! `ollama-server`: Ollama's chat and generate API in front of humd.
//!
//! Three endpoints, in Ollama's own shape:
//!   POST /api/chat      a conversation; NDJSON streaming response
//!   POST /api/generate  a single prompt; NDJSON streaming response
//!   GET  /api/tags      the models its breath says it can reach
//!
//! Ollama's streaming response is already line-delimited JSON: every text
//! petal that unfurls is one output line, and a tendril that reaches the
//! client a `tool_calls` line. A stream holds its headers until its bloom
//! has something to say, so a bloom that breaks off before its first word
//! fails with its status, as Ollama's own does.
//!
//! Every request shares one link to humd ([`requester`]), a conversation
//! keeps one sigil while it grows ([`conversation`]), and each bloom is
//! followed to its end, or cancelled when its client goes away
//! ([`growth`]).
//!
//! The hive's other bee, its nurse, calls Ollama itself; a call that
//! carries its mark ([`hop::FROM_NURSE`]) is refused here, so the nurse
//! never prompts hum through its own forager.

mod conversation;
mod growth;
mod hop;
mod requester;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Result;
use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use thrum_core::{Bloom, Model, Petal};
use tokio::sync::mpsc;
use tracing::info;

use crate::conversation::{Message, Said, Sigils, Tail};
use crate::growth::{Call, Failure, Grew, Growing, Opening};
use crate::requester::Requester;

const HIVE_NAME: &str = "ollama-server";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");
const PORT: u16 = 14624;
const READY: Duration = Duration::from_secs(10);

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    models: Option<Vec<String>>,
}

fn read_file_config() -> FileConfig {
    std::fs::read_to_string(hum_paths::hive_config(HIVE_NAME))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[derive(Debug, Clone)]
struct Config {
    listen: String,
    label: String,
    models: Option<Vec<String>>,
    bind: Option<SocketAddr>,
}

impl Config {
    fn load() -> Self {
        Self::from(var, read_file_config())
    }

    fn from(var: impl Fn(&str) -> Option<String>, file: FileConfig) -> Self {
        let port = var("OLLAMA_SERVER_PORT").and_then(|p| p.trim().parse().ok()).or(file.port).unwrap_or(PORT);
        let host = var("OLLAMA_SERVER_HOST").map(|h| h.trim().to_string()).or(file.host).unwrap_or_else(|| "127.0.0.1".into());
        let models = var("OLLAMA_SERVER_MODELS")
            .map(|raw| raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .or(file.models)
            .filter(|m: &Vec<String>| !m.is_empty());
        let label = var("OLLAMA_SERVER_LABEL").map(|l| l.trim().to_string()).unwrap_or_else(|| "main".into());
        Self { listen: format!("{host}:{port}"), label, models, bind: None }
    }
}

struct Hive {
    cfg: Config,
    link: tokio::sync::Mutex<Option<Arc<Requester>>>,
    sigils: Mutex<Sigils>,
}

impl Hive {
    fn hello(&self) -> Value {
        let mut hello = json!({
            "version": NESTLING_VERSION,
            "propensity": { "statefulness": "convention-stateful", "richness": "medium", "wire": "ollama/api" },
            "chis": ["hello", "prompt", "cancel", "cleanup", "unfurl", "wilt", "error", "tendril-reach", "tendril-result"],
            "source": "https://github.com/adiled/hum/tree/main/hives/ollama-server",
        });
        if let Some(addr) = self.cfg.bind {
            hello["bind"] = json!({ "host": addr.ip().to_string(), "port": addr.port(), "scheme": "http" });
        }
        hello
    }

    async fn requester(&self) -> Result<Arc<Requester>, Failure> {
        let rq = {
            let mut link = self.link.lock().await;
            match link.as_ref().filter(|rq| !rq.gone()) {
                Some(rq) => rq.clone(),
                None => {
                    let rq = Requester::link(HIVE_NAME, &self.cfg.label, self.hello())?;
                    *link = Some(rq.clone());
                    rq
                }
            }
        };
        rq.ready(READY).await?;
        Ok(rq)
    }

    async fn rest(&self) {
        let mut link = self.link.lock().await;
        if link.as_ref().is_some_and(|rq| rq.gone() || (!rq.busy() && rq.idle_for() >= requester::IDLE))
            && let Some(rq) = link.take()
        {
            rq.close();
        }
    }

    fn sigils(&self) -> MutexGuard<'_, Sigils> {
        self.sigils.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Debug, Deserialize)]
struct ChatRequest {
    #[serde(default)]
    model: String,
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    stream: Option<bool>,
    #[serde(default)]
    tools: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct GenerateRequest {
    #[serde(default)]
    model: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    images: Option<Vec<String>>,
    #[serde(default)]
    stream: Option<bool>,
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn tools_to_thrum(tools: Option<&[Value]>) -> Option<Vec<Value>> {
    let out: Vec<Value> = tools?
        .iter()
        .filter(|t| t.get("type").is_none_or(|kind| kind == "function"))
        .filter_map(|t| {
            let f = t.get("function").unwrap_or(t);
            let name = f.get("name").and_then(Value::as_str).filter(|n| !n.is_empty())?;
            let schema = f.get("parameters").filter(|p| p.is_object()).cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            let mut tool = json!({ "name": name, "inputSchema": schema });
            if let Some(d) = f.get("description").and_then(Value::as_str).filter(|d| !d.is_empty()) {
                tool["description"] = json!(d);
            }
            Some(tool)
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

fn prompt(sid: &str, model: &str, system: Option<&str>, content: &[Petal], graft: &[Bloom], tools: Option<Vec<Value>>) -> Value {
    let mut tone = json!({ "chi": "prompt", "sid": sid, "content": content });
    if !model.is_empty() {
        tone["modelId"] = json!(model);
    }
    if let Some(system) = system.filter(|s| !s.is_empty()) {
        tone["systemPrompt"] = json!(system);
    }
    if !graft.is_empty() {
        tone["graft"] = json!(graft);
    }
    if let Some(tools) = tools {
        tone["tools"] = json!(tools);
    }
    tone
}

#[derive(Clone, Copy)]
enum Shape {
    Chat,
    Generate,
}

struct Turn {
    hive: Arc<Hive>,
    rq: Arc<Requester>,
    said: Option<Said>,
    model: String,
    shape: Shape,
}

impl Turn {
    fn line(&self, body: Value, done: bool) -> Map<String, Value> {
        let mut line = Map::new();
        line.insert("model".into(), json!(self.model));
        line.insert("created_at".into(), json!(now_iso()));
        match self.shape {
            Shape::Chat => line.insert("message".into(), body),
            Shape::Generate => line.insert("response".into(), body),
        };
        line.insert("done".into(), json!(done));
        line
    }

    fn said(&self, text: &str) -> Value {
        match self.shape {
            Shape::Chat => json!({ "role": "assistant", "content": text }),
            Shape::Generate => json!(text),
        }
    }

    fn called(&self, text: &str, calls: &[Call]) -> Value {
        let mut body = self.said(text);
        if matches!(self.shape, Shape::Chat) && !calls.is_empty() {
            body["tool_calls"] = calls
                .iter()
                .map(|c| json!({ "id": c.id, "function": { "index": c.index, "name": c.name, "arguments": c.args } }))
                .collect();
        }
        body
    }

    fn done(&self, text: &str, calls: &[Call], reason: &str, nectar: Option<Map<String, Value>>) -> Value {
        let mut line = self.line(self.called(text, calls), true);
        line.insert("done_reason".into(), json!(reason));
        if let Shape::Chat = self.shape {
            line.extend(nectar.unwrap_or_default());
        }
        Value::Object(line)
    }

    fn remember(&self, text: &str, calls: &[Call], sid: &str) {
        let Some(said) = &self.said else { return };
        let mut after = said.blooms.clone();
        if let Some(last) = after.last_mut() {
            let grew = last.grew.get_or_insert_with(Vec::new);
            if !text.is_empty() {
                grew.push(conversation::text(text));
            }
            grew.extend(calls.iter().map(|c| conversation::tendril(&c.id, &c.name, &c.args)));
        }
        let gone = self.hive.sigils().remember(said.system.as_deref(), &after, sid);
        if let Some(gone) = gone.filter(|g| !self.rq.following(g)) {
            self.rq.send(json!({ "chi": "cleanup", "sid": gone }));
        }
    }
}

async fn chat(State(hive): State<Arc<Hive>>, Json(req): Json<ChatRequest>) -> Response {
    let said = conversation::from_chat(&req.messages);
    if said.tail == Tail::None {
        return Failure::new(StatusCode::BAD_REQUEST, "messages holds nothing to answer").into_response();
    }
    let rq = match hive.requester().await {
        Ok(rq) => rq,
        Err(f) => return f.into_response(),
    };
    let growing = {
        let mut sigils = hive.sigils();
        let mut given = conversation::answers(&said);
        let held = if given.is_empty() { None } else { rq.held(&mut given) };
        match held.filter(|sid| !rq.following(sid)) {
            Some(sid) => Growing::open(rq.clone(), sid, Opening::Answers(given), false),
            None => {
                let (graft, content) = conversation::opening(&said);
                let sid = sigils.continuing(said.system.as_deref(), &graft, |s| rq.following(s));
                let tools = tools_to_thrum(req.tools.as_deref());
                let tone = prompt(&sid, &req.model, said.system.as_deref(), &content, &graft, tools);
                Growing::open(rq.clone(), sid, Opening::Prompt(tone), false)
            }
        }
    };
    let turn = Turn { hive, rq, said: Some(said), model: req.model, shape: Shape::Chat };
    answer(turn, growing, req.stream.unwrap_or(true)).await
}

async fn generate(State(hive): State<Arc<Hive>>, Json(req): Json<GenerateRequest>) -> Response {
    let rq = match hive.requester().await {
        Ok(rq) => rq,
        Err(f) => return f.into_response(),
    };
    let sid = Sigils::fresh();
    let content = conversation::content_of(Some(&req.prompt), req.images.as_deref());
    let tone = prompt(&sid, &req.model, req.system.as_deref(), &content, &[], None);
    let growing = Growing::open(rq.clone(), sid, Opening::Prompt(tone), true);
    let turn = Turn { hive, rq, said: None, model: req.model, shape: Shape::Generate };
    answer(turn, growing, req.stream.unwrap_or(true)).await
}

async fn answer(turn: Turn, mut growing: Growing, stream: bool) -> Response {
    let mut text = String::new();
    let mut calls: Vec<Call> = Vec::new();
    if !stream {
        loop {
            match growing.next().await {
                Ok(Grew::Text(t)) => text.push_str(&t),
                Ok(Grew::Call(c)) => calls.push(c),
                Ok(Grew::Done { reason, nectar }) => {
                    turn.remember(&text, &calls, growing.sid());
                    return Json(turn.done(&text, &calls, &reason, nectar)).into_response();
                }
                Err(f) => return f.into_response(),
            }
        }
    }
    let first = match growing.next().await {
        Ok(grew) => grew,
        Err(f) => return f.into_response(),
    };
    let (tx, rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(128);
    tokio::spawn(async move {
        let mut grew: Result<Grew, Failure> = Ok(first);
        loop {
            let (line, last) = match grew {
                Ok(Grew::Text(t)) => {
                    text.push_str(&t);
                    (Value::Object(turn.line(turn.said(&t), false)), false)
                }
                Ok(Grew::Call(c)) => {
                    let line = Value::Object(turn.line(turn.called("", std::slice::from_ref(&c)), false));
                    calls.push(c);
                    (line, false)
                }
                Ok(Grew::Done { reason, nectar }) => {
                    turn.remember(&text, &calls, growing.sid());
                    (turn.done("", &[], &reason, nectar), true)
                }
                Err(f) => {
                    let mut line = f.line();
                    line["done"] = json!(true);
                    (line, true)
                }
            };
            if tx.send(Ok(format!("{line}\n").into())).await.is_err() || last {
                break;
            }
            grew = tokio::select! {
                grew = growing.next() => grew,
                _ = tx.closed() => break,
            };
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
        .unwrap()
}

async fn tags(State(hive): State<Arc<Hive>>) -> Response {
    let models: Vec<Model> = match &hive.cfg.models {
        Some(ids) => ids.iter().map(|id| Model { id: id.clone(), hive: "hum".into() }).collect(),
        None => match hive.requester().await {
            Ok(rq) => rq.models(),
            Err(f) => return f.into_response(),
        },
    };
    let mut seen = HashSet::new();
    let models: Vec<Value> = models
        .into_iter()
        .filter(|m| seen.insert(m.id.clone()))
        .map(|m| {
            json!({
                "name": m.id,
                "model": m.id,
                "modified_at": now_iso(),
                "size": 0,
                "digest": "",
                "details": {
                    "parent_model": "",
                    "format": "hum",
                    "family": m.hive,
                    "families": [m.hive],
                    "parameter_size": "",
                    "quantization_level": "",
                }
            })
        })
        .collect();
    Json(json!({ "models": models })).into_response()
}

async fn root() -> &'static str {
    "Ollama is running\n"
}

async fn refuse_the_nurse(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if req.headers().contains_key(hop::FROM_NURSE) {
        return Failure::new(
            StatusCode::LOOP_DETECTED,
            "this is hum's own Ollama surface, not Ollama; point OLLAMA_NURSE_URL at Ollama itself",
        )
        .into_response();
    }
    next.run(req).await
}

#[tokio::main]
async fn main() -> Result<()> {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let mut cfg = Config::load();
    let listen = cfg.listen.clone();
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    let bound = listener.local_addr()?;
    cfg.bind = Some(bound);
    info!(listen = %listen, bound = %bound, "ollama-server.start");

    let hive = Arc::new(Hive { cfg, link: Default::default(), sigils: Mutex::new(Sigils::new(4096)) });
    tokio::spawn({
        let hive = hive.clone();
        async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                hive.rest().await;
            }
        }
    });
    let app = Router::new()
        .route("/", get(root))
        .route("/api/tags", get(tags))
        .route("/api/chat", post(chat))
        .route("/api/generate", post(generate))
        .layer(axum::middleware::from_fn(refuse_the_nurse))
        .with_state(hive);

    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    fn guarded() -> Router {
        Router::new()
            .route("/api/chat", post(|| async { "reached" }))
            .layer(axum::middleware::from_fn(refuse_the_nurse))
    }

    #[tokio::test]
    async fn the_hives_own_nurse_is_refused() {
        let marked = axum::http::Request::post("/api/chat")
            .header(hop::FROM_NURSE, "1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(guarded().oneshot(marked).await.unwrap().status(), StatusCode::LOOP_DETECTED);
        let plain = axum::http::Request::post("/api/chat").body(Body::empty()).unwrap();
        assert_eq!(guarded().oneshot(plain).await.unwrap().status(), StatusCode::OK);
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()).filter(|v| !v.trim().is_empty())
    }

    #[test]
    fn an_empty_setting_keeps_the_default() {
        let cfg = Config::from(env(&[("OLLAMA_SERVER_PORT", ""), ("OLLAMA_SERVER_HOST", ""), ("OLLAMA_SERVER_MODELS", "")]), FileConfig::default());
        assert_eq!(cfg.listen, "127.0.0.1:14624", "beside hum's other surfaces, off Ollama's own port");
        assert_eq!(cfg.models, None, "the breath's models");
        assert_eq!(cfg.label, "main");

        let file = FileConfig { host: Some("0.0.0.0".into()), port: Some(9), models: Some(vec!["m".into()]) };
        let cfg = Config::from(env(&[("OLLAMA_SERVER_PORT", "11500"), ("OLLAMA_SERVER_MODELS", " a, ,b ")]), file);
        assert_eq!(cfg.listen, "0.0.0.0:11500", "the environment wins over the file");
        assert_eq!(cfg.models, Some(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn a_tool_always_carries_a_schema() {
        let tools = tools_to_thrum(Some(&[
            json!({"type": "function", "function": {"name": "a", "description": "the a", "parameters": {"type": "object"}}}),
            json!({"type": "function", "function": {"name": "b"}}),
            json!({"type": "retrieval"}),
        ]));
        assert_eq!(tools, Some(vec![
            json!({"name": "a", "description": "the a", "inputSchema": {"type": "object"}}),
            json!({"name": "b", "inputSchema": {"type": "object", "properties": {}}}),
        ]));
    }
}
