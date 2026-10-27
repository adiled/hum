//! A humd in the test's own process, with its bees as synthetic
//! connections or as real ones on its socket.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hum_identity::BeeKey;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use ensemble::{Ensemble, Hid, InMemoryEndpoint, PeerCapabilities};
use humd::thrumd::{Thrum, WireCheck};
use serde_json::{Value, json};
use thrum_core::{Role, WaneTracker};
use tokio::sync::mpsc::Receiver;
use tokio::time::{Instant, timeout_at};

pub const HIVE: &str = "sids";
pub const MODEL: &str = "m";
pub const TOOL: &str = "t";
pub const WAIT: Duration = Duration::from_secs(10);
pub const QUIET: Duration = Duration::from_millis(300);

pub struct Humd {
    pub thrum: Thrum,
    pub ens: Arc<Ensemble>,
    pub wane: Arc<WaneTracker>,
    pub bees: humd::EnteredBees,
    _dir: tempfile::TempDir,
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Humd {
    pub async fn start() -> Humd {
        let dir = tempfile::tempdir().unwrap();
        let at = |name: &str| dir.path().join(name);
        let thrum = Thrum::new();
        thrum.set_wire_check(WireCheck::Strict);
        let ens = Arc::new(Ensemble::with_strict_auth(Hid::random_humd(), false));
        let wane = Arc::new(WaneTracker::new());
        let bees = humd::EnteredBees::in_memory();
        let mut hum_cfg = hum_paths::config::HumConfig::default();
        hum_cfg.humd.metrics_addr = "none".into();
        let cfg = humd::DaemonConfig {
            thrum_path: at("thrum.sock"),
            http_path: at("http.sock"),
            comb_path: at("comb.json"),
            routing_path: at("routing.json"),
            hum_cfg,
            comb_persist_interval: Duration::from_secs(3600),
            routing_persist_interval: Duration::from_secs(3600),
            thrum_override: Some(thrum.clone()),
            ensemble: Some(ens.clone()),
            revoked_path: at("revoked.json"),
            bees_snapshot_path: at("bees.json"),
            advisories_path: at("advisories.json"),
            waneman: Some(wane.clone()),
            humd_key: None,
            bootstrap_peers: Vec::new(),
            thehum_cfg: None,
            trust_remote_nurses: true,
            trust_remote_attendants: true,
            hives: humd::InstalledHives::in_memory([HIVE]),
            bees: bees.clone(),
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(humd::run(cfg, async {
            let _ = stopped.await;
        }));
        while !thrum.has_sink() || !ens.has_subscribers() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Humd { thrum, ens, wane, bees, _dir: dir, _stop: stop }
    }

    pub fn enter(&self, role: Role) -> BeeKey {
        let key = BeeKey::generate();
        self.bees.enter(HIVE, role, &key);
        key
    }

    pub async fn nestle(&self, role: Role, fields: Value) -> (String, Receiver<Value>) {
        let cid = hum_identity::HumId::mint().to_string();
        let rx = self.thrum.register_synthetic(cid.clone());
        self.say(&cid, hello(&self.enter(role), role, fields)).await;
        (cid, rx)
    }

    pub async fn socket(&self) -> PathBuf {
        let sock = self._dir.path().join("bees.sock");
        tokio::spawn(humd::thrumd::serve(self.thrum.clone(), sock.clone()));
        while UnixStream::connect(&sock).await.is_err() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        sock
    }

    pub async fn nurse(&self) -> (String, Receiver<Value>) {
        let chis = ["hello", "prompt", "cancel", "cleanup", "unfurl", "wilt", "error", "tendril-reach", "tendril-result"];
        self.nestle(Role::Nurse, json!({"models": [MODEL], "chis": chis})).await
    }

    pub async fn attendant(&self) -> (String, Receiver<Value>) {
        let tools = [json!({"name": TOOL, "description": "fails", "inputSchema": {"type": "object"}})];
        self.nestle(Role::Attendant, json!({"tools": tools})).await
    }

    pub async fn forager(&self) -> (String, Receiver<Value>) {
        self.nestle(Role::Forager, json!({})).await
    }

    pub async fn say(&self, cid: &str, tone: Value) {
        self.thrum.inject_tone(cid, tone).await;
    }

    pub fn advisories(&self) -> humd::advise::Book {
        humd::advise::read(&self._dir.path().join("advisories.json"))
    }

    pub fn wane_of(&self, sid: &str) -> Option<u64> {
        self.wane.snapshot().get(sid).copied()
    }
}

pub fn hello(key: &BeeKey, role: Role, fields: Value) -> Value {
    let mut hello = json!({
        "chi": "hello",
        "rid": hum_identity::HumId::mint().to_string(),
        "hive": HIVE,
        "role": role.as_str(),
        "version": "0.0.0",
        "protoVersion": thrum_core::THRUM_VERSION,
    });
    for (k, v) in fields.as_object().unwrap() {
        hello[k] = v.clone();
    }
    key.sign_hello(&mut hello);
    hello
}

pub struct Wire {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
}

impl Wire {
    pub async fn connect(sock: &Path) -> Wire {
        let (read, write) = UnixStream::connect(sock).await.unwrap().into_split();
        Wire { lines: BufReader::new(read).lines(), write }
    }

    pub async fn send(&mut self, tone: Value) {
        let mut line = serde_json::to_vec(&tone).unwrap();
        line.push(b'\n');
        let _ = self.write.write_all(&line).await;
    }

    pub async fn next(&mut self, within: Duration) -> Option<Value> {
        let line = tokio::time::timeout(within, self.lines.next_line()).await.ok()?.ok()??;
        serde_json::from_str(&line).ok()
    }

    pub async fn nestled(&mut self) {
        let deadline = Instant::now() + WAIT;
        loop {
            let tone = self.next(deadline - Instant::now()).await.expect("never nestled");
            if tone["chi"] == "breath" && tone["sigils"].is_object() {
                return;
            }
        }
    }

    pub async fn closed(&mut self) -> bool {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            match tokio::time::timeout(deadline - Instant::now(), self.lines.next_line()).await {
                Ok(Ok(Some(_))) => continue,
                Ok(_) => return true,
                Err(_) => return false,
            }
        }
        false
    }
}

pub fn wire(a: &Humd, b: &Humd) {
    let caps = PeerCapabilities { proto_version: thrum_core::THRUM_VERSION.into(), ..Default::default() };
    let (a_end, b_end) = InMemoryEndpoint::pair(a.ens.me(), caps.clone(), b.ens.me(), caps.clone());
    a.ens.add_peer_with_caps(a_end, caps.clone());
    b.ens.add_peer_with_caps(b_end, caps);
}

pub fn prompt(sid: &str) -> Value {
    json!({"chi": "prompt", "sid": sid, "modelId": MODEL, "text": "hi"})
}

pub fn unfurl(sid: &str) -> Value {
    json!({"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": "words"})
}

pub fn wilt(sid: &str) -> Value {
    json!({"chi": "wilt", "sid": sid, "reason": "stop"})
}

pub async fn heard_within(rx: &mut Receiver<Value>, chi: &str, sid: &str, within: Duration) -> Option<Value> {
    let deadline = Instant::now() + within;
    while let Ok(Some(tone)) = timeout_at(deadline, rx.recv()).await {
        if tone["chi"] == chi && tone["sid"] == sid {
            return Some(tone);
        }
    }
    None
}

pub async fn hear(rx: &mut Receiver<Value>, chi: &str, sid: &str) -> Value {
    heard_within(rx, chi, sid, WAIT)
        .await
        .unwrap_or_else(|| panic!("no {chi} for {sid}"))
}

pub async fn reaches(rx: &mut Receiver<Value>, what: &str, named: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let tone = timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("the forager's breath never named {named}"))
            .unwrap();
        let listed = tone[what]
            .as_array()
            .is_some_and(|xs| xs.iter().any(|x| x["id"] == named || x["name"] == named));
        if tone["chi"] == "breath" && listed {
            return;
        }
    }
}

pub fn reach(sid: &str, call_id: &str) -> Value {
    json!({"chi": "tendril-reach", "sid": sid, "callId": call_id, "name": TOOL, "args": {}})
}

pub async fn answer_with_error(humd: &Humd, a: &str, a_rx: &mut Receiver<Value>, call_id: &str) {
    let reached = timeout_at(Instant::now() + WAIT, async {
        loop {
            let tone = a_rx.recv().await.unwrap();
            if tone["chi"] == "tendril-reach" && tone["callId"] == call_id {
                return tone;
            }
        }
    })
    .await
    .expect("the attendant was never reached");
    let sid = reached["sid"].clone();
    humd.say(a, json!({"chi": "error", "sid": sid, "callId": call_id, "message": "no such thing", "code": "402", "ext": {"x402": {"price": "1"}}})).await;
}

pub async fn grasp(rx: &mut Receiver<Value>, call_id: &str, within: Duration) -> Option<Value> {
    let deadline = Instant::now() + within;
    while let Ok(Some(tone)) = timeout_at(deadline, rx.recv()).await {
        if tone["chi"] == "tendril-result" && tone["callId"] == call_id {
            return Some(tone);
        }
    }
    None
}

pub struct Ensemble2 {
    pub near: Humd,
    pub far: Humd,
    pub forager: (String, Receiver<Value>),
    pub nurse: (String, Receiver<Value>),
}

pub async fn forager_near_nurse_far() -> Ensemble2 {
    let near = Humd::start().await;
    let far = Humd::start().await;
    wire(&near, &far);
    let nurse = far.nurse().await;
    let mut forager = near.forager().await;
    reaches(&mut forager.1, "models", MODEL).await;
    Ensemble2 { near, far, forager, nurse }
}

