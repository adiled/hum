//! The hum side: one forager bee for the life of the process, and the
//! blooms it opens for the asks the spool holds.
//!
//! The bee is linked once and outlives humd as every bee does; a bloom is
//! opened only while it is nestled, since a tone sent while humd is away
//! is dropped. Each bloom has its own sid, made from the ask's key and
//! the attempt, so no two asks, and no two tries at one ask, ever share a
//! line of blooms; its mid is the same, and its dusk is the ask's expiry.
//! What humd sounds under a sid goes to whoever opened it. humd going
//! away ends every open bloom as astray, and an ask on it is tried again.

use std::collections::HashMap;
use std::time::Duration;

use hum_thrum::{Bee, Event, Options, Sender};
use serde_json::{json, Map, Value};
use thrum_core::{Chi, PetalKind};
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

pub enum Heard {
    Unfurl(String),
    Wilt,
    Broke { code: Option<String>, message: String },
    Astray,
}

enum Cmd {
    Open { sid: String, rid: String, prompt: Value, to: mpsc::UnboundedSender<Heard> },
    Close { sid: String },
}

#[derive(Clone)]
pub struct Hum {
    cmds: mpsc::UnboundedSender<Cmd>,
    nestled: watch::Receiver<bool>,
    sender: Sender,
}

pub struct Bloom<'a> {
    pub sid: String,
    pub text: &'a str,
    pub model: &'a str,
    pub system: Option<&'a str>,
    pub dusk: u64,
}

pub enum Outcome {
    Grown { ok: bool, text: String },
    Again(String),
    Expired,
}

impl Hum {
    pub fn start(bee: Bee) -> Hum {
        let (cmds, rx) = mpsc::unbounded_channel();
        let (nestled_tx, nestled) = watch::channel(false);
        let sender = bee.sender();
        tokio::spawn(dispatch(bee, rx, nestled_tx));
        Hum { cmds, nestled, sender }
    }

    pub async fn grow(&self, bloom: Bloom<'_>, deadline: tokio::time::Instant) -> Outcome {
        let mut nestled = self.nestled.clone();
        let here = tokio::time::timeout_at(deadline, nestled.wait_for(|n| *n)).await.is_ok_and(|r| r.is_ok());
        if !here {
            return Outcome::Expired;
        }
        let rid = format!("{}-ask", bloom.sid);
        let mut prompt = Map::new();
        prompt.insert("chi".into(), json!(Chi::Prompt));
        prompt.insert("rid".into(), json!(rid));
        prompt.insert("sid".into(), json!(bloom.sid));
        prompt.insert("mid".into(), json!(bloom.sid));
        prompt.insert("dusk".into(), json!(bloom.dusk));
        prompt.insert("text".into(), json!(bloom.text));
        prompt.insert("modelId".into(), json!(bloom.model));
        if let Some(system) = bloom.system {
            prompt.insert("systemPrompt".into(), json!(system));
        }
        let (to, mut heard) = mpsc::unbounded_channel();
        let _ = self.cmds.send(Cmd::Open { sid: bloom.sid.clone(), rid, prompt: Value::Object(prompt), to });
        let mut text = String::new();
        let outcome = loop {
            let next = tokio::time::timeout_at(deadline, heard.recv()).await;
            match next {
                Err(_) => {
                    self.sender.send(json!({"chi": Chi::Cancel, "sid": bloom.sid}));
                    break Outcome::Expired;
                }
                Ok(None) => break Outcome::Again("the bee stopped".into()),
                Ok(Some(Heard::Unfurl(more))) => text.push_str(&more),
                Ok(Some(Heard::Wilt)) => break Outcome::Grown { ok: true, text },
                Ok(Some(Heard::Broke { code, message })) if code.as_deref() == Some("no-nurse") => {
                    break Outcome::Again(message);
                }
                Ok(Some(Heard::Broke { message, .. })) => break Outcome::Grown { ok: false, text: message },
                Ok(Some(Heard::Astray)) => break Outcome::Again("humd went away mid-bloom".into()),
            }
        };
        let _ = self.cmds.send(Cmd::Close { sid: bloom.sid });
        outcome
    }
}

async fn dispatch(mut bee: Bee, mut cmds: mpsc::UnboundedReceiver<Cmd>, nestled: watch::Sender<bool>) {
    let mut open: HashMap<String, mpsc::UnboundedSender<Heard>> = HashMap::new();
    let mut rids: HashMap<String, String> = HashMap::new();
    let mut here = false;
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(Cmd::Open { sid, rid, prompt, to }) => {
                    if !here {
                        let _ = to.send(Heard::Astray);
                        continue;
                    }
                    open.insert(sid.clone(), to);
                    rids.insert(rid, sid);
                    bee.send(prompt);
                }
                Some(Cmd::Close { sid }) => {
                    open.remove(&sid);
                    rids.retain(|_, s| *s != sid);
                }
                None => return,
            },
            event = bee.next() => match event {
                Some(Event::Nestled) => {
                    here = true;
                    info!("bp7.nestled");
                    let _ = nestled.send(true);
                }
                Some(Event::Astray) => {
                    here = false;
                    warn!(open = open.len(), "bp7.astray — humd went away; the asks it held are tried again");
                    let _ = nestled.send(false);
                    for (_, to) in open.drain() {
                        let _ = to.send(Heard::Astray);
                    }
                    rids.clear();
                }
                Some(Event::Refused(why)) => {
                    error!(%why, "bp7.refused");
                    std::process::exit(3);
                }
                Some(Event::Advisory(a)) => info!(code = %a.code, "bp7.advisory"),
                Some(Event::Tone(tone)) => route(&tone, &open, &rids),
                None => {
                    error!("bp7.client.stopped");
                    std::process::exit(1);
                }
            },
        }
    }
}

fn route(tone: &Value, open: &HashMap<String, mpsc::UnboundedSender<Heard>>, rids: &HashMap<String, String>) {
    let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
    if chi == "echo" {
        if tone.get("ok") == Some(&Value::Bool(false))
            && let Some(sid) = tone.get("rid").and_then(Value::as_str).and_then(|r| rids.get(r))
            && let Some(to) = open.get(sid)
        {
            let why = tone.get("error").and_then(Value::as_str).unwrap_or("humd refused the ask");
            let _ = to.send(Heard::Broke { code: None, message: why.to_string() });
        }
        return;
    }
    let Some(to) = tone.get("sid").and_then(Value::as_str).and_then(|s| open.get(s)) else {
        return;
    };
    let heard = match chi {
        "unfurl" if tone.get("kind").and_then(Value::as_str) == Some(PetalKind::Text.as_str()) => {
            Heard::Unfurl(tone.get("text").and_then(Value::as_str).unwrap_or("").to_string())
        }
        "wilt" => Heard::Wilt,
        "error" => Heard::Broke {
            code: tone.get("code").and_then(Value::as_str).map(str::to_string),
            message: tone.get("message").and_then(Value::as_str).unwrap_or("the bloom broke off").to_string(),
        },
        _ => return,
    };
    let _ = to.send(heard);
}

pub fn backoff(attempts: u32) -> Duration {
    Duration::from_millis(500u64.saturating_mul(1 << attempts.min(6))).min(Duration::from_secs(30))
}

pub fn link(label: &str, hello: Value) -> anyhow::Result<Bee> {
    let opts = Options::new(crate::HIVE_NAME, label, thrum_core::Role::Forager).hello(hello);
    let key = hum_identity::enter_as(crate::HIVE_NAME, label, thrum_core::Role::Forager.as_str())
        .map_err(|e| hum_thrum::Refused(e.to_string()))?;
    Ok(Bee::spawn(opts, key))
}
