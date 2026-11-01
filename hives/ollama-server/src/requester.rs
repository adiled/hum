//! The forager's one link to humd, shared by every request.
//!
//! The link opens on the first request and is ready once humd admits the
//! bee and its breath says what it can reach, the `models` it keeps as
//! humd breathes again. It follows each sigil it is growing a bloom in,
//! one follower a sid, and a sid it follows hears an error if humd goes
//! away mid-bloom or refuses the bee.
//!
//! A reach for one of the client's tools is held by callId until the
//! client answers it, its name and arguments kept for a client that sends
//! no ids back. A reach for a sid whose bloom has already gone back to the
//! client cannot be put to it any more, so it is answered at once with an
//! error asking the cell to call again. A humd that goes away takes the
//! blooms it held reaches for with it, so they are held no longer. A link with nothing to follow and
//! nothing held closes after [`IDLE`] and opens again on the next request.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use hum_thrum::{Bee, Event, Options, Sender};
use serde_json::{json, Value};
use thrum_core::{Model, Role};
use tokio::sync::{mpsc, watch};

use crate::conversation::Answer;
use crate::growth::{Call, Failure};

pub const IDLE: Duration = Duration::from_secs(600);
const HELD: Duration = Duration::from_secs(1800);

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Linking,
    Ready,
    Gone(String),
}

struct Held {
    sid: String,
    name: String,
    args: Value,
    at: Instant,
}

#[derive(Default)]
struct Kept {
    models: Vec<Model>,
    following: HashMap<String, mpsc::UnboundedSender<Value>>,
    held: HashMap<String, Held>,
    rested: HashSet<String>,
}

impl Kept {
    fn fail_all(&self, code: &str, message: &str) {
        for (sid, follower) in &self.following {
            let _ = follower.send(json!({ "chi": "error", "sid": sid, "code": code, "message": message }));
        }
    }
}

pub struct Requester {
    sender: Sender,
    phase: watch::Receiver<Phase>,
    kept: Arc<Mutex<Kept>>,
    used: Mutex<Instant>,
}

impl Requester {
    pub fn link(hive: &str, label: &str, hello: Value) -> Result<Arc<Self>, Failure> {
        let key = hum_identity::enter_as(hive, label, Role::Forager.as_str())
            .map_err(|e| Failure::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
        let bee = Bee::spawn(Options::new(hive, label, Role::Forager).hello(hello), key);
        let sender = bee.sender();
        let (phase_tx, phase) = watch::channel(Phase::Linking);
        let kept = Arc::new(Mutex::new(Kept::default()));
        tokio::spawn(pump(bee, kept.clone(), phase_tx));
        Ok(Arc::new(Self { sender, phase, kept, used: Mutex::new(Instant::now()) }))
    }

    pub async fn ready(&self, within: Duration) -> Result<(), Failure> {
        self.touch();
        let mut phase = self.phase.clone();
        match tokio::time::timeout(within, phase.wait_for(|p| *p != Phase::Linking)).await {
            Ok(Ok(p)) => match &*p {
                Phase::Gone(why) => Err(Failure::new(StatusCode::BAD_GATEWAY, format!("humd refused the forager's bee: {why}"))),
                _ => Ok(()),
            },
            Ok(Err(_)) => Err(Failure::new(StatusCode::BAD_GATEWAY, "the forager's link to humd closed")),
            Err(_) => Err(Failure::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "humd did not admit the forager's bee in time; is humd running?",
            )),
        }
    }

    pub fn gone(&self) -> bool {
        matches!(*self.phase.borrow(), Phase::Gone(_))
    }

    pub fn models(&self) -> Vec<Model> {
        self.kept().models.clone()
    }

    pub fn send(&self, tone: Value) {
        self.touch();
        self.sender.send(tone);
    }

    pub fn follow(&self, sid: &str) -> mpsc::UnboundedReceiver<Value> {
        self.touch();
        let (tx, rx) = mpsc::unbounded_channel();
        let mut kept = self.kept();
        kept.rested.remove(sid);
        kept.following.insert(sid.to_string(), tx);
        rx
    }

    pub fn unfollow(&self, sid: &str, rest: bool) {
        let mut kept = self.kept();
        kept.following.remove(sid);
        if rest {
            kept.rested.insert(sid.to_string());
        }
    }

    pub fn following(&self, sid: &str) -> bool {
        self.kept().following.contains_key(sid)
    }

    pub fn hold(&self, sid: &str, call: &Call) {
        let held = Held { sid: sid.to_string(), name: call.name.clone(), args: call.args.clone(), at: Instant::now() };
        self.kept().held.insert(call.id.clone(), held);
    }

    /// The sid every answer's reach is held under, each answer's callId
    /// made the reach's own; none when any is not held, or they are not
    /// all one bloom's.
    pub fn held(&self, answers: &mut [Answer]) -> Option<String> {
        let mut kept = self.kept();
        kept.held.retain(|_, h| h.at.elapsed() < HELD);
        let mut taken: HashSet<String> = HashSet::new();
        let mut sids: HashSet<String> = HashSet::new();
        for a in answers.iter_mut() {
            let id = if kept.held.contains_key(&a.call_id) && !taken.contains(&a.call_id) {
                a.call_id.clone()
            } else {
                kept.held
                    .iter()
                    .filter(|(id, h)| !taken.contains(*id) && h.name == a.name && h.args == a.args)
                    .max_by_key(|(_, h)| h.at)
                    .map(|(id, _)| id.clone())?
            };
            sids.insert(kept.held[&id].sid.clone());
            taken.insert(id.clone());
            a.call_id = id;
        }
        if sids.len() == 1 { sids.into_iter().next() } else { None }
    }

    pub fn release<'a>(&self, call_ids: impl IntoIterator<Item = &'a str>) {
        let mut kept = self.kept();
        for id in call_ids {
            kept.held.remove(id);
        }
    }

    pub fn busy(&self) -> bool {
        let mut kept = self.kept();
        kept.held.retain(|_, h| h.at.elapsed() < HELD);
        !kept.following.is_empty() || !kept.held.is_empty()
    }

    pub fn idle_for(&self) -> Duration {
        self.used.lock().unwrap_or_else(|e| e.into_inner()).elapsed()
    }

    pub fn close(&self) {
        self.sender.close();
    }

    fn touch(&self) {
        *self.used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner())
    }
}

async fn pump(mut bee: Bee, kept: Arc<Mutex<Kept>>, phase: watch::Sender<Phase>) {
    let sender = bee.sender();
    let lock = || kept.lock().unwrap_or_else(|e| e.into_inner());
    let why = loop {
        let Some(event) = bee.next().await else { break "the link closed".to_string() };
        match event {
            Event::Tone(tone) if tone.get("chi").and_then(Value::as_str) == Some("breath") => {
                let models = tone.get("models").cloned().and_then(|m| serde_json::from_value(m).ok()).unwrap_or_default();
                lock().models = models;
                phase.send_replace(Phase::Ready);
            }
            Event::Tone(tone) => {
                let Some(sid) = tone.get("sid").and_then(Value::as_str) else { continue };
                let kept = lock();
                if let Some(follower) = kept.following.get(sid) {
                    let _ = follower.send(tone);
                } else if tone.get("chi").and_then(Value::as_str) == Some("tendril-reach") && kept.rested.contains(sid) {
                    sender.send(json!({
                        "chi": "tendril-result",
                        "sid": sid,
                        "callId": tone.get("callId").cloned().unwrap_or(Value::Null),
                        "isError": true,
                        "output": "this call came after the turn had gone back to the client, so no one will answer it; call it again",
                    }));
                }
            }
            Event::Astray => {
                phase.send_replace(Phase::Linking);
                let mut kept = lock();
                kept.fail_all("humd-away", "humd went away mid-bloom");
                kept.held.clear();
                kept.rested.clear();
            }
            Event::Refused(why) => break why,
            Event::Nestled | Event::Advisory(_) => {}
        }
    };
    lock().fail_all("humd-refused", &why);
    phase.send_replace(Phase::Gone(why));
}
