//! The tokio driver, and the client a Rust bee holds.
//!
//! [`Bee::link`] enters the bee under its hive and label, starts a task
//! that carries out what the [`Core`] asks for over the Unix socket, and
//! returns once humd has admitted the bee, or fails with [`Refused`]: by
//! humd, or because the bee's hive is not installed here to enter it.
//! From then on the bee reads [`Event`]s with [`Bee::next`] and sends with
//! [`Bee::send`], [`Bee::hello`] and [`Bee::close`], or with a [`Sender`]
//! cloned into other tasks. Reconnecting is the core's business: a bee
//! hears [`Event::Lost`] when humd goes away and [`Event::Nestled`] when
//! it is admitted again, and nothing else changes.
//!
//! [`Endpoint::Resolve`] looks the socket up afresh on every dial, since
//! a restarted humd may publish a new one in `runtime.json`.
//!
//! The link (`hum bee link`) is this same client on stdio, so a bee in
//! any other language gets exactly what a Rust bee gets.

use std::collections::VecDeque;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use hum_identity::BeeKey;
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::trace;

use crate::core::{Advisory, Core, Endpoint, Output, Timer};

#[derive(Debug, Clone)]
pub struct Options {
    pub hive: String,
    pub label: String,
    pub hello: Map<String, Value>,
    pub endpoint: Endpoint,
}

impl Options {
    pub fn new(hive: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            hive: hive.into(),
            label: label.into(),
            hello: Map::new(),
            endpoint: Endpoint::Resolve,
        }
    }

    pub fn hello(mut self, hello: Value) -> Self {
        if let Value::Object(map) = hello {
            self.hello = map;
        }
        self
    }

    pub fn endpoint(mut self, endpoint: Endpoint) -> Self {
        self.endpoint = endpoint;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Nestled,
    Tone(Value),
    Lost,
    Advisory(Advisory),
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "humd refused the bee: {}", self.0)
    }
}

impl std::error::Error for Refused {}

enum Command {
    Send(Value),
    Hello(Map<String, Value>),
    Close,
}

#[derive(Clone)]
pub struct Sender {
    tx: mpsc::UnboundedSender<Command>,
}

impl Sender {
    pub fn send(&self, tone: Value) {
        let _ = self.tx.send(Command::Send(tone));
    }

    pub fn hello(&self, patch: Value) {
        if let Value::Object(map) = patch {
            let _ = self.tx.send(Command::Hello(map));
        }
    }

    pub fn close(&self) {
        let _ = self.tx.send(Command::Close);
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

pub struct Bee {
    id: String,
    hive: String,
    sender: Sender,
    events: mpsc::UnboundedReceiver<Event>,
    early: VecDeque<Event>,
}

impl Bee {
    pub async fn link(opts: Options) -> Result<Bee> {
        let key = hum_identity::enter_as(&opts.hive, &opts.label).map_err(|e| Refused(e.to_string()))?;
        Self::link_as(opts, key).await
    }

    pub async fn link_as(opts: Options, key: BeeKey) -> Result<Bee> {
        let mut bee = Self::spawn(opts, key);
        loop {
            match bee.events.recv().await {
                Some(Event::Nestled) => return Ok(bee),
                Some(Event::Refused(why)) => return Err(Refused(why).into()),
                Some(other) => bee.early.push_back(other),
                None => anyhow::bail!("the client stopped before humd admitted the bee"),
            }
        }
    }

    pub fn spawn(opts: Options, key: BeeKey) -> Bee {
        let core = Core::new(opts.hive.clone(), key, opts.hello, opts.endpoint);
        let id = core.bee_id();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        tokio::spawn(drive(core, cmd_rx, ev_tx));
        Bee {
            id,
            hive: opts.hive,
            sender: Sender { tx: cmd_tx },
            events: ev_rx,
            early: VecDeque::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn hive(&self) -> &str {
        &self.hive
    }

    pub fn sender(&self) -> Sender {
        self.sender.clone()
    }

    pub fn send(&self, tone: Value) {
        self.sender.send(tone);
    }

    pub fn hello(&self, patch: Value) {
        self.sender.hello(patch);
    }

    pub fn close(&self) {
        self.sender.close();
    }

    pub async fn next(&mut self) -> Option<Event> {
        if let Some(ev) = self.early.pop_front() {
            return Some(ev);
        }
        self.events.recv().await
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

type Conn = (OwnedReadHalf, OwnedWriteHalf);

async fn read_some(conn: &mut Option<Conn>, buf: &mut [u8]) -> std::io::Result<usize> {
    match conn {
        Some((r, _)) => r.read(buf).await,
        None => std::future::pending().await,
    }
}

async fn wake_at(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn drive(
    mut core: Core,
    mut cmds: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut conn: Option<Conn> = None;
    let mut retry: Option<Instant> = None;
    let mut verdict: Option<Instant> = None;
    let mut buf = vec![0u8; 64 * 1024];
    let mut cmds_open = true;
    core.start();
    loop {
        while let Some(out) = core.poll() {
            match out {
                Output::Connect(endpoint) => {
                    let path = match endpoint {
                        Endpoint::Resolve => hum_paths::thrum_sock_resolved(),
                        Endpoint::Unix(path) => path,
                    };
                    match UnixStream::connect(&path).await {
                        Ok(stream) => {
                            conn = Some(stream.into_split());
                            core.connected(now_ms());
                        }
                        Err(e) => {
                            trace!(socket = %path.display(), err = %e, "thrum.dial.failed");
                            core.closed(now_ms());
                        }
                    }
                }
                Output::Write(bytes) => {
                    let failed = match conn.as_mut() {
                        Some((_, w)) => w.write_all(&bytes).await.is_err(),
                        None => false,
                    };
                    if failed {
                        conn = None;
                        core.closed(now_ms());
                    }
                }
                Output::Close => conn = None,
                Output::Arm(Timer::Retry, after) => retry = Some(Instant::now() + after),
                Output::Arm(Timer::Verdict, after) => verdict = Some(Instant::now() + after),
                Output::Tone(tone) => {
                    let _ = events.send(Event::Tone(tone));
                }
                Output::Nestled => {
                    let _ = events.send(Event::Nestled);
                }
                Output::Lost => {
                    let _ = events.send(Event::Lost);
                }
                Output::Advisory(advisory) => {
                    let _ = events.send(Event::Advisory(advisory));
                }
                Output::Refused(why) => {
                    let _ = events.send(Event::Refused(why));
                    return;
                }
                Output::Stopped => return,
            }
        }
        tokio::select! {
            read = read_some(&mut conn, &mut buf) => match read {
                Ok(0) | Err(_) => {
                    conn = None;
                    core.closed(now_ms());
                }
                Ok(n) => core.received(now_ms(), &buf[..n]),
            },
            cmd = cmds.recv(), if cmds_open => match cmd {
                Some(Command::Send(tone)) => core.send(now_ms(), tone),
                Some(Command::Hello(patch)) => core.hello(now_ms(), patch),
                Some(Command::Close) | None => {
                    cmds_open = false;
                    core.stop();
                }
            },
            _ = wake_at(retry) => {
                retry = None;
                core.fired(now_ms(), Timer::Retry);
            }
            _ = wake_at(verdict) => {
                verdict = None;
                core.fired(now_ms(), Timer::Verdict);
            }
        }
    }
}
