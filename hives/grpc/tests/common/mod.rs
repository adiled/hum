//! The world a gRPC client sees the forager from: a humd in a root of its
//! own, a nurse that answers by what it is asked, the forager as the
//! process orchd would run, and a client that streams tones to it.
//!
//! The nurse answers `echo: <text>`, and `slow <ms> <text>` waits before
//! it echoes. It notes every sid it hears a `cancel` for.

#![allow(dead_code)]

pub mod hum {
    tonic::include_proto!("hum");
}

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hum_thrum::{Bee, Event, Options, Sender};
use serde_json::{json, Value};
use thrum_core::Role;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;

use hum::{hum_client::HumClient, Tone};

pub const WAIT: Duration = Duration::from_secs(30);
pub const MODEL: &str = "echo";

pub struct World {
    pub root: PathBuf,
    cancelled: Arc<Mutex<Vec<String>>>,
    rt: tokio::runtime::Handle,
    humd: Mutex<Option<Humd>>,
}

static WORLD: OnceLock<World> = OnceLock::new();

pub fn world() -> &'static World {
    WORLD.get_or_init(World::new)
}

impl World {
    fn new() -> World {
        let root = tempfile::Builder::new().prefix("grpc").tempdir().unwrap().keep();
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
        std::fs::write(hum_paths::hum_json(), json!({"humd": {"metricsAddr": "none", "irohPort": 0}}).to_string())
            .unwrap();
        for hive in ["grpc", "nurse-echo", "nurse-later"] {
            std::fs::write(hum_paths::hive_installation(hive), "# grpc's test\n").unwrap();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            tx.send(rt.handle().clone()).unwrap();
            rt.block_on(std::future::pending::<()>());
        });
        let world = World {
            root,
            cancelled: Arc::default(),
            rt: rx.recv().unwrap(),
            humd: Mutex::new(Some(Humd::start())),
        };
        world.nurse("nurse-echo", MODEL);
        world
    }

    pub fn stop_humd(&self) {
        if let Some(mut humd) = self.humd.lock().unwrap().take() {
            humd.stop();
        }
    }

    pub fn start_humd(&self) {
        let mut humd = self.humd.lock().unwrap();
        if humd.is_none() {
            *humd = Some(Humd::start());
        }
    }

    pub fn cancelled(&self, sid: &str) -> bool {
        self.cancelled.lock().unwrap().iter().any(|s| s == sid)
    }

    pub fn nurse(&self, hive: &str, model: &str) {
        let cancelled = self.cancelled.clone();
        let (hive, model) = (hive.to_string(), model.to_string());
        let (nestled_tx, nestled_rx) = std::sync::mpsc::channel();
        self.rt.spawn(async move {
            let key = hum_identity::enter_as(&hive, "main", "nurse").unwrap();
            let opts = Options::new(&hive, "main", Role::Nurse).hello(json!({
                "version": "1.0.0",
                "models": [model],
                "headroom": {"freeSlots": 64, "totalSlots": 64},
            }));
            let mut bee = Bee::spawn(opts, key);
            let sender = bee.sender();
            while let Some(event) = bee.next().await {
                let tone = match event {
                    Event::Nestled => {
                        let _ = nestled_tx.send(());
                        continue;
                    }
                    Event::Tone(tone) if tone["chi"] == "prompt" => tone,
                    Event::Tone(tone) if tone["chi"] == "cancel" => {
                        cancelled.lock().unwrap().push(tone["sid"].as_str().unwrap_or("").to_string());
                        continue;
                    }
                    _ => continue,
                };
                let sid = tone["sid"].as_str().unwrap_or("").to_string();
                let text = tone["text"].as_str().unwrap_or("").to_string();
                let sender = sender.clone();
                tokio::spawn(async move { answer(&sender, &sid, &text).await });
            }
        });
        nestled_rx.recv_timeout(WAIT).expect("the nurse nestles");
    }
}

async fn answer(bee: &Sender, sid: &str, text: &str) {
    let mut words = text;
    if let Some(rest) = text.strip_prefix("slow ") {
        let (ms, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        tokio::time::sleep(Duration::from_millis(ms.parse().unwrap_or(0))).await;
        words = rest;
    }
    bee.send(json!({"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": format!("echo: {words}")}));
    bee.send(json!({"chi": "wilt", "sid": sid, "reason": "stop"}));
}

struct Humd {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Humd {
    fn start() -> Humd {
        let sock = hum_paths::thrum_sock_resolved();
        let _ = std::fs::remove_file(&sock);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut cfg = humd::DaemonConfig::from_env();
                cfg.humd_key = None;
                let _ = humd::run(cfg, async {
                    let _ = rx.await;
                })
                .await;
            });
            rt.shutdown_background();
        });
        let since = Instant::now();
        while std::os::unix::net::UnixStream::connect(&sock).is_err() {
            assert!(since.elapsed() < WAIT, "humd did not bind {}", sock.display());
            std::thread::sleep(Duration::from_millis(50));
        }
        Humd { stop: Some(tx), thread: Some(thread) }
    }

    fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn prompt(sid: &str, text: &str) -> Value {
    json!({"chi": "prompt", "sid": sid, "rid": format!("{sid}-ask"), "modelId": MODEL, "text": text})
}

pub fn is(chi: &'static str) -> impl Fn(&Value) -> bool {
    move |t| t["chi"] == chi
}

pub struct Forager {
    _child: tokio::process::Child,
    pub name: String,
    pub addr: SocketAddr,
    pub log: PathBuf,
}

impl Forager {
    pub async fn start(name: &str, env: &[(&str, &str)]) -> Forager {
        let w = world();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let log = w.root.join(format!("forager-{name}.log"));
        let out = std::fs::File::create(&log).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_grpc-forager"));
        cmd.env("HUM_GRPC_HOST", "127.0.0.1")
            .env("HUM_GRPC_PORT", port.to_string())
            .env("RUST_LOG", "info")
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let forager = Forager {
            _child: cmd.spawn().unwrap(),
            name: name.to_string(),
            addr: SocketAddr::from(([127, 0, 0, 1], port)),
            log,
        };
        forager.wait_for(".listen").await;
        forager
    }

    pub async fn wait_for(&self, line: &str) {
        let since = Instant::now();
        while !self.log_text().contains(line) {
            assert!(since.elapsed() < WAIT, "{}: no {line:?} in its log:\n{}", self.name, self.log_text());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub async fn open(&self) -> Stream {
        let since = Instant::now();
        let mut client = loop {
            match HumClient::connect(format!("http://{}", self.addr)).await {
                Ok(c) => break c,
                Err(e) => assert!(since.elapsed() < WAIT, "{}: {e}\n{}", self.name, self.log_text()),
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        let (tx, rx) = mpsc::channel(64);
        let tones = client.stream(ReceiverStream::new(rx)).await.expect("the stream opens").into_inner();
        Stream { tx: Some(tx), tones, log: self.log.clone() }
    }
}

pub struct Stream {
    tx: Option<mpsc::Sender<Tone>>,
    tones: tonic::Streaming<Tone>,
    log: PathBuf,
}

impl Stream {
    pub async fn send(&self, tone: Value) {
        let field = |k: &str| tone[k].as_str().unwrap_or("").to_string();
        let wire = Tone { chi: field("chi"), sid: field("sid"), rid: field("rid"), body: tone.to_string().into_bytes() };
        self.tx.as_ref().expect("the send side is open").send(wire).await.unwrap();
    }

    pub fn close_send(&mut self) {
        self.tx = None;
    }

    /// The next tone back, `None` once the stream ends OK, or the status
    /// it ended with.
    pub async fn next(&mut self) -> Result<Option<Value>, Status> {
        let next = tokio::time::timeout(WAIT, self.tones.message()).await;
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        let wire = next.unwrap_or_else(|_| panic!("nothing came back within {WAIT:?}\n{log}"))?;
        Ok(wire.map(|w| {
            let tone: Value = serde_json::from_slice(&w.body).expect("a body is its tone's JSON");
            for (k, lifted) in [("chi", &w.chi), ("sid", &w.sid), ("rid", &w.rid)] {
                assert_eq!(tone[k].as_str().unwrap_or(""), lifted, "{k} is lifted out of the body");
            }
            tone
        }))
    }

    /// Every tone back up to and including the first that `want` matches.
    pub async fn until(&mut self, want: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = vec![];
        loop {
            match self.next().await {
                Ok(Some(t)) => {
                    let done = want(&t);
                    seen.push(t);
                    if done {
                        return seen;
                    }
                }
                end => panic!("the stream ended first: {end:?}, after {seen:?}"),
            }
        }
    }

    /// Every tone back until the stream ends, and how it ended.
    pub async fn rest(&mut self) -> (Vec<Value>, Result<(), Status>) {
        let mut seen = vec![];
        loop {
            match self.next().await {
                Ok(Some(t)) => seen.push(t),
                Ok(None) => return (seen, Ok(())),
                Err(status) => return (seen, Err(status)),
            }
        }
    }
}
