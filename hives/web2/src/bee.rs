//! The bee: an attendant serving one fixed tool, `wrepl`.
//!
//! A reach carries one command line in `line`, and its result is the
//! engine's answer. Every reach is answered, even when the engine fails or
//! panics. A reach that came from a peer humd carries that peer's `from`,
//! and is refused unless web2.json says `servePeers`: what web2 is signed
//! in to is this machine's, lent to a peer's blooms only when a person says
//! so. web2.json is read again when it changes, so a service added
//! there is reached on the next call; a broken file is answered with what
//! is wrong with it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use hum_thrum::{Bee, Event, Options, Refused, Sender};
use parking_lot::Mutex;
use serde_json::{Value, json};
use thrum_core::Role;
use tracing::{info, warn};

use crate::config::{self, HIVE};
use crate::engine::{Answer, Engine};

pub const TOOL: &str = "wrepl";
const BEE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn tool() -> Value {
    json!({
        "name": TOOL,
        "description": "A command line for the web services this hum reaches: APIs and MCP servers, each one as if it were a CLI. \
            Start with `help` to list them, `<service> help` for a service's commands, and `<service> <command> --help` for a command's flags. \
            Flags are `--name value`; `--json '{…}'` gives a command's whole input, and `--jq '<filter>'` narrows a JSON result. \
            A service only reads unless this hum's web2 config turns its writes on.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "line": {
                    "type": "string",
                    "description": "One command line, as typed at a prompt: `help`, `github help issues`, \
                        `github issues/list-for-repo --owner rust-lang --repo rust --state open --jq '.[].title'`."
                }
            },
            "required": ["line"]
        }
    })
}

struct Current {
    stamp: Option<SystemTime>,
    engine: Result<Arc<Engine>, String>,
}

pub struct Shared {
    current: Mutex<Current>,
}

fn stamp() -> Option<SystemTime> {
    std::fs::metadata(config::config_path()).and_then(|m| m.modified()).ok()
}

fn build() -> Result<Arc<Engine>, String> {
    config::load().map(|c| Arc::new(Engine::new(c, config::state_dir()))).map_err(|e| format!("{e:#}"))
}

impl Shared {
    pub fn new() -> Shared {
        Shared { current: Mutex::new(Current { stamp: stamp(), engine: build() }) }
    }

    pub fn engine(&self) -> Result<Arc<Engine>, String> {
        let now = stamp();
        let mut current = self.current.lock();
        if current.stamp != now {
            info!(path = %config::config_path().display(), "web2.config.reloaded");
            *current = Current { stamp: now, engine: build() };
        }
        current.engine.clone()
    }
}

impl Default for Shared {
    fn default() -> Self {
        Shared::new()
    }
}

pub async fn answer_reach(shared: &Shared, tone: &Value) -> Answer {
    let name = tone.get("name").and_then(Value::as_str).unwrap_or("");
    if name != TOOL {
        return Answer { output: format!("web2 serves {TOOL} and nothing else, not {name}"), is_error: true };
    }
    let Some(line) = tone.pointer("/args/line").and_then(Value::as_str) else {
        return Answer { output: format!("{TOOL} needs a `line`; try `help`"), is_error: true };
    };
    let engine = match shared.engine() {
        Ok(e) => e,
        Err(why) => {
            return Answer {
                output: format!("web2's config is broken, so wrepl reaches nothing until a person fixes it: {why}"),
                is_error: true,
            };
        }
    };
    if let Some(refused) = peer_refused(&engine.config, tone) {
        return refused;
    }
    let line = line.to_string();
    match tokio::spawn(async move { engine.run(&line).await }).await {
        Ok(answer) => answer,
        Err(e) => Answer { output: format!("wrepl failed on that line: {e}"), is_error: true },
    }
}

pub fn peer_refused(config: &config::Config, tone: &Value) -> Option<Answer> {
    let peer = tone.get("from").and_then(Value::as_str).filter(|f| !f.is_empty())?;
    if config.serve_peers {
        return None;
    }
    let short: String = peer.chars().take(17).collect();
    Some(Answer {
        output: format!(
            "web2 on this humd serves its own blooms, not {short}…'s; a person lends it to peers with `servePeers: true` in its web2.json"
        ),
        is_error: true,
    })
}

fn result(tone: &Value, answer: &Answer) -> Value {
    let text = |k: &str| tone.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    json!({
        "chi": "tendril-result",
        "sid": text("sid"),
        "callId": text("callId"),
        "output": answer.output,
        "isError": answer.is_error,
    })
}

async fn reach(shared: Arc<Shared>, wr: Sender, tone: Value) {
    let answer = answer_reach(&shared, &tone).await;
    wr.send(result(&tone, &answer));
}

pub async fn serve() -> Result<()> {
    let hello = json!({
        "version": BEE_VERSION,
        "tools": [tool()],
        "chis": ["hello", "tendril-reach", "tendril-result"],
        "source": "https://github.com/adiled/hum/tree/main/hives/web2",
    });
    let mut bee = Bee::link(Options::new(HIVE, "main", Role::Attendant).hello(hello))
        .await
        .context("link the web2 attendant")?;
    let shared = Arc::new(Shared::new());
    match shared.engine() {
        Ok(e) => info!(hive = HIVE, bee = %bee.id(), services = e.config.services.len(), "attendant.nestled"),
        Err(why) => warn!(hive = HIVE, bee = %bee.id(), %why, "attendant.nestled.config-broken"),
    }
    let wr = bee.sender();

    let reaper = shared.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Ok(engine) = reaper.engine() {
                engine.pool().reap().await;
            }
        }
    });

    while let Some(event) = bee.next().await {
        let tone = match event {
            Event::Tone(tone) => tone,
            Event::Refused(why) => return Err(Refused(why).into()),
            Event::Nestled | Event::Astray | Event::Advisory(_) => continue,
        };
        if tone.get("chi").and_then(Value::as_str) == Some("tendril-reach") {
            tokio::spawn(reach(shared.clone(), wr.clone(), tone));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_is_one_line_in() {
        let t = tool();
        assert_eq!(t["name"], TOOL);
        assert_eq!(t["inputSchema"]["required"], json!(["line"]));
    }

    #[test]
    fn a_peers_reach_is_refused_until_web2_json_lends_it() {
        let local = json!({ "name": TOOL, "args": { "line": "help" } });
        let remote = json!({ "name": TOOL, "args": { "line": "help" }, "from": "humd_0123456789abcdef0123" });
        let closed = config::parse("{}", std::path::PathBuf::new()).unwrap();
        let open = config::parse(r#"{ "servePeers": true }"#, std::path::PathBuf::new()).unwrap();
        assert!(peer_refused(&closed, &local).is_none());
        let refused = peer_refused(&closed, &remote).expect("a peer is refused by default");
        assert!(refused.is_error && refused.output.contains("servePeers"), "{}", refused.output);
        assert!(peer_refused(&open, &remote).is_none());
    }

    #[test]
    fn a_result_answers_the_reach_it_came_from() {
        let tone = json!({ "chi": "tendril-reach", "sid": "s1", "callId": "c9", "name": TOOL, "args": { "line": "help" } });
        let r = result(&tone, &Answer { output: "out".into(), is_error: true });
        assert_eq!(r, json!({ "chi": "tendril-result", "sid": "s1", "callId": "c9", "output": "out", "isError": true }));
    }
}
