//! The hum side: one forager bee for the life of the process, and a bloom
//! for each text.
//!
//! A bloom is opened only while the bee is nestled, since a tone sent
//! while humd is away is dropped, so a text that arrives then waits for
//! humd. Every text from one number shares a sid, and a bloom is keyed by
//! its sid, so a number's texts bloom one at a time, in the order they
//! came. What humd sounds under a sid goes to the bloom open on it. A
//! bloom lost before it closed, to humd going away or to the peer growing
//! it leaving the ensemble (`nurse-gone`), is grown again, up to
//! [`TRIES`] times in all; past that, the text gets what it grew.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hum_thrum::{Bee, Event, Options};
use serde_json::Value;
use thrum_core::PetalKind;
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

pub const TRIES: usize = 3;

enum Heard {
    Text(String),
    Closed { lost: bool },
}

enum Cmd {
    Open { sid: String, prompt: Value, to: mpsc::UnboundedSender<Heard> },
    Close { sid: String },
}

type Line = Arc<tokio::sync::Mutex<()>>;

pub struct Hum {
    cmds: mpsc::UnboundedSender<Cmd>,
    nestled: watch::Receiver<bool>,
    lines: Mutex<HashMap<String, Line>>,
}

impl Hum {
    pub fn start(bee: Bee) -> Arc<Hum> {
        let (cmds, rx) = mpsc::unbounded_channel();
        let (nestled_tx, nestled) = watch::channel(false);
        tokio::spawn(dispatch(bee, rx, nestled_tx));
        Arc::new(Hum { cmds, nestled, lines: Mutex::new(HashMap::new()) })
    }

    /// Opens a bloom on the prompt's sid once the blooms before it there
    /// have closed, and returns the text it grew.
    pub async fn grow(&self, sid: &str, prompt: Value) -> String {
        let line = self.lines.lock().unwrap().entry(sid.to_string()).or_default().clone();
        let turn = line.clone().lock_owned().await;
        let mut tries = 0;
        let text = loop {
            let _ = self.nestled.clone().wait_for(|n| *n).await;
            let (to, mut heard) = mpsc::unbounded_channel();
            let _ = self.cmds.send(Cmd::Open { sid: sid.to_string(), prompt: prompt.clone(), to });
            let mut text = String::new();
            let lost = loop {
                match heard.recv().await {
                    Some(Heard::Text(more)) => text.push_str(&more),
                    Some(Heard::Closed { lost }) => break lost,
                    None => break false,
                }
            };
            let _ = self.cmds.send(Cmd::Close { sid: sid.to_string() });
            tries += 1;
            if !lost || tries == TRIES {
                break text;
            }
            info!(sid, tries, "gsm-modem.regrow: the bloom was lost before it closed, so it is grown again");
        };
        drop(turn);
        let mut lines = self.lines.lock().unwrap();
        if Arc::strong_count(&line) == 2 {
            lines.remove(sid);
        }
        text
    }
}

async fn dispatch(mut bee: Bee, mut cmds: mpsc::UnboundedReceiver<Cmd>, nestled: watch::Sender<bool>) {
    let mut open: HashMap<String, mpsc::UnboundedSender<Heard>> = HashMap::new();
    let mut here = false;
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(Cmd::Open { sid, prompt, to }) => {
                    if !here {
                        let _ = to.send(Heard::Closed { lost: true });
                        continue;
                    }
                    open.insert(sid, to);
                    bee.send(prompt);
                }
                Some(Cmd::Close { sid }) => {
                    open.remove(&sid);
                }
                None => return,
            },
            event = bee.next() => match event {
                Some(Event::Nestled) => {
                    here = true;
                    info!("gsm-modem.nestled");
                    let _ = nestled.send(true);
                }
                Some(Event::Astray) => {
                    here = false;
                    warn!(open = open.len(), "gsm-modem.astray: humd went away, and each open bloom is grown again once it is back");
                    let _ = nestled.send(false);
                    for (_, to) in open.drain() {
                        let _ = to.send(Heard::Closed { lost: true });
                    }
                }
                Some(Event::Refused(why)) => {
                    error!(%why, "gsm-modem.refused");
                    std::process::exit(3);
                }
                Some(Event::Advisory(a)) => info!(code = %a.code, "gsm-modem.advisory"),
                Some(Event::Tone(tone)) => route(&tone, &open),
                None => {
                    error!("gsm-modem.client.stopped");
                    std::process::exit(1);
                }
            },
        }
    }
}

fn route(tone: &Value, open: &HashMap<String, mpsc::UnboundedSender<Heard>>) {
    let Some(to) = tone.get("sid").and_then(Value::as_str).and_then(|s| open.get(s)) else {
        return;
    };
    let heard = match tone.get("chi").and_then(Value::as_str) {
        Some("unfurl") if tone.get("kind").and_then(Value::as_str) == Some(PetalKind::Text.as_str()) => {
            Heard::Text(tone.get("text").and_then(Value::as_str).unwrap_or("").to_string())
        }
        Some("wilt") => Heard::Closed { lost: false },
        Some("error") => Heard::Closed { lost: tone.get("code").and_then(Value::as_str) == Some("nurse-gone") },
        _ => return,
    };
    let _ = to.send(heard);
}

pub fn link(hello: Value) -> anyhow::Result<Bee> {
    let opts = Options::new(crate::HIVE_NAME, "main", thrum_core::Role::Forager).hello(hello);
    let key = hum_identity::enter_as(crate::HIVE_NAME, "main", thrum_core::Role::Forager.as_str())
        .map_err(|e| hum_thrum::Refused(e.to_string()))?;
    Ok(Bee::spawn(opts, key))
}
