//! `bash` — a builder: the host it runs on, lent to the blooms that build
//! there.
//!
//! Its bee is entered as a builder. humd sends it a `chi:"tendril-reach"`
//! for `bash` whenever a bloom building on this humd reaches for one,
//! carrying the `cwd` the bloom builds in, and none when it builds at
//! home. Each sigil gets one [`Shell`], started in that directory (the
//! home directory when there is none) at its first reach and kept from
//! one reach to the next. A reach whose `cwd` is not where the sigil's
//! shell started means the bloom moved, and the shell starts again there.
//! Reaches for one sigil run one after another, sigils side by side.
//!
//! Each reach is answered with `chi:"tendril-result"`: the output, the
//! exit code in `metadata.exitCode`, and `isError` when it is not 0. A
//! command past its timeout is answered with code 124 and its shell is
//! gone, and so is a reach this builder cannot run. A sigil's shell goes
//! with the sigil's `chi:"cleanup"`, or after [`IDLE`] of quiet.
//!
//! Lending a shell is the most a humd can give: every bloom that builds
//! here, from this humd or a peer it trusts, runs as the user this
//! builder runs as. A humd lends its host only when this hive is
//! installed there.

mod shell;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use hum_thrum::{Bee, Event, Options, Refused, Sender};
use serde_json::{Value, json};
use thrum_core::{BASH, BashArgs, Role};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::shell::Shell;

const HIVE: &str = "bash";

const BEE_VERSION: &str = env!("CARGO_PKG_VERSION");

const IDLE: Duration = Duration::from_secs(30 * 60);

const TIMED_OUT: i32 = 124;

type Slot = Arc<Mutex<Option<Shell>>>;

#[derive(Default)]
struct Shells {
    by_sigil: parking_lot::Mutex<HashMap<String, Slot>>,
}

impl Shells {
    fn slot(&self, sid: &str) -> Slot {
        self.by_sigil.lock().entry(sid.to_string()).or_default().clone()
    }

    fn forget(&self, sid: &str) {
        self.by_sigil.lock().remove(sid);
    }

    async fn reap(&self) {
        let slots: Vec<(String, Slot)> = self
            .by_sigil
            .lock()
            .iter()
            .map(|(sid, slot)| (sid.clone(), slot.clone()))
            .collect();
        for (sid, slot) in slots {
            let Ok(mut held) = slot.try_lock() else { continue };
            if held.as_ref().is_some_and(|sh| sh.idle_for() >= IDLE) {
                *held = None;
                info!(%sid, "builder.shell.idle");
            }
            if held.is_none() {
                drop(held);
                self.forget(&sid);
            }
        }
    }
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

    let hello = json!({
        "version": BEE_VERSION,
        "chis": ["hello", "tendril-reach", "tendril-result", "cleanup"],
    });
    let mut bee = Bee::link(Options::new(HIVE, "main", Role::Builder).hello(hello))
        .await
        .context("link the builder")?;
    info!(hive = HIVE, bee = %bee.id(), home = %hum_paths::home().display(), "builder.nestled");
    let wr = bee.sender();
    let shells = Arc::new(Shells::default());

    let reaper = shells.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            reaper.reap().await;
        }
    });

    while let Some(event) = bee.next().await {
        let tone = match event {
            Event::Tone(tone) => tone,
            Event::Refused(why) => return Err(Refused(why).into()),
            Event::Nestled | Event::Astray | Event::Advisory(_) => continue,
        };
        match tone.get("chi").and_then(Value::as_str).unwrap_or("") {
            "tendril-reach" => {
                let shells = shells.clone();
                let wr = wr.clone();
                tokio::spawn(async move { reach(&shells, &wr, tone).await });
            }
            "cleanup" => {
                if let Some(sid) = tone.get("sid").and_then(Value::as_str) {
                    shells.forget(sid);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

async fn reach(shells: &Shells, wr: &Sender, tone: Value) {
    let text = |k: &str| tone.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (sid, call_id) = (text("sid"), text("callId"));
    let answer = |output: String, exit: i32| {
        wr.send(json!({
            "chi": "tendril-result",
            "sid": sid,
            "callId": call_id,
            "output": output,
            "isError": exit != 0,
            "metadata": { "exitCode": exit },
        }));
    };
    if text("name") != BASH {
        answer(format!("this builder runs {BASH} and nothing else, not {}", text("name")), 1);
        return;
    }
    let Some(args) = tone.get("args").cloned().and_then(|a| serde_json::from_value::<BashArgs>(a).ok()) else {
        answer(format!("{BASH} needs a command"), 1);
        return;
    };
    let dir = match text("cwd") {
        cwd if cwd.is_empty() => hum_paths::home(),
        cwd => PathBuf::from(cwd),
    };
    let slot = shells.slot(&sid);
    let mut held = slot.lock().await;
    if held.as_ref().is_some_and(|sh| sh.started_in() != dir) {
        *held = None;
    }
    if held.is_none() {
        match Shell::start(&dir).await {
            Ok(sh) => *held = Some(sh),
            Err(e) => {
                warn!(%sid, dir = %dir.display(), err = %e, "builder.shell.start.failed");
                answer(format!("{e:#}"), 1);
                return;
            }
        }
    }
    let Some(sh) = held.as_mut() else { return };
    let timeout = args.timeout_ms();
    match sh.run(&args.command, Duration::from_millis(timeout)).await {
        Ok(Some(ran)) => {
            if !ran.alive {
                *held = None;
            }
            answer(ran.output, ran.exit);
        }
        Ok(None) => {
            *held = None;
            answer(
                format!(
                    "the command ran past its {timeout} ms and was stopped, with its shell; \
                     the next command starts a new shell in {}",
                    dir.display()
                ),
                TIMED_OUT,
            );
        }
        Err(e) => {
            *held = None;
            warn!(%sid, err = %e, "builder.shell.failed");
            answer(format!("{e:#}"), 1);
        }
    }
}
