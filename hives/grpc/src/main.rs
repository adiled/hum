//! grpc-forager: the thrum over one gRPC bidi stream, `hum.Hum/Stream`.
//!
//! The process is one forager bee, the hive's `main`, linked to humd when
//! the first stream opens and shared by every stream after it, so humd
//! knows one forager however many clients stream. Nothing is translated:
//! gRPC is the medium, and each client speaks the forager's side of the
//! thrum itself. A tone a stream sends goes to humd whole, except a
//! `hello`, which is the bridge's: a stream cannot change the bee every
//! stream shares. A tone humd sends the bee goes back whole to the streams
//! it is for: a breath to every stream, an echo to the stream that sent
//! its rid, a sid's tones to each stream that prompted, attached or
//! reached under it, and anything else to every stream.
//!
//! A stream lasts until the client closes its side and the blooms it
//! opened have wilted or broken off, or until the client goes away, which
//! cancels the blooms it alone held. humd going away loses every open
//! bloom; what a stream sends meanwhile waits until humd is back. A
//! refusal of the bee ends every stream with its reason, and the next
//! stream links again.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use hum_thrum::{Bee, Event, Options, Refused};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status, Streaming, transport::Server};
use tracing::{debug, info, warn};

pub mod hum {
    tonic::include_proto!("hum");
}

use hum::{
    Tone,
    hum_server::{Hum, HumServer},
};

const HIVE_NAME: &str = "grpc";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");
const RIDS_KEPT: usize = 4096;

type Out = mpsc::UnboundedSender<Result<Tone, Status>>;

enum Cmd {
    Join { id: u64, out: Out },
    Tone { id: u64, tone: Value },
    Closed { id: u64 },
    Gone { id: u64 },
}

#[derive(Clone)]
struct Link {
    cmds: mpsc::UnboundedSender<Cmd>,
    nestled: watch::Receiver<bool>,
}

async fn link(held: &Mutex<Option<Link>>) -> Result<Link> {
    let mut held = held.lock().await;
    if let Some(link) = held.as_ref()
        && !link.cmds.is_closed()
    {
        return Ok(link.clone());
    }
    let hello = json!({
        "version": NESTLING_VERSION,
        "propensity": {"statefulness": "transport-only", "richness": "opaque", "wire": "grpc/bidi"},
        "source": "https://github.com/adiled/hum/tree/main/hives/grpc",
    });
    let bee = Bee::link(Options::new(HIVE_NAME, "main", thrum_core::Role::Forager).hello(hello)).await?;
    let (cmds, rx) = mpsc::unbounded_channel();
    let (here, nestled) = watch::channel(true);
    tokio::spawn(dispatch(bee, rx, here));
    let link = Link { cmds, nestled };
    *held = Some(link.clone());
    Ok(link)
}

fn refusal(e: &anyhow::Error) -> Status {
    match e.downcast_ref::<Refused>() {
        Some(refused) => Status::failed_precondition(refused.to_string()),
        None => Status::unavailable(format!("{e:#}")),
    }
}

async fn carry(id: u64, mut incoming: Streaming<Tone>, link: Link, mut gone: oneshot::Receiver<()>) {
    let mut nestled = link.nestled.clone();
    loop {
        tokio::select! {
            next = incoming.next() => match next {
                Some(Ok(wire)) => {
                    let Some(tone) = tone_of(wire) else { continue };
                    if nestled.wait_for(|n| *n).await.is_err() {
                        return;
                    }
                    let _ = link.cmds.send(Cmd::Tone { id, tone });
                }
                Some(Err(e)) => {
                    debug!(id, error = %e, "grpc.stream.gone");
                    break;
                }
                None => {
                    let _ = link.cmds.send(Cmd::Closed { id });
                    let _ = gone.await;
                    break;
                }
            },
            _ = &mut gone => break,
        }
    }
    let _ = link.cmds.send(Cmd::Gone { id });
}

fn tone_of(wire: Tone) -> Option<Value> {
    if wire.body.is_empty() {
        let mut tone = json!({"chi": wire.chi});
        for (k, v) in [("sid", wire.sid), ("rid", wire.rid)] {
            if !v.is_empty() {
                tone[k] = json!(v);
            }
        }
        return Some(tone);
    }
    serde_json::from_slice::<Value>(&wire.body).ok().filter(Value::is_object)
}

fn wire(tone: &Value) -> Tone {
    let lifted = |k: &str| tone.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Tone { chi: lifted("chi"), sid: lifted("sid"), rid: lifted("rid"), body: tone.to_string().into_bytes() }
}

fn text(tone: &Value, key: &str) -> Option<String> {
    tone.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

struct Held {
    out: Out,
    growing: HashMap<String, usize>,
    closed: bool,
}

#[derive(Default)]
struct Streams {
    by_id: HashMap<u64, Held>,
    sids: HashMap<String, HashSet<u64>>,
    rids: HashMap<String, (u64, Option<String>)>,
    order: VecDeque<String>,
    breath: Option<Value>,
}

impl Streams {
    fn join(&mut self, id: u64, out: Out) {
        if let Some(breath) = &self.breath {
            let _ = out.send(Ok(wire(breath)));
        }
        self.by_id.insert(id, Held { out, growing: HashMap::new(), closed: false });
    }

    fn send(&self, id: u64, tone: &Value) {
        if let Some(held) = self.by_id.get(&id) {
            let _ = held.out.send(Ok(wire(tone)));
        }
    }

    fn client_sent(&mut self, id: u64, tone: Value) -> Option<Value> {
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
        let (sid, rid) = (text(&tone, "sid"), text(&tone, "rid"));
        if chi == "hello" {
            if let Some(rid) = rid {
                let why = "the hello is the bridge's: every stream is the same bee";
                self.send(id, &json!({"chi": "echo", "rid": rid, "ok": false, "error": why}));
            }
            return None;
        }
        if let Some(rid) = rid {
            self.rids.insert(rid.clone(), (id, sid.clone().filter(|_| chi == "prompt")));
            self.order.push_back(rid);
            if self.order.len() > RIDS_KEPT
                && let Some(old) = self.order.pop_front()
            {
                self.rids.remove(&old);
            }
        }
        if let Some(sid) = sid {
            match chi {
                "prompt" | "attach" | "tendril-reach" => {
                    self.sids.entry(sid.clone()).or_default().insert(id);
                    if chi == "prompt"
                        && let Some(held) = self.by_id.get_mut(&id)
                    {
                        *held.growing.entry(sid).or_default() += 1;
                    }
                }
                "detach" | "cleanup" => {
                    if let Some(held) = self.by_id.get_mut(&id) {
                        held.growing.remove(&sid);
                    }
                    self.release(id, &sid);
                }
                _ => {}
            }
        }
        Some(tone)
    }

    fn humd_sent(&mut self, tone: Value) {
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("").to_string();
        if chi == "breath" {
            for id in self.by_id.keys() {
                self.send(*id, &tone);
            }
            self.breath = Some(tone);
            return;
        }
        if chi == "echo" {
            let Some((id, prompted)) = text(&tone, "rid").and_then(|r| self.rids.get(&r).cloned()) else { return };
            self.send(id, &tone);
            if tone.get("ok") == Some(&Value::Bool(false))
                && let Some(sid) = prompted
            {
                self.settle(id, &sid);
            }
            return;
        }
        let Some(sid) = text(&tone, "sid") else {
            for id in self.by_id.keys() {
                self.send(*id, &tone);
            }
            return;
        };
        let holders: Vec<u64> = self.sids.get(&sid).map(|h| h.iter().copied().collect()).unwrap_or_default();
        for id in holders {
            self.send(id, &tone);
            if chi == "wilt" || chi == "error" {
                self.settle(id, &sid);
            }
        }
    }

    fn settle(&mut self, id: u64, sid: &str) {
        if let Some(held) = self.by_id.get_mut(&id)
            && let Some(n) = held.growing.get_mut(sid)
        {
            *n -= 1;
            if *n == 0 {
                held.growing.remove(sid);
            }
        }
        self.end_if_done(id);
    }

    fn closed(&mut self, id: u64) {
        if let Some(held) = self.by_id.get_mut(&id) {
            held.closed = true;
        }
        self.end_if_done(id);
    }

    fn end_if_done(&mut self, id: u64) {
        if self.by_id.get(&id).is_some_and(|h| h.closed && h.growing.is_empty()) {
            self.end(id);
        }
    }

    fn end(&mut self, id: u64) -> Option<Held> {
        let held = self.by_id.remove(&id)?;
        let sids: Vec<String> = self.sids.iter().filter(|(_, h)| h.contains(&id)).map(|(s, _)| s.clone()).collect();
        for sid in sids {
            self.release(id, &sid);
        }
        self.rids.retain(|_, (holder, _)| *holder != id);
        Some(held)
    }

    fn release(&mut self, id: u64, sid: &str) {
        if let Some(holders) = self.sids.get_mut(sid) {
            holders.remove(&id);
            if holders.is_empty() {
                self.sids.remove(sid);
            }
        }
    }

    fn gone(&mut self, id: u64) -> Vec<String> {
        let Some(held) = self.end(id) else { return vec![] };
        held.growing.into_keys().filter(|sid| !self.sids.contains_key(sid)).collect()
    }

    fn astray(&mut self) {
        self.breath = None;
        let lost = Status::unavailable("humd went away; the blooms open on this stream are lost");
        let ids: Vec<u64> = self.by_id.keys().copied().collect();
        for id in ids {
            let held = self.by_id.get_mut(&id).expect("listed");
            held.growing.clear();
            if held.closed {
                let _ = held.out.send(Err(lost.clone()));
                self.end(id);
            }
        }
    }

    fn stop(&mut self, cmds: &mut mpsc::UnboundedReceiver<Cmd>, status: Status) {
        cmds.close();
        while let Ok(cmd) = cmds.try_recv() {
            if let Cmd::Join { out, .. } = cmd {
                let _ = out.send(Err(status.clone()));
            }
        }
        for (_, held) in self.by_id.drain() {
            let _ = held.out.send(Err(status.clone()));
        }
    }
}

async fn dispatch(mut bee: Bee, mut cmds: mpsc::UnboundedReceiver<Cmd>, nestled: watch::Sender<bool>) {
    let mut streams = Streams::default();
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(Cmd::Join { id, out }) => streams.join(id, out),
                Some(Cmd::Tone { id, tone }) => {
                    if let Some(tone) = streams.client_sent(id, tone) {
                        bee.send(tone);
                    }
                }
                Some(Cmd::Closed { id }) => streams.closed(id),
                Some(Cmd::Gone { id }) => {
                    for sid in streams.gone(id) {
                        bee.send(json!({"chi": "cancel", "sid": sid}));
                    }
                }
                None => return,
            },
            event = bee.next() => match event {
                Some(Event::Tone(tone)) => streams.humd_sent(tone),
                Some(Event::Nestled) => {
                    let _ = nestled.send(true);
                }
                Some(Event::Astray) => {
                    let _ = nestled.send(false);
                    streams.astray();
                }
                Some(Event::Advisory(_)) => {}
                Some(Event::Refused(why)) => {
                    warn!(%why, "grpc.refused");
                    return streams.stop(&mut cmds, Status::failed_precondition(Refused(why).to_string()));
                }
                None => return streams.stop(&mut cmds, Status::unavailable("the thrum client stopped")),
            },
        }
    }
}

#[derive(Default)]
struct HumBridge {
    link: Arc<Mutex<Option<Link>>>,
    streams: AtomicU64,
}

#[tonic::async_trait]
impl Hum for HumBridge {
    type StreamStream = Pin<Box<dyn Stream<Item = Result<Tone, Status>> + Send + 'static>>;

    async fn stream(&self, req: Request<Streaming<Tone>>) -> Result<Response<Self::StreamStream>, Status> {
        let incoming = req.into_inner();
        let id = self.streams.fetch_add(1, Ordering::Relaxed);
        let (out, mut rx) = mpsc::unbounded_channel();
        let (gone_tx, gone) = oneshot::channel::<()>();
        let reply = async_stream::stream! {
            let _dropped_with_the_reply = gone_tx;
            while let Some(tone) = rx.recv().await {
                yield tone;
            }
        };
        let held = self.link.clone();
        tokio::spawn(async move {
            match link(&held).await {
                Ok(link) => match link.cmds.send(Cmd::Join { id, out }) {
                    Ok(()) => carry(id, incoming, link, gone).await,
                    Err(mpsc::error::SendError(Cmd::Join { out, .. })) => {
                        let _ = out.send(Err(Status::unavailable("the bee stopped as the stream opened")));
                    }
                    Err(_) => {}
                },
                Err(e) => {
                    warn!(error = %format!("{e:#}"), "grpc.refused");
                    let _ = out.send(Err(refusal(&e)));
                }
            }
        });
        Ok(Response::new(Box::pin(reply) as Self::StreamStream))
    }
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

#[tokio::main]
async fn main() -> Result<()> {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let host: IpAddr = parsed("HUM_GRPC_HOST", Ipv4Addr::LOCALHOST.into())?;
    let port: u16 = parsed("HUM_GRPC_PORT", 14621)?;
    let listener = tokio::net::TcpListener::bind((host, port)).await.with_context(|| format!("bind {host}:{port}"))?;
    info!(addr = %listener.local_addr()?, "grpc.listen");
    let incoming = TcpIncoming::from_listener(listener, true, None).map_err(|e| anyhow!("{e}"))?;
    Server::builder()
        .add_service(HumServer::new(HumBridge::default()))
        .serve_with_incoming(incoming)
        .await?;
    Ok(())
}
