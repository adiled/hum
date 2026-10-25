//! bp7-forager: hum as a DTN endpoint. A bundle that asks arrives over
//! UDP (RFC 7122), is held in the spool until its answer has gone home,
//! and is answered by a bloom on the humd here.
//!
//! Three tasks share the spool. Intake decodes what arrives and holds
//! what is an ask, off the socket's task so a contact window's burst is
//! not lost to a full receive buffer. Growers take held asks, at most
//! `BP7_BLOOMS` at a time, open a bloom for each, and send its answer
//! home; an ask that cannot bloom yet, because humd is away or no nurse
//! grows its model, is tried again until it expires. The sweeper forgets
//! what has expired.

mod dtn;
mod hum;
mod spool;

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, info, warn};

use crate::dtn::Arrival;
use crate::hum::{Bloom, Hum, Outcome};
use crate::spool::{Grown, Spool, Taken};

pub const HIVE_NAME: &str = "bp7";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");
const DATAGRAM: usize = 65_536;
const INTAKE: usize = 4_096;

#[derive(Debug, Clone)]
struct Config {
    listen: SocketAddr,
    node_eid: String,
    label: String,
    model: String,
    route: Option<SocketAddr>,
    allow: Option<Vec<IpAddr>>,
    blooms: usize,
    mtu: usize,
    spool: PathBuf,
    hold: usize,
    sweep: Duration,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn parsed<T: std::str::FromStr>(name: &str, default: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match var(name) {
        Some(v) => v.trim().parse().map_err(|e| anyhow!("{name}={v}: {e}")),
        None => Ok(default),
    }
}

impl Config {
    fn from_env() -> Result<Self> {
        let node_eid = var("BP7_NODE_EID").unwrap_or_else(|| "dtn://hum.local/inference".into());
        bundle_protocol::EndpointID::try_from(node_eid.as_str())
            .map_err(|e| anyhow!("BP7_NODE_EID={node_eid} is not an endpoint id: {e:?}"))?;
        let route = match var("BP7_ROUTE") {
            Some(r) => Some(
                r.to_socket_addrs()
                    .with_context(|| format!("BP7_ROUTE={r}"))?
                    .next()
                    .ok_or_else(|| anyhow!("BP7_ROUTE={r} resolves to nothing"))?,
            ),
            None => None,
        };
        let allow = match var("BP7_ALLOW") {
            Some(list) => Some(
                list.split(',')
                    .map(|ip| ip.trim().parse().map_err(|e| anyhow!("BP7_ALLOW: {ip}: {e}")))
                    .collect::<Result<Vec<IpAddr>>>()?,
            ),
            None => None,
        };
        let blooms: usize = parsed("BP7_BLOOMS", 4)?;
        if blooms == 0 {
            bail!("BP7_BLOOMS must be at least 1");
        }
        Ok(Self {
            listen: parsed("BP7_LISTEN", "0.0.0.0:4556".parse()?)?,
            node_eid,
            label: var("BP7_LABEL").unwrap_or_else(|| "main".into()),
            model: var("BP7_MODEL").unwrap_or_else(|| "claude-sonnet-4".into()),
            route,
            allow,
            blooms,
            mtu: parsed("BP7_MTU", 1232)?,
            spool: var("BP7_SPOOL").map(PathBuf::from).unwrap_or_else(|| hum_paths::hive_state_dir(HIVE_NAME).join("spool")),
            hold: parsed("BP7_HOLD", 10_000)?,
            sweep: Duration::from_millis(parsed("BP7_SWEEP_MS", 60_000)?),
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn deadline_of(expires: u64) -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_millis(expires.saturating_sub(now_ms()))
}

struct Forager {
    cfg: Config,
    sock: Arc<UdpSocket>,
    spool: Mutex<Spool>,
    hum: Hum,
    queue: mpsc::UnboundedSender<String>,
    busy: Mutex<HashSet<String>>,
}

#[tokio::main]
async fn main() {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    if let Err(e) = run().await {
        let refused = e.downcast_ref::<hum_thrum::Refused>().is_some();
        tracing::error!(error = %format!("{e:#}"), "bp7.stopped");
        eprintln!("bp7-forager: {e:#}");
        std::process::exit(if refused { 3 } else { 1 });
    }
}

async fn run() -> Result<()> {
    let cfg = Config::from_env()?;
    let spool = Spool::open(&cfg.spool, cfg.hold)?;
    let bee = hum::link(&cfg.label, json!({
        "version": NESTLING_VERSION,
        "propensity": {"statefulness": "stateless", "richness": "lean", "wire": "bp7/dtn-udpcl"},
        "chis": ["hello", "prompt", "cancel", "unfurl", "wilt", "error"],
        "source": "https://github.com/adiled/hum/tree/main/hives/bp7",
    }))?;
    let sock = Arc::new(UdpSocket::bind(cfg.listen).await.with_context(|| format!("bind {}", cfg.listen))?);
    let (queue, asks) = mpsc::unbounded_channel();
    let me = Arc::new(Forager { cfg, sock, spool: Mutex::new(spool), hum: Hum::start(bee), queue, busy: Mutex::default() });
    info!(
        listen = %me.sock.local_addr()?, eid = %me.cfg.node_eid, label = %me.cfg.label, route = ?me.cfg.route,
        blooms = me.cfg.blooms, mtu = me.cfg.mtu, spool = %me.cfg.spool.display(),
        "bp7.listen"
    );

    me.sweep();
    for key in me.spool.lock().unwrap().waiting() {
        me.enqueue(key);
    }
    tokio::spawn(me.clone().grow_all(asks));
    tokio::spawn({
        let me = me.clone();
        async move {
            loop {
                tokio::time::sleep(me.cfg.sweep).await;
                me.sweep();
            }
        }
    });

    let (arrived, intake) = mpsc::channel::<(Vec<u8>, SocketAddr)>(INTAKE);
    tokio::task::spawn_blocking({
        let me = me.clone();
        move || me.intake(intake)
    });
    let mut buf = vec![0u8; DATAGRAM];
    loop {
        let (n, peer) = me.sock.recv_from(&mut buf).await?;
        if let Some(allow) = &me.cfg.allow
            && !allow.contains(&peer.ip())
        {
            debug!(%peer, "bundle.drop.not-allowed");
            continue;
        }
        if arrived.try_send((buf[..n].to_vec(), peer)).is_err() {
            warn!(%peer, "bundle.drop.intake-full — the forager is taking bundles slower than they arrive");
        }
    }
}

impl Forager {
    fn intake(self: Arc<Self>, mut arrived: mpsc::Receiver<(Vec<u8>, SocketAddr)>) {
        while let Some((bytes, peer)) = arrived.blocking_recv() {
            let taken = match dtn::arrive(&bytes, &self.cfg.node_eid, now_ms()) {
                Arrival::Dropped(why) => {
                    info!(%peer, why, "bundle.drop");
                    continue;
                }
                Arrival::Whole(ask, payload) => {
                    let taken = self.spool.lock().unwrap().take(&ask, &payload, peer);
                    (ask, taken)
                }
                Arrival::Piece { ask, offset, total, data } => {
                    let taken = self.spool.lock().unwrap().piece(&ask, offset, total, &data, peer);
                    (ask, taken)
                }
            };
            match taken {
                (ask, Ok(Taken::New(key))) => {
                    info!(id = %ask.id, key, "bundle.held");
                    self.enqueue(key);
                }
                (ask, Ok(Taken::Partial)) => debug!(id = %ask.id, "bundle.piece.held"),
                (ask, Ok(Taken::Duplicate)) => info!(id = %ask.id, "bundle.drop.duplicate"),
                (ask, Ok(Taken::Full)) => warn!(id = %ask.id, "bundle.drop.spool-full"),
                (ask, Err(e)) => warn!(id = %ask.id, error = %format!("{e:#}"), "bundle.drop.spool-failed"),
            }
        }
    }

    fn enqueue(&self, key: String) {
        if self.busy.lock().unwrap().insert(key.clone()) {
            let _ = self.queue.send(key);
        }
    }

    fn sweep(&self) {
        for key in self.spool.lock().unwrap().forget_expired(now_ms()) {
            info!(key, "bundle.expired.unanswered");
        }
    }

    async fn grow_all(self: Arc<Self>, mut asks: mpsc::UnboundedReceiver<String>) {
        let room = Arc::new(Semaphore::new(self.cfg.blooms));
        while let Some(key) = asks.recv().await {
            let permit = room.clone().acquire_owned().await.expect("the semaphore stays open");
            let me = self.clone();
            tokio::spawn(async move {
                let again = me.grow(&key).await;
                drop(permit);
                me.busy.lock().unwrap().remove(&key);
                if let Some(after) = again {
                    tokio::time::sleep(after).await;
                    me.enqueue(key);
                }
            });
        }
    }

    async fn grow(&self, key: &str) -> Option<Duration> {
        let loaded = self.spool.lock().unwrap().load(key);
        let Ok((held, payload, grown)) = loaded else {
            return None;
        };
        if held.expires <= now_ms() {
            info!(id = %held.id, "bundle.expired.unanswered");
            let _ = self.spool.lock().unwrap().heard(key, held.expires);
            return None;
        }
        let grown = match grown {
            Some(grown) => grown,
            None => {
                let attempt = match self.spool.lock().unwrap().attempt(key) {
                    Ok(n) => n,
                    Err(e) => {
                        warn!(id = %held.id, error = %format!("{e:#}"), "bundle.spool.failed");
                        return Some(hum::backoff(held.attempts));
                    }
                };
                let (text, model, system) = words(&payload, &self.cfg.model);
                let bloom = Bloom {
                    sid: format!("bp7-{}-{attempt}", &key[..16]),
                    text: &text,
                    model: &model,
                    system: system.as_deref(),
                    dusk: held.expires,
                };
                match self.hum.grow(bloom, deadline_of(held.expires)).await {
                    Outcome::Grown { ok, text } => {
                        let grown = Grown { ok, text };
                        if let Err(e) = self.spool.lock().unwrap().grown(key, &grown) {
                            warn!(id = %held.id, error = %format!("{e:#}"), "bundle.spool.failed");
                        }
                        grown
                    }
                    Outcome::Again(why) => {
                        info!(id = %held.id, attempt, %why, "bundle.again");
                        return Some(hum::backoff(attempt));
                    }
                    Outcome::Expired => {
                        info!(id = %held.id, "bundle.expired.unanswered");
                        let _ = self.spool.lock().unwrap().heard(key, held.expires);
                        return None;
                    }
                }
            }
        };
        match self.send_home(&held, &grown).await {
            Ok(n) => {
                info!(id = %held.id, ok = grown.ok, bundles = n, "bundle.answered");
                let _ = self.spool.lock().unwrap().heard(key, held.expires);
                None
            }
            Err(e) => {
                warn!(id = %held.id, error = %format!("{e:#}"), "bundle.send.failed");
                Some(hum::backoff(held.attempts))
            }
        }
    }

    async fn send_home(&self, held: &spool::Held, grown: &Grown) -> Result<usize> {
        let datagrams = dtn::reply(
            &self.cfg.node_eid,
            &dtn::Answer {
                to: &held.source,
                answers: &held.id,
                ok: grown.ok,
                text: grown.text.as_bytes(),
                lifetime: Duration::from_millis(held.lifetime_ms),
            },
            self.cfg.mtu,
        )?;
        let to = self.cfg.route.unwrap_or(held.peer);
        for d in &datagrams {
            self.sock.send_to(d, to).await.with_context(|| format!("send to {to}"))?;
        }
        Ok(datagrams.len())
    }
}

fn words(payload: &[u8], default_model: &str) -> (String, String, Option<String>) {
    if let Ok(v) = serde_json::from_slice::<Value>(payload)
        && let Some(text) = v.get("text").and_then(Value::as_str)
    {
        let model = v.get("modelId").and_then(Value::as_str).unwrap_or(default_model);
        let system = v.get("system").and_then(Value::as_str).map(str::to_string);
        return (text.to_string(), model.to_string(), system);
    }
    (String::from_utf8_lossy(payload).into_owned(), default_model.to_string(), None)
}
