//! The world an Ollama client sees the forager from: a humd in a root of
//! its own, a nurse that answers by what it is asked, and the forager as
//! the process orchd would run.
//!
//! The nurse answers `echo: <words>`, or by the first word: `fail` breaks
//! off with an error, `hang` unfurls once and waits for a cancel, and
//! `tool <name> <json>` reaches that tool and unfurls what it grasped.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hum_thrum::{Bee, Event, Options, Sender};
use serde_json::{json, Value};
use thrum_core::Role;

pub const WAIT: Duration = Duration::from_secs(30);
pub const MODEL: &str = "echo-1";
pub const NURSE: &str = "nurse-echo";

#[derive(Default)]
struct Heard {
    prompts: Vec<Value>,
    cancels: Vec<String>,
    results: Vec<Value>,
}

pub struct World {
    pub root: PathBuf,
    heard: Arc<Mutex<Heard>>,
    _humd: Humd,
}

static WORLD: OnceLock<World> = OnceLock::new();

pub fn world() -> &'static World {
    WORLD.get_or_init(World::new)
}

impl World {
    fn new() -> World {
        let root = tempfile::Builder::new().prefix("ollama-server").tempdir().unwrap().keep();
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
        for hive in ["ollama-server", NURSE] {
            std::fs::write(hum_paths::hive_installation(hive), "# ollama-server's test\n").unwrap();
        }
        let humd = Humd::start();
        let heard = Arc::new(Mutex::new(Heard::default()));
        let (nestled_tx, nestled_rx) = std::sync::mpsc::channel();
        let nursing = heard.clone();
        humd.rt.spawn(async move { nurse(nursing, nestled_tx).await });
        nestled_rx.recv_timeout(WAIT).expect("the nurse nestles");
        World { root, heard, _humd: humd }
    }

    pub fn prompts(&self, marker: &str) -> Vec<Value> {
        self.heard.lock().unwrap().prompts.iter().filter(|p| p.to_string().contains(marker)).cloned().collect()
    }

    pub fn cancelled(&self, sid: &str) -> bool {
        self.heard.lock().unwrap().cancels.iter().any(|s| s == sid)
    }

    pub fn results(&self, sid: &str) -> Vec<Value> {
        self.heard.lock().unwrap().results.iter().filter(|r| r["sid"] == sid).cloned().collect()
    }

    pub async fn prompt(&self, marker: &str) -> Value {
        until(|| self.prompts(marker).pop()).await.unwrap_or_else(|| panic!("no prompt carries {marker}"))
    }
}

pub async fn until<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let since = Instant::now();
    while since.elapsed() < WAIT {
        if let Some(t) = f() {
            return Some(t);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

pub fn words_of(prompt: &Value) -> String {
    prompt["content"]
        .as_array()
        .and_then(|c| c.iter().rev().find(|p| p["kind"] == "text"))
        .and_then(|p| p["text"].as_str())
        .or_else(|| prompt["text"].as_str())
        .unwrap_or("")
        .to_string()
}

async fn nurse(heard: Arc<Mutex<Heard>>, nestled: std::sync::mpsc::Sender<()>) {
    let key = hum_identity::enter_as(NURSE, "main", "nurse").unwrap();
    let opts = Options::new(NURSE, "main", Role::Nurse).hello(json!({
        "version": "1.0.0",
        "models": [MODEL],
        "headroom": {"freeSlots": 64, "totalSlots": 64},
    }));
    let mut bee = Bee::spawn(opts, key);
    let tx = bee.sender();
    let mut reaching: HashMap<String, String> = HashMap::new();
    let mut hanging: Vec<String> = Vec::new();
    let mut reaches = 0;
    while let Some(event) = bee.next().await {
        let tone = match event {
            Event::Nestled => {
                let _ = nestled.send(());
                continue;
            }
            Event::Tone(tone) => tone,
            _ => continue,
        };
        let sid = tone["sid"].as_str().unwrap_or("").to_string();
        match tone["chi"].as_str().unwrap_or("") {
            "prompt" => {
                heard.lock().unwrap().prompts.push(tone.clone());
                let words = words_of(&tone);
                if words.starts_with("fail") {
                    tx.send(json!({"chi": "error", "sid": sid, "code": "boom", "message": "the cell broke"}));
                } else if words.starts_with("hang") {
                    unfurl(&tx, &sid, "hanging");
                    hanging.push(sid);
                } else if let Some(rest) = words.strip_prefix("tool ") {
                    let (name, args) = rest.split_once(' ').unwrap_or((rest, "{}"));
                    reaches += 1;
                    let call_id = format!("reach-{reaches}");
                    reaching.insert(call_id.clone(), sid.clone());
                    let args: Value = serde_json::from_str(args).unwrap_or(json!({}));
                    tx.send(json!({"chi": "tendril-reach", "sid": sid, "callId": call_id, "name": name, "args": args}));
                } else {
                    unfurl(&tx, &sid, &format!("echo: {words}"));
                    tx.send(json!({"chi": "wilt", "sid": sid, "reason": "stop", "nectar": {"input_tokens": 3, "output_tokens": 2}}));
                }
            }
            "cancel" => {
                heard.lock().unwrap().cancels.push(sid.clone());
                if let Some(at) = hanging.iter().position(|s| *s == sid) {
                    hanging.remove(at);
                    tx.send(json!({"chi": "wilt", "sid": sid, "reason": "cancel"}));
                }
            }
            "tendril-result" => {
                heard.lock().unwrap().results.push(tone.clone());
                if let Some(sid) = tone["callId"].as_str().and_then(|c| reaching.remove(c)) {
                    let output = tone["output"].as_str().unwrap_or("");
                    unfurl(&tx, &sid, &format!("grasped: {output}"));
                    tx.send(json!({"chi": "wilt", "sid": sid, "reason": "stop"}));
                }
            }
            _ => {}
        }
    }
}

fn unfurl(tx: &Sender, sid: &str, text: &str) {
    tx.send(json!({"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": text}));
}

struct Humd {
    rt: tokio::runtime::Handle,
}

impl Humd {
    fn start() -> Humd {
        let sock = hum_paths::thrum_sock_resolved();
        let _ = std::fs::remove_file(&sock);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            tx.send(rt.handle().clone()).unwrap();
            rt.block_on(async {
                let mut cfg = humd::DaemonConfig::from_env();
                cfg.humd_key = None;
                let _ = humd::run(cfg, std::future::pending::<()>()).await;
            });
        });
        let rt = rx.recv().unwrap();
        let since = Instant::now();
        while std::os::unix::net::UnixStream::connect(&sock).is_err() {
            assert!(since.elapsed() < WAIT, "humd did not bind {}", sock.display());
            std::thread::sleep(Duration::from_millis(50));
        }
        Humd { rt }
    }
}

pub struct Forager {
    child: tokio::process::Child,
    pub name: String,
    pub addr: SocketAddr,
    pub log: PathBuf,
}

impl Forager {
    pub async fn start(name: &str, env: &[(&str, &str)]) -> Forager {
        let w = world();
        let log = w.root.join(format!("forager-{name}.log"));
        let out = std::fs::File::create(&log).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_ollama-server"));
        for var in ["OLLAMA_SERVER_HOST", "OLLAMA_SERVER_MODELS"] {
            cmd.env_remove(var);
        }
        cmd.env("OLLAMA_SERVER_PORT", "0")
            .env("OLLAMA_SERVER_LABEL", name)
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().unwrap();
        let mut forager = Forager { child, name: name.to_string(), addr: "127.0.0.1:0".parse().unwrap(), log };
        forager.wait_for("ollama-server.start").await;
        let text = forager.log_text();
        forager.addr = text
            .split("bound=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|a| a.parse().ok())
            .unwrap_or_else(|| panic!("{name}: no bound address in its log:\n{text}"));
        forager
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub async fn wait_for(&self, line: &str) {
        let since = Instant::now();
        loop {
            if self.log_text().contains(line) {
                return;
            }
            assert!(since.elapsed() < WAIT, "{}: no {line:?} in its log:\n{}", self.name, self.log_text());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub fn log_text(&self) -> String {
        let raw = std::fs::read_to_string(&self.log).unwrap_or_default();
        let mut text = String::with_capacity(raw.len());
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                chars.by_ref().find(|c| c.is_ascii_alphabetic());
            } else {
                text.push(c);
            }
        }
        text
    }

    pub fn sockets(&self) -> Option<usize> {
        let pid = self.child.id()?;
        let out = std::process::Command::new("lsof").args(["-a", "-U", "-p", &pid.to_string(), "-F", "f"]).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.starts_with('f')).count())
    }
}

pub fn configure(hive: &str, config: Value) {
    world();
    let path = hum_paths::hive_config(hive);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, config.to_string()).unwrap();
}

pub async fn spawn_nurse(env: &[(&str, &str)]) -> tokio::process::Child {
    let w = world();
    let log = w.root.join("nurse.log");
    let out = std::fs::File::create(&log).unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_ollama-nurse"));
    for var in ["OLLAMA_NURSE_URL", "OLLAMA_NURSE_MODEL", "OLLAMA_NURSE_MODELS", "OLLAMA_NURSE_CTX"] {
        cmd.env_remove(var);
    }
    cmd.env("NO_PROXY", "127.0.0.1,localhost")
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .stdout(out.try_clone().unwrap())
        .stderr(out)
        .kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().unwrap();
    let since = Instant::now();
    while !std::fs::read_to_string(&log).unwrap_or_default().contains("nurse.nestled") {
        assert!(since.elapsed() < WAIT, "the nurse did not nestle:\n{}", std::fs::read_to_string(&log).unwrap_or_default());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    child
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().timeout(WAIT).build().unwrap()
}

pub async fn get(url: &str) -> Value {
    client().get(url).send().await.unwrap().json().await.unwrap()
}

pub async fn post(url: &str, body: Value) -> (u16, Value) {
    let resp = client().post(url).json(&body).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

pub async fn lines(url: &str, body: Value) -> (u16, Vec<Value>) {
    let resp = client().post(url).json(&body).send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("the stream ends");
    (status, text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
}
