//! The world apple-fm's nurse grows in: a humd in a root of its own, the
//! nurse as the process orchd would run, a bridge that stands in for
//! Apple's, and a forager that prompts through humd.
//!
//! The stand-in bridge answers `heard <words>. `, or by the first word:
//! `hang` unfurls once and waits to be killed. It answers `--probe` with
//! the line in its `probe` file, or as available when there is none, and
//! keeps each request it was asked in `asked`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use hum_thrum::{Bee, Event, Options};
use serde_json::{json, Value};
use thrum_core::Role;

pub const WAIT: Duration = Duration::from_secs(30);

pub struct World {
    pub root: PathBuf,
}

static WORLD: OnceLock<World> = OnceLock::new();

pub fn world() -> &'static World {
    WORLD.get_or_init(World::new)
}

impl World {
    fn new() -> World {
        let root = tempfile::Builder::new().prefix("apple-fm").tempdir().unwrap().keep();
        unsafe {
            std::env::set_var("XDG_STATE_HOME", root.join("s"));
            std::env::set_var("XDG_CONFIG_HOME", root.join("c"));
            std::env::set_var("XDG_DATA_HOME", root.join("d"));
            std::env::set_var("XDG_CACHE_HOME", root.join("k"));
            std::env::set_var("XDG_RUNTIME_DIR", root.join("r"));
            std::env::set_var("HUM_LOG_LEVEL", "warn");
            std::env::remove_var("HUM_THRUM_SOCK");
            std::env::remove_var("HUM_SOCKET");
        }
        std::fs::create_dir_all(hum_paths::hives_d_dir()).unwrap();
        std::fs::write(hum_paths::hum_json(), json!({"humd": {"metricsAddr": "none", "irohPort": 0}}).to_string()).unwrap();
        for hive in ["apple-fm", "probe"] {
            std::fs::write(hum_paths::hive_installation(hive), "# apple-fm's test\n").unwrap();
        }
        let sock = hum_paths::thrum_sock_resolved();
        let _ = std::fs::remove_file(&sock);
        std::thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut cfg = humd::DaemonConfig::from_env();
                cfg.humd_key = None;
                let _ = humd::run(cfg, std::future::pending::<()>()).await;
            });
        });
        let since = Instant::now();
        while std::os::unix::net::UnixStream::connect(&sock).is_err() {
            assert!(since.elapsed() < WAIT, "humd did not bind {}", sock.display());
            std::thread::sleep(Duration::from_millis(50));
        }
        World { root }
    }
}

const STUB: &str = r#"#!/bin/sh
here="$(cd "$(dirname "$0")" && pwd)"
if [ "$1" = "--probe" ]; then
  if [ -f "$here/probe" ]; then cat "$here/probe"; else echo '{"event":"probe","available":true}'; fi
  exit 0
fi
IFS= read -r line
printf '%s\n' "$line" >> "$here/asked"
echo $$ > "$here/pid"
words=$(printf '%s' "$line" | sed -n 's/.*"prompt":"\([^"]*\)"}$/\1/p')
case "$words" in
  hang*) echo '{"event":"delta","text":"hanging"}'; exec sleep 30 ;;
esac
echo '{"event":"delta","text":"heard "}'
printf '{"event":"delta","text":"%s. "}\n' "$words"
echo '{"event":"finish","inputTokens":7,"outputTokens":3}'
"#;

pub struct Stub {
    dir: tempfile::TempDir,
}

impl Stub {
    pub fn new() -> Stub {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bridge = dir.path().join("apple-fm-gen");
        std::fs::write(&bridge, STUB).unwrap();
        std::fs::set_permissions(&bridge, std::fs::Permissions::from_mode(0o755)).unwrap();
        Stub { dir }
    }

    pub fn bridge(&self) -> PathBuf {
        self.dir.path().join("apple-fm-gen")
    }

    pub fn probe(&self, available: bool, reason: &str) {
        let line = json!({"event": "probe", "available": available, "reason": reason});
        std::fs::write(self.dir.path().join("probe"), format!("{line}\n")).unwrap();
    }

    pub fn asked(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("asked"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    pub fn alive(&self) -> bool {
        let pid = std::fs::read_to_string(self.dir.path().join("pid")).unwrap();
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }
}

pub struct Nurse {
    child: tokio::process::Child,
    pub model: String,
    pub log: PathBuf,
}

impl Nurse {
    pub fn spawn(label: &str, bridge: &Path, env: &[(&str, &str)]) -> Nurse {
        let w = world();
        let model = format!("apple-fm-{label}");
        let log = w.root.join(format!("nurse-{label}.log"));
        let out = std::fs::File::create(&log).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_apple-fm-nurse"));
        cmd.env_remove("APPLE_FM_PROBE_MS")
            .env("APPLE_FM_LABEL", label)
            .env("APPLE_FM_MODEL", &model)
            .env("APPLE_FM_BRIDGE", bridge)
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        Nurse { child: cmd.spawn().unwrap(), model, log }
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub async fn wait_for(&self, line: &str) {
        let since = Instant::now();
        while !self.log_text().contains(line) {
            assert!(since.elapsed() < WAIT, "no {line:?} in the nurse's log:\n{}", self.log_text());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub async fn nestled(self) -> Nurse {
        self.wait_for("nurse.nestled").await;
        self
    }

    pub async fn exit(&mut self) -> std::process::ExitStatus {
        tokio::time::timeout(WAIT, self.child.wait()).await.expect("the nurse exits").unwrap()
    }
}

pub struct Forager {
    bee: Bee,
}

impl Forager {
    pub async fn link(label: &str) -> Forager {
        world();
        let bee = Bee::link(Options::new("probe", label, Role::Forager).hello(json!({"version": "1.0.0"}))).await.unwrap();
        Forager { bee }
    }

    pub async fn breathed(&mut self, model: &str) {
        let since = Instant::now();
        loop {
            assert!(since.elapsed() < WAIT, "humd never breathed that a nurse grows {model}");
            let tone = self.next().await;
            if tone["chi"] == "breath" && tone["models"].as_array().is_some_and(|m| m.iter().any(|m| m["id"] == model)) {
                return;
            }
        }
    }

    pub fn send(&self, tone: Value) {
        self.bee.send(tone);
    }

    pub fn prompt(&self, sid: &str, model: &str, words: &str) {
        self.send(json!({"chi": "prompt", "sid": sid, "modelId": model, "content": [{"kind": "text", "text": words}]}));
    }

    pub async fn next(&mut self) -> Value {
        loop {
            match tokio::time::timeout(WAIT, self.bee.next()).await.expect("a tone in time").expect("the link lives") {
                Event::Tone(tone) => return tone,
                _ => continue,
            }
        }
    }

    pub async fn on(&mut self, sid: &str) -> Value {
        loop {
            let tone = self.next().await;
            if tone["sid"] == sid {
                return tone;
            }
        }
    }

    pub async fn bloom(&mut self, sid: &str) -> Bloom {
        let mut bloom = Bloom::default();
        loop {
            let tone = self.on(sid).await;
            match tone["chi"].as_str() {
                Some("unfurl") => {
                    if let Some(text) = tone["text"].as_str() {
                        bloom.said.push_str(text);
                        bloom.unfurls += 1;
                    }
                }
                Some("wilt") | Some("error") => {
                    bloom.end = tone;
                    return bloom;
                }
                _ => {}
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Bloom {
    pub said: String,
    pub unfurls: usize,
    pub end: Value,
}

impl Bloom {
    pub fn ended(&self) -> String {
        let end = &self.end;
        format!("{} {}", end["chi"].as_str().unwrap_or(""), end["reason"].as_str().or(end["code"].as_str()).unwrap_or(""))
    }
}
