//! One bloom, grown for one request.
//!
//! A request opens a bloom with a prompt, or carries its client's answers
//! to the tools the bloom reached for, and then follows the bloom's sid.
//! Its words come as they unfurl. A tool the client offered is the
//! client's to run only when humd reaches the forager with it, since humd
//! may have handed the name to an attendant instead; the reach is held
//! until the client answers. Once reaches stop coming the turn goes back to
//! the client with its tool calls, `done_reason: "stop"`, as Ollama's does.
//!
//! The bloom finishes when it wilts, with its reason and nectar. It fails
//! when humd or the nurse says it broke off, with the status that fits: no
//! nurse for the model is a 404, a prompt the nurse cannot grow a 400,
//! humd away or the nurse gone with it a 503, a client may try again on,
//! anything else from the nurse a 502. A bloom dropped
//! before it finishes, its client gone, is cancelled.

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Map, Value};
use thrum_core::PetalKind;
use tokio::sync::mpsc;
use tokio::time::Sleep;

use crate::conversation::{args_of, Answer};
use crate::requester::Requester;

const QUIET: Duration = Duration::from_millis(150);

#[derive(Debug, Clone)]
pub struct Call {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug)]
pub struct Failure {
    pub status: StatusCode,
    pub message: String,
}

impl Failure {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    fn of(tone: &Value) -> Self {
        let message = tone.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()).unwrap_or("the bloom broke off");
        let status = match tone.get("code").and_then(Value::as_str) {
            Some("no-nurse") => StatusCode::NOT_FOUND,
            Some("cannot-grow-on" | "empty-prompt") => StatusCode::BAD_REQUEST,
            Some("humd-away" | "nurse-gone") => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self::new(status, message)
    }

    pub fn line(&self) -> Value {
        json!({ "error": self.message })
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.status, Json(self.line())).into_response()
    }
}

pub enum Opening {
    Prompt(Value),
    Answers(Vec<Answer>),
}

pub enum Grew {
    Text(String),
    Call(Call),
    Done { reason: String, nectar: Option<Map<String, Value>> },
}

pub struct Growing {
    rq: Arc<Requester>,
    sid: String,
    tones: mpsc::UnboundedReceiver<Value>,
    asked: HashSet<String>,
    quiet: Option<Pin<Box<Sleep>>>,
    settled: bool,
    rest: bool,
    once: bool,
}

impl Growing {
    /// `once` is a bloom whose sigil ends with it, cleaned up when it does.
    pub fn open(rq: Arc<Requester>, sid: String, opening: Opening, once: bool) -> Self {
        let tones = rq.follow(&sid);
        match opening {
            Opening::Prompt(prompt) => rq.send(prompt),
            Opening::Answers(answers) => {
                rq.release(answers.iter().map(|a| a.call_id.as_str()));
                for a in answers {
                    rq.send(json!({ "chi": "tendril-result", "sid": sid, "callId": a.call_id, "output": a.result }));
                }
            }
        }
        Self { rq, sid, tones, asked: HashSet::new(), quiet: None, settled: false, rest: false, once }
    }

    pub fn sid(&self) -> &str {
        &self.sid
    }

    pub async fn next(&mut self) -> Result<Grew, Failure> {
        loop {
            let tone = match self.quiet.as_mut() {
                Some(quiet) => tokio::select! {
                    tone = self.tones.recv() => tone,
                    _ = quiet => {
                        self.settled = true;
                        self.rest = true;
                        return Ok(Grew::Done { reason: "stop".into(), nectar: None });
                    }
                },
                None => self.tones.recv().await,
            };
            let Some(tone) = tone else {
                self.settled = true;
                return Err(Failure::new(StatusCode::BAD_GATEWAY, "the forager stopped following the bloom"));
            };
            match tone.get("chi").and_then(Value::as_str) {
                Some("unfurl") => {
                    let text = tone.get("text").and_then(Value::as_str).filter(|t| !t.is_empty());
                    let closed = tone.get("closed").and_then(Value::as_bool).unwrap_or(false);
                    if let Some(text) = text.filter(|_| !closed && tone.get("kind").and_then(Value::as_str) == Some(PetalKind::Text.as_str())) {
                        return Ok(Grew::Text(text.to_string()));
                    }
                }
                Some("tendril-reach") => {
                    let id = tone.get("callId").and_then(Value::as_str).unwrap_or("").to_string();
                    if id.is_empty() || !self.asked.insert(id.clone()) {
                        continue;
                    }
                    let name = tone.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    let call = Call { index: self.asked.len() - 1, id, name, args: args_of(tone.get("args")) };
                    self.rq.hold(&self.sid, &call);
                    self.quiet = Some(Box::pin(tokio::time::sleep(QUIET)));
                    return Ok(Grew::Call(call));
                }
                Some("wilt") if self.asked.is_empty() => {
                    self.settled = true;
                    let reason = tone.get("reason").and_then(Value::as_str).unwrap_or("stop").to_string();
                    return Ok(Grew::Done { reason, nectar: tone.get("nectar").and_then(Value::as_object).cloned() });
                }
                Some("error") => {
                    self.settled = true;
                    return Err(Failure::of(&tone));
                }
                _ => {}
            }
        }
    }
}

impl Drop for Growing {
    fn drop(&mut self) {
        if !self.settled {
            self.rq.send(json!({ "chi": "cancel", "sid": self.sid }));
        }
        self.rq.unfollow(&self.sid, self.rest);
        if self.once {
            self.rq.send(json!({ "chi": "cleanup", "sid": self.sid }));
        }
    }
}
