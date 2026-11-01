//! The client with no I/O: everything a bee decides about its connection
//! to humd, and nothing it does.
//!
//! [`Core`] is a state machine. A driver tells it what happened (the
//! socket connected, bytes arrived, the socket closed, a timer fired, the
//! bee sent a tone or changed its hello) and carries out what it asks
//! for, one [`Output`] at a time from [`Core::poll`]: connect, write,
//! close, arm a timer, hand the bee a tone or an event. The core touches
//! no socket, clock, file or thread, and the time comes in with every
//! call, so one core serves every driver: tokio for a Rust bee, and the
//! link for a bee written in anything else.
//!
//! The hello. On connect humd breathes once (`sigils: []`) before it
//! has heard anything. The core answers with the bee's hello, signed at
//! the time it is given, under a fresh rid, carrying the fields the
//! client owns ([`CLIENT_OWNED`]): `chi`, `rid`, `hive`, `role`,
//! `protoVersion`, and the signature (`hid`, `from`, `pubkey`, `signedAt`,
//! `signature`).
//! humd acknowledges the hello's rid as it does every tone's, then gives
//! its verdict: a breath (`sigils: {}`) admits the bee, an echo with
//! `ok: false` under the hello's rid refuses it. A breath before the
//! acknowledgement is the one humd sent on connect and admits nothing. A
//! refusal after admission is still a refusal. A refusal is final: the
//! core closes and stops, and whatever supervises the bee decides what
//! comes next. A humd that gives no verdict within [`VERDICT`] is
//! treated as gone.
//!
//! The bee hears every breath after the one on connect, the one that
//! admits it included: a breath carries what humd keeps the bee in step
//! with, and a forager's says what it can reach.
//!
//! A change to the hello while nestled goes out at once as a fresh signed
//! hello on the same connection, which humd takes as the bee's new
//! announcement; its refusal is final. A
//! change made while humd's verdict is pending goes out the same way the
//! moment the bee is admitted. A `pulse` carrying `headroom` updates the
//! stored hello too, so a reconnect announces the bee's latest capacity.
//!
//! Outliving humd. When the connection closes or cannot be made, the
//! core waits and dials again: [`BACKOFF_FIRST`], doubling to
//! [`BACKOFF_MOST`], a fifth either way of jitter so bees that lost humd
//! together do not return together. A bee that had nestled hears `astray`
//! first. One that never has is racing humd at boot, and its retries
//! stay quiet until [`QUIET_TRIES`] have failed. Admission resets the
//! wait.
//!
//! What the bee sends. A tone is a JSON object with a `chi`; anything
//! else is dropped with an advisory. A tone without a `rid` is given one,
//! and humd's acknowledgements of a rid the core minted (it may echo one
//! twice) are swallowed: the bee never chose it, and a refusal of it
//! becomes an advisory. A tone
//! with keys the thrum does not declare for its chi, or a chi the thrum
//! does not know, is sent anyway with an advisory, once per chi and key:
//! the wire is checked here before humd checks it again. A hello sent as
//! a tone is taken as a change to the hello. A tone sent while humd is
//! away is dropped, since its sigil went with that humd, and the count
//! is reported once the bee nestles again.
//!
//! Advisories reach the bee from two places, this core and humd, which
//! sends them as a `log` tone whose `fields.advisory` names the code.
//! Either way the bee gets an [`Advisory`], never the log tone, and humd
//! agreeing about keys the core already flagged is not news.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use hum_identity::{BeeKey, HumId};
use serde::Serialize;
use serde_json::{Map, Value, json};
use thrum_core::AdvisoryCode as Code;
use tracing::{debug, info, trace, warn};

pub const BACKOFF_FIRST: Duration = Duration::from_millis(250);
pub const BACKOFF_MOST: Duration = Duration::from_secs(30);
pub const VERDICT: Duration = Duration::from_secs(10);
pub const QUIET_TRIES: u32 = 8;
const MINTED_KEPT: usize = 4096;

pub const CLIENT_OWNED: &[&str] = &[
    "chi",
    "rid",
    "hive",
    "role",
    "protoVersion",
    "hid",
    "from",
    "pubkey",
    "signedAt",
    "signature",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Resolve,
    Unix(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timer {
    Retry,
    Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Advisory {
    pub code: String,
    pub message: String,
    pub from: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
}

impl Advisory {
    fn local(code: Code, message: impl Into<String>, fields: Map<String, Value>) -> Self {
        Self {
            code: code.as_str().to_string(),
            message: message.into(),
            from: "client".into(),
            fields,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Connect(Endpoint),
    Write(Vec<u8>),
    Close,
    Arm(Timer, Duration),
    Tone(Value),
    Nestled,
    Astray,
    Refused(String),
    Advisory(Advisory),
    Stopped,
}

#[derive(Debug)]
enum State {
    Idle,
    Dialing,
    Greeting { rid: String, acked: bool },
    Nestled { rids: Vec<String>, rehello: bool },
    Waiting,
    Refused,
    Stopped,
}

pub struct Core {
    hive: String,
    role: thrum_core::Role,
    key: BeeKey,
    endpoint: Endpoint,
    hello: Map<String, Value>,
    hello_stale: bool,
    state: State,
    rx: Vec<u8>,
    tries: u32,
    nestled_once: bool,
    warned: HashSet<String>,
    minted: VecDeque<String>,
    minted_set: HashSet<String>,
    dropped: u64,
    out: VecDeque<Output>,
}

impl Core {
    pub fn new(
        hive: impl Into<String>,
        role: thrum_core::Role,
        key: BeeKey,
        hello: Map<String, Value>,
        endpoint: Endpoint,
    ) -> Self {
        let mut core = Self {
            hive: hive.into(),
            role,
            key,
            endpoint,
            hello: Map::new(),
            hello_stale: false,
            state: State::Idle,
            rx: Vec::new(),
            tries: 0,
            nestled_once: false,
            warned: HashSet::new(),
            minted: VecDeque::new(),
            minted_set: HashSet::new(),
            dropped: 0,
            out: VecDeque::new(),
        };
        core.merge_hello(hello);
        core.hello_stale = false;
        core
    }

    pub fn bee_id(&self) -> String {
        self.key.id()
    }

    pub fn hive(&self) -> &str {
        &self.hive
    }

    pub fn role(&self) -> thrum_core::Role {
        self.role
    }

    pub fn is_nestled(&self) -> bool {
        matches!(self.state, State::Nestled { .. })
    }

    pub fn poll(&mut self) -> Option<Output> {
        self.out.pop_front()
    }

    pub fn start(&mut self) {
        if matches!(self.state, State::Idle) {
            self.dial();
        }
    }

    pub fn connected(&mut self, now_ms: i64) {
        if !matches!(self.state, State::Dialing) {
            return;
        }
        self.rx.clear();
        let rid = HumId::mint().to_string();
        let hello = self.signed_hello(&rid, now_ms);
        self.hello_stale = false;
        self.write(&hello);
        self.out.push_back(Output::Arm(Timer::Verdict, VERDICT));
        self.state = State::Greeting { rid, acked: false };
        trace!(hive = %self.hive, "thrum.hello.sent");
    }

    pub fn received(&mut self, now_ms: i64, bytes: &[u8]) {
        if !matches!(self.state, State::Greeting { .. } | State::Nestled { .. }) {
            return;
        }
        self.rx.extend_from_slice(bytes);
        while let Some(at) = self.rx.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.rx.drain(..=at).collect();
            let line = &line[..line.len() - 1];
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<Value>(line) {
                Ok(tone) => self.heard(now_ms, tone),
                Err(e) => trace!(err = %e, "thrum.parse.skip"),
            }
            if !matches!(self.state, State::Greeting { .. } | State::Nestled { .. }) {
                self.rx.clear();
                return;
            }
        }
    }

    pub fn closed(&mut self, _now_ms: i64) {
        match self.state {
            State::Dialing | State::Greeting { .. } => self.retry(),
            State::Nestled { .. } => {
                info!(hive = %self.hive, "thrum.astray — humd went away; dialing again");
                self.out.push_back(Output::Astray);
                self.retry();
            }
            State::Idle | State::Waiting | State::Refused | State::Stopped => {}
        }
    }

    pub fn fired(&mut self, _now_ms: i64, timer: Timer) {
        match (timer, &self.state) {
            (Timer::Retry, State::Waiting) => self.dial(),
            (Timer::Verdict, State::Greeting { .. }) => {
                warn!(hive = %self.hive, after_ms = VERDICT.as_millis() as u64,
                    "thrum.hello.unanswered — humd gave no verdict; dialing again");
                self.out.push_back(Output::Close);
                self.retry();
            }
            _ => {}
        }
    }

    pub fn send(&mut self, now_ms: i64, tone: Value) {
        let Value::Object(mut tone) = tone else {
            self.advise_once("not-a-tone", Code::NotATone, "a tone is a JSON object; dropped", Map::new());
            return;
        };
        let chi = match tone.get("chi").and_then(Value::as_str) {
            Some(chi) if !chi.is_empty() => chi.to_string(),
            _ => {
                self.advise_once("no-chi", Code::NotATone, "a tone needs a chi; dropped", Map::new());
                return;
            }
        };
        if chi == "hello" {
            self.advise_once(
                "hello-as-tone",
                Code::ClientOwned,
                "a hello is the client's to send; taken as a change to the hello",
                Map::new(),
            );
            self.hello(now_ms, tone);
            return;
        }
        if chi == "pulse"
            && let Some(headroom) = tone.get("headroom")
        {
            self.hello.insert("headroom".into(), headroom.clone());
            if !self.is_nestled() {
                self.hello_stale = true;
            }
        }
        if !self.is_nestled() {
            self.dropped += 1;
            return;
        }
        if !tone.get("rid").is_some_and(Value::is_string) {
            let rid = HumId::mint().to_string();
            self.remember_minted(&rid);
            tone.insert("rid".into(), Value::String(rid));
        }
        let tone = Value::Object(tone);
        self.check_spec(&chi, &tone);
        self.write(&tone);
    }

    pub fn hello(&mut self, now_ms: i64, patch: Map<String, Value>) {
        self.merge_hello(patch);
        if self.is_nestled() {
            self.rehello(now_ms);
        }
    }

    fn rehello(&mut self, now_ms: i64) {
        let rid = HumId::mint().to_string();
        let hello = self.signed_hello(&rid, now_ms);
        if let State::Nestled { rids, rehello } = &mut self.state {
            rids.push(rid);
            *rehello = true;
        }
        self.hello_stale = false;
        self.write(&hello);
    }

    pub fn stop(&mut self) {
        if matches!(self.state, State::Stopped) {
            return;
        }
        if matches!(self.state, State::Greeting { .. } | State::Nestled { .. }) {
            self.out.push_back(Output::Close);
        }
        self.state = State::Stopped;
        self.out.push_back(Output::Stopped);
    }

    fn dial(&mut self) {
        self.state = State::Dialing;
        self.out.push_back(Output::Connect(self.endpoint.clone()));
    }

    fn retry(&mut self) {
        let wait = backoff(self.tries);
        self.tries = self.tries.saturating_add(1);
        if self.nestled_once || self.tries >= QUIET_TRIES {
            if self.tries == QUIET_TRIES || (self.tries > QUIET_TRIES && self.tries.is_power_of_two()) {
                warn!(hive = %self.hive, tries = self.tries,
                    "thrum.dial.failing — humd is not answering; is it running, and does HUM_THRUM_SOCK match?");
            } else {
                debug!(hive = %self.hive, tries = self.tries, wait_ms = wait.as_millis() as u64, "thrum.dial.retry");
            }
        } else {
            debug!(hive = %self.hive, tries = self.tries, "thrum.dial.waiting — humd not up yet");
        }
        self.state = State::Waiting;
        self.out.push_back(Output::Arm(Timer::Retry, wait));
    }

    fn heard(&mut self, now_ms: i64, tone: Value) {
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("").to_string();
        if chi == "log"
            && let Some(advisory) = humd_advisory(&tone)
        {
            if self.already_advised(&advisory) {
                debug!(hive = %self.hive, code = %advisory.code, "thrum.advisory.echoed — humd agrees");
            } else {
                warn!(hive = %self.hive, code = %advisory.code, "thrum.advisory — {}", advisory.message);
                self.out.push_back(Output::Advisory(advisory));
            }
            return;
        }
        let rid = tone.get("rid").and_then(Value::as_str).map(str::to_string);
        let ok = tone.get("ok").and_then(Value::as_bool).unwrap_or(true);
        let why = || {
            tone.get("error")
                .and_then(Value::as_str)
                .unwrap_or("humd refused the hello")
                .to_string()
        };
        match &mut self.state {
            State::Greeting { rid: hello_rid, acked } => {
                if chi == "echo" && rid.as_deref() == Some(hello_rid.as_str()) {
                    if ok {
                        *acked = true;
                    } else {
                        let why = why();
                        self.refused(why);
                    }
                } else if chi == "breath" && *acked {
                    let rids = vec![hello_rid.clone()];
                    self.nestle(rids);
                    self.out.push_back(Output::Tone(tone));
                    if self.hello_stale {
                        self.rehello(now_ms);
                    }
                } else {
                    trace!(%chi, "thrum.before-verdict.skip");
                }
            }
            State::Nestled { rids, rehello } => {
                if chi == "echo" && rid.as_ref().is_some_and(|r| rids.contains(r)) {
                    if !ok {
                        let why = why();
                        self.refused(why);
                    }
                } else if chi == "breath" && *rehello {
                    *rehello = false;
                    self.out.push_back(Output::Tone(tone));
                } else if chi == "echo" && rid.as_ref().is_some_and(|r| self.minted_set.contains(r)) {
                    let rid = rid.unwrap_or_default();
                    if !ok {
                        let mut fields = Map::new();
                        fields.insert("rid".into(), Value::String(rid));
                        let message = format!("humd refused a tone: {}", why());
                        self.out.push_back(Output::Advisory(Advisory::local(Code::ToneRefused, message, fields)));
                    }
                } else {
                    let _ = now_ms;
                    self.out.push_back(Output::Tone(tone));
                }
            }
            _ => {}
        }
    }

    fn nestle(&mut self, rids: Vec<String>) {
        self.state = State::Nestled { rids, rehello: false };
        self.tries = 0;
        self.nestled_once = true;
        info!(hive = %self.hive, bee = %self.key.hid.short(), "thrum.nestled");
        self.out.push_back(Output::Nestled);
        if self.dropped > 0 {
            let n = std::mem::take(&mut self.dropped);
            let mut fields = Map::new();
            fields.insert("count".into(), json!(n));
            let message = format!(
                "{n} tone(s) sent while humd was away were dropped; stop a bloom when the bee hears astray"
            );
            self.out.push_back(Output::Advisory(Advisory::local(Code::DroppedWhileAway, message, fields)));
        }
    }

    fn refused(&mut self, why: String) {
        warn!(hive = %self.hive, reason = %why, "thrum.refused — humd will not hear this bee");
        self.state = State::Refused;
        self.out.push_back(Output::Close);
        self.out.push_back(Output::Refused(why));
    }

    fn signed_hello(&self, rid: &str, now_ms: i64) -> Value {
        let mut hello = Value::Object(self.hello.clone());
        if let Some(obj) = hello.as_object_mut() {
            obj.insert("chi".into(), json!("hello"));
            obj.insert("rid".into(), json!(rid));
            obj.insert("hive".into(), json!(self.hive));
            obj.insert("role".into(), json!(self.role.as_str()));
            obj.insert("protoVersion".into(), json!(thrum_core::THRUM_VERSION));
        }
        self.key.sign_hello_at(&mut hello, now_ms);
        hello
    }

    fn merge_hello(&mut self, patch: Map<String, Value>) {
        self.hello_stale = true;
        let mut owned = Vec::new();
        for (k, v) in patch {
            if CLIENT_OWNED.contains(&k.as_str()) {
                owned.push(k);
            } else if v.is_null() {
                self.hello.remove(&k);
            } else {
                self.hello.insert(k, v);
            }
        }
        owned.retain(|k| k != "chi");
        if !owned.is_empty() {
            owned.sort();
            let message = format!("the client owns {} on the hello; the bee's values were ignored", owned.join(", "));
            let mut fields = Map::new();
            fields.insert("keys".into(), json!(owned));
            self.advise_once(&format!("owned:{}", owned.join(",")), Code::ClientOwned, &message, fields);
        }
        let hello = Value::Object(self.hello.clone());
        let mut probe = hello.clone();
        if let Some(obj) = probe.as_object_mut() {
            obj.insert("chi".into(), json!("hello"));
        }
        self.check_spec("hello", &probe);
    }

    fn check_spec(&mut self, chi: &str, tone: &Value) {
        if !thrum_core::is_chi(chi) {
            let mut fields = Map::new();
            fields.insert("chi".into(), json!(chi));
            let message = format!("chi {chi:?} is not in the thrum; sent anyway");
            self.advise_once(&format!("chi:{chi}"), Code::UnknownChi, &message, fields);
            return;
        }
        let Some((_, keys)) = thrum_core::undeclared(tone) else {
            return;
        };
        let fresh: Vec<String> = keys
            .into_iter()
            .filter(|k| !self.warned.contains(&format!("key:{chi}:{k}")))
            .collect();
        if fresh.is_empty() {
            return;
        }
        for k in &fresh {
            self.warned.insert(format!("key:{chi}:{k}"));
        }
        let mut fields = Map::new();
        fields.insert("chi".into(), json!(chi));
        fields.insert("keys".into(), json!(fresh));
        let message = format!(
            "{chi} carries {}, which the thrum does not declare for it; sent anyway",
            fresh.join(", ")
        );
        warn!(hive = %self.hive, %chi, keys = ?fresh, "thrum.advisory — {message}");
        self.out.push_back(Output::Advisory(Advisory::local(Code::UndeclaredKey, message, fields)));
    }

    fn already_advised(&self, advisory: &Advisory) -> bool {
        if advisory.code != Code::UndeclaredKey.as_str() {
            return false;
        }
        let chi = advisory.fields.get("chi").and_then(Value::as_str).unwrap_or("");
        let keys = advisory.fields.get("keys").and_then(Value::as_array);
        keys.is_some_and(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .all(|k| self.warned.contains(&format!("key:{chi}:{k}")))
        })
    }

    fn advise_once(&mut self, key: &str, code: Code, message: &str, fields: Map<String, Value>) {
        if !self.warned.insert(key.to_string()) {
            return;
        }
        warn!(hive = %self.hive, code = code.as_str(), "thrum.advisory — {message}");
        self.out.push_back(Output::Advisory(Advisory::local(code, message, fields)));
    }

    fn remember_minted(&mut self, rid: &str) {
        if self.minted.len() >= MINTED_KEPT
            && let Some(old) = self.minted.pop_front()
        {
            self.minted_set.remove(&old);
        }
        self.minted.push_back(rid.to_string());
        self.minted_set.insert(rid.to_string());
    }

    fn write(&mut self, tone: &Value) {
        let mut line = serde_json::to_vec(tone).unwrap_or_default();
        line.push(b'\n');
        self.out.push_back(Output::Write(line));
    }
}

fn humd_advisory(tone: &Value) -> Option<Advisory> {
    let fields = tone.get("fields")?.as_object()?;
    let code = fields.get("advisory")?.as_str()?.to_string();
    let mut rest = fields.clone();
    rest.remove("advisory");
    Some(Advisory {
        code,
        message: tone.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
        from: "humd".into(),
        fields: rest,
    })
}

pub fn backoff(tries: u32) -> Duration {
    let first = BACKOFF_FIRST.as_millis() as u64;
    let most = BACKOFF_MOST.as_millis() as u64;
    let base = first.saturating_mul(1u64 << tries.min(16)).min(most);
    let jitter = 0.8 + rand::random::<f64>() * 0.4;
    Duration::from_millis((base as f64 * jitter) as u64)
}
