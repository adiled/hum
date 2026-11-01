//! The world a DTN node sees the forager from: a humd in a root of its
//! own, a nurse that answers by what it is asked, the forager as the
//! process orchd would run, and a node that speaks bundles over UDP.
//!
//! The nurse answers `echo: <text>`, or by the first word of the text:
//! `slow <ms> <text>` waits before it echoes, `big <n>` unfurls n bytes,
//! `fail` breaks off with an error. It notes every sid it hears a
//! `cleanup` for.

#![allow(dead_code)]

pub mod near;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bundle_protocol::canonical::{
    new_hop_count_block, new_payload_block, CanonicalBlockBuilder, CanonicalData,
};
use bundle_protocol::flags::{BlockControlFlags, BundleControlFlags};
use bundle_protocol::primary::PrimaryBlockBuilder;
use bundle_protocol::{dtn_time_now, Bundle, CreationTimestamp, EndpointID};
use hum_thrum::{Bee, Event, Options, Sender};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thrum_core::Role;
use tokio::net::UdpSocket;

pub const WAIT: Duration = Duration::from_secs(30);
pub const QUIET: Duration = Duration::from_millis(1500);
pub const HUM_BLOCK: u64 = 193;
pub const MODEL: &str = "echo";

#[derive(Clone, Debug)]
pub struct Heard {
    pub model: String,
    pub text: String,
    pub sid: String,
}

pub struct World {
    pub root: PathBuf,
    pub ensemble: bool,
    heard: Arc<Mutex<Vec<Heard>>>,
    cleaned: Arc<Mutex<Vec<String>>>,
    busy: Arc<Mutex<HashMap<String, (usize, usize)>>>,
    rt: tokio::runtime::Handle,
    humd: Mutex<Option<Humd>>,
}

static WORLD: OnceLock<World> = OnceLock::new();

pub fn world() -> &'static World {
    WORLD.get_or_init(|| World::new(false))
}

pub fn ensemble_world() -> &'static World {
    let w = WORLD.get_or_init(|| World::new(true));
    assert!(w.ensemble, "one test binary, one kind of world");
    w
}

impl World {
    fn new(ensemble: bool) -> World {
        let root = tempfile::Builder::new().prefix("bp7").tempdir().unwrap().keep();
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
        let humd = if ensemble {
            json!({"humd": {"metricsAddr": "none", "irohPort": 0, "tcpListen": "127.0.0.1:0"}})
        } else {
            json!({"humd": {"metricsAddr": "none", "irohPort": 0}})
        };
        std::fs::write(hum_paths::hum_json(), humd.to_string()).unwrap();
        for hive in ["bp7", "nurse-echo", "nurse-later"] {
            std::fs::write(hum_paths::hive_installation(hive), "# bp7's test\n").unwrap();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            tx.send(rt.handle().clone()).unwrap();
            rt.block_on(std::future::pending::<()>());
        });
        let world = World {
            root,
            ensemble,
            heard: Arc::default(),
            cleaned: Arc::default(),
            busy: Arc::default(),
            rt: rx.recv().unwrap(),
            humd: Mutex::new(Some(Humd::start(ensemble))),
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
            *humd = Some(Humd::start(self.ensemble));
        }
    }

    pub fn pin_tcp(&self, tcp_hint: &str) {
        let addr = tcp_hint.trim_start_matches("tcp:");
        let cfg = json!({"humd": {"metricsAddr": "none", "irohPort": 0, "tcpListen": addr}});
        std::fs::write(hum_paths::hum_json(), cfg.to_string()).unwrap();
    }

    pub fn heard(&self, marker: &str) -> Vec<Heard> {
        self.heard.lock().unwrap().iter().filter(|h| h.text.contains(marker)).cloned().collect()
    }

    pub async fn wait_cleaned(&self, sid: &str) {
        let since = Instant::now();
        while !self.cleaned.lock().unwrap().iter().any(|s| s == sid) {
            assert!(since.elapsed() < WAIT, "no cleanup for {sid}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub fn peak(&self, group: &str) -> usize {
        self.busy.lock().unwrap().get(group).map(|(_, peak)| *peak).unwrap_or(0)
    }

    pub fn nurse(&self, hive: &str, model: &str) {
        let heard = self.heard.clone();
        let cleaned = self.cleaned.clone();
        let busy = self.busy.clone();
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
                    Event::Tone(tone) if tone["chi"] == "cleanup" => {
                        cleaned.lock().unwrap().push(tone["sid"].as_str().unwrap_or("").to_string());
                        continue;
                    }
                    _ => continue,
                };
                let sid = tone["sid"].as_str().unwrap_or("").to_string();
                let text = tone["text"].as_str().unwrap_or("").to_string();
                let model = tone["modelId"].as_str().unwrap_or("").to_string();
                heard.lock().unwrap().push(Heard { model, text: text.clone(), sid: sid.clone() });
                let group = group_of(&text);
                {
                    let mut busy = busy.lock().unwrap();
                    let (now, peak) = busy.entry(group.clone()).or_default();
                    *now += 1;
                    *peak = (*peak).max(*now);
                }
                let (sender, busy) = (sender.clone(), busy.clone());
                tokio::spawn(async move {
                    answer(&sender, &sid, &text).await;
                    if let Some((now, _)) = busy.lock().unwrap().get_mut(&group) {
                        *now -= 1;
                    }
                });
            }
        });
        nestled_rx.recv_timeout(WAIT).expect("the nurse nestles");
    }
}

fn group_of(text: &str) -> String {
    let last = text.split_whitespace().last().unwrap_or("");
    last.rsplit_once('-').map(|(g, _)| g).unwrap_or(last).to_string()
}

async fn answer(bee: &Sender, sid: &str, text: &str) {
    let unfurl = |text: &str| json!({"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": text});
    let mut words = text.to_string();
    if let Some(rest) = text.strip_prefix("slow ") {
        let (ms, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        tokio::time::sleep(Duration::from_millis(ms.parse().unwrap_or(0))).await;
        words = rest.to_string();
    }
    if let Some(n) = text.strip_prefix("big ") {
        let n: usize = n.trim().parse().unwrap_or(0);
        let mut left = n;
        while left > 0 {
            let take = left.min(4096);
            bee.send(unfurl(&"x".repeat(take)));
            left -= take;
        }
    } else if text == "fail" {
        bee.send(json!({"chi": "error", "sid": sid, "code": "boom", "message": "the cell broke"}));
        return;
    } else {
        bee.send(unfurl(&format!("echo: {words}")));
    }
    bee.send(json!({"chi": "wilt", "sid": sid, "reason": "stop"}));
}

struct Humd {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Humd {
    fn start(keyed: bool) -> Humd {
        let sock = hum_paths::thrum_sock_resolved();
        let _ = std::fs::remove_file(&sock);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut cfg = humd::DaemonConfig::from_env();
                if !keyed {
                    cfg.humd_key = None;
                }
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

pub struct Forager {
    child: tokio::process::Child,
    pub name: String,
    pub addr: SocketAddr,
    pub eid: String,
    pub spool: PathBuf,
    pub log: PathBuf,
    env: Vec<(String, String)>,
}

impl Forager {
    pub async fn start(name: &str, env: &[(&str, &str)]) -> Forager {
        let w = world();
        let env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Forager::spawn(
            name.to_string(),
            "127.0.0.1:0".parse().unwrap(),
            format!("dtn://{name}.hum.test/inference"),
            w.root.join(format!("spool-{name}")),
            env,
            0,
        )
        .await
    }

    async fn spawn(
        name: String,
        addr: SocketAddr,
        eid: String,
        spool: PathBuf,
        env: Vec<(String, String)>,
        life: u32,
    ) -> Forager {
        let log = world().root.join(format!("forager-{name}-{life}.log"));
        let out = std::fs::File::create(&log).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_bp7-forager"));
        cmd.env("BP7_LISTEN", addr.to_string())
            .env("BP7_NODE_EID", &eid)
            .env("BP7_MODEL", MODEL)
            .env("BP7_SPOOL", &spool)
            .env("BP7_LABEL", &name)
            .env("RUST_LOG", "info")
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .kill_on_drop(true);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().unwrap();
        let mut forager = Forager { child, name, addr, eid, spool, log, env };
        forager.wait_for("bp7.listen").await;
        let log = forager.log_text();
        let bound = log
            .split("bp7.listen listen=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|a| a.parse().ok())
            .unwrap_or_else(|| panic!("{}: no bound address in its log:\n{log}", forager.name));
        forager.addr = bound;
        forager
    }

    pub async fn wait_for(&self, line: &str) {
        let since = Instant::now();
        loop {
            if std::fs::read_to_string(&self.log).unwrap_or_default().contains(line) {
                return;
            }
            assert!(since.elapsed() < WAIT, "{}: no {line:?} in its log:\n{}", self.name, self.log_text());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub async fn restart(mut self) -> Forager {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        let life = std::fs::read_dir(&world().root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("forager-{}-", self.name)))
            .count() as u32;
        let any = "127.0.0.1:0".parse().unwrap();
        Forager::spawn(self.name.clone(), any, self.eid.clone(), self.spool.clone(), self.env.clone(), life).await
    }
}

pub fn sid_of(id: &str, attempt: u32) -> String {
    let key: String = Sha256::digest(id.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("bp7-{key}-{attempt}")
}

pub fn eid(s: &str) -> EndpointID {
    EndpointID::try_from(s).unwrap()
}

pub fn ask(from: &str, to: &str, payload: &[u8]) -> Bundle {
    ask_with(from, to, payload, Duration::from_secs(3600), CreationTimestamp::now())
}

pub fn ask_with(from: &str, to: &str, payload: &[u8], lifetime: Duration, created: CreationTimestamp) -> Bundle {
    let source = if from == "dtn:none" { EndpointID::none() } else { eid(from) };
    let primary = PrimaryBlockBuilder::default()
        .destination(eid(to))
        .source(source)
        .report_to(EndpointID::none())
        .creation_timestamp(created)
        .lifetime(lifetime)
        .build()
        .unwrap();
    let mut b = Bundle::new(
        primary,
        vec![
            new_payload_block(BlockControlFlags::empty(), payload.to_vec()),
            new_hop_count_block(2, BlockControlFlags::empty(), 32),
        ],
    );
    b.set_crc(bundle_protocol::crc::CRC_32);
    b.sort_canonicals();
    b
}

pub fn fragments(from: &str, to: &str, payload: &[u8], cuts: &[usize]) -> Vec<Bundle> {
    let created = CreationTimestamp::now();
    let mut bounds = vec![0];
    bounds.extend_from_slice(cuts);
    bounds.push(payload.len());
    bounds
        .windows(2)
        .map(|w| {
            let primary = PrimaryBlockBuilder::default()
                .bundle_control_flags(BundleControlFlags::BUNDLE_IS_FRAGMENT.bits())
                .destination(eid(to))
                .source(eid(from))
                .report_to(EndpointID::none())
                .creation_timestamp(created.clone())
                .lifetime(Duration::from_secs(3600))
                .fragmentation_offset(w[0] as u64)
                .total_data_length(payload.len() as u64)
                .build()
                .unwrap();
            let mut b = Bundle::new(
                primary,
                vec![new_payload_block(BlockControlFlags::empty(), payload[w[0]..w[1]].to_vec())],
            );
            b.set_crc(bundle_protocol::crc::CRC_32);
            b
        })
        .collect()
}

pub fn hum_block(b: &Bundle) -> Option<Value> {
    b.canonicals.iter().find(|c| c.block_type == HUM_BLOCK).and_then(|c| match c.data() {
        CanonicalData::Unknown(bytes) => serde_json::from_slice(bytes).ok(),
        _ => None,
    })
}

pub fn with_hum_block(mut b: Bundle, body: Value) -> Bundle {
    let block = CanonicalBlockBuilder::default()
        .block_type(HUM_BLOCK)
        .block_number(3)
        .block_control_flags(BlockControlFlags::BLOCK_REPLICATE.bits())
        .data(CanonicalData::Unknown(serde_json::to_vec(&body).unwrap()))
        .build()
        .unwrap();
    b.canonicals.push(block);
    b.set_crc(bundle_protocol::crc::CRC_32);
    b.sort_canonicals();
    b
}

pub fn dtn_ms_ago(ms: u64) -> CreationTimestamp {
    CreationTimestamp::with_time_and_seq(dtn_time_now() - ms, 0)
}

pub struct Reply {
    pub text: String,
    pub bundles: Vec<Bundle>,
    pub sizes: Vec<usize>,
}

impl Reply {
    pub fn first(&self) -> &Bundle {
        &self.bundles[0]
    }
}

pub struct Node {
    pub sock: UdpSocket,
    pub eid: String,
}

impl Node {
    pub async fn new(name: &str) -> Node {
        Node { sock: UdpSocket::bind("127.0.0.1:0").await.unwrap(), eid: format!("dtn://{name}.test/") }
    }

    pub fn addr(&self) -> SocketAddr {
        self.sock.local_addr().unwrap()
    }

    pub async fn send(&self, b: &mut Bundle, to: SocketAddr) {
        self.sock.send_to(&b.to_cbor(), to).await.unwrap();
    }

    pub async fn send_raw(&self, bytes: &[u8], to: SocketAddr) {
        self.sock.send_to(bytes, to).await.unwrap();
    }

    pub async fn replies(&self, n: usize, within: Duration) -> Vec<Reply> {
        let deadline = tokio::time::Instant::now() + within;
        let mut done = vec![];
        let mut partial: HashMap<String, Vec<(Bundle, usize)>> = HashMap::new();
        let mut buf = vec![0u8; 65536];
        while done.len() < n {
            let Ok(Ok((len, _))) = tokio::time::timeout_at(deadline, self.sock.recv_from(&mut buf)).await else {
                break;
            };
            let b = Bundle::try_from(&buf[..len]).expect("the forager sends bundles");
            if !b.primary.has_fragmentation() {
                let text = String::from_utf8_lossy(b.payload().map(|p| p.as_slice()).unwrap_or(&[])).into_owned();
                done.push(Reply { text, bundles: vec![b], sizes: vec![len] });
                continue;
            }
            let key = format!("{}-{}", b.primary.source, b.primary.creation_timestamp);
            let total = b.primary.total_data_length as usize;
            let parts = partial.entry(key.clone()).or_default();
            parts.push((b, len));
            let mut covered = vec![false; total];
            for (p, _) in parts.iter() {
                let off = p.primary.fragmentation_offset as usize;
                let n = p.payload().map(|d| d.len()).unwrap_or(0);
                covered[off..off + n].iter_mut().for_each(|c| *c = true);
            }
            if covered.iter().all(|c| *c) {
                let mut parts = partial.remove(&key).unwrap();
                parts.sort_by_key(|(p, _)| p.primary.fragmentation_offset);
                let mut adu = vec![0u8; total];
                for (p, _) in &parts {
                    let off = p.primary.fragmentation_offset as usize;
                    let d = p.payload().unwrap();
                    adu[off..off + d.len()].copy_from_slice(d);
                }
                let sizes = parts.iter().map(|(_, s)| *s).collect();
                let bundles = parts.into_iter().map(|(b, _)| b).collect();
                done.push(Reply { text: String::from_utf8_lossy(&adu).into_owned(), bundles, sizes });
            }
        }
        done
    }

    pub async fn reply(&self, within: Duration) -> Option<Reply> {
        self.replies(1, within).await.pop()
    }
}

pub fn spool_files(dir: &Path) -> usize {
    fn walk(d: &Path) -> usize {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| if e.path().is_dir() { walk(&e.path()) } else { 1 })
                    .sum()
            })
            .unwrap_or(0)
    }
    walk(dir)
}
