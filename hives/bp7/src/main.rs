use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use hum_thrum::{Bee, Event, Options, Refused};
use thrum_core::{Chi, PetalKind};
use tokio::net::UdpSocket;
use tracing::{info, warn};

const HIVE_NAME: &str = "bp7";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_NODE_EID: &str = "dtn://hum.local/inference";
const DEFAULT_MODEL: &str = "claude-sonnet-4";
const BUNDLE_BUFFER_BYTES: usize = 65536;

const DEFAULT_LISTEN: &str = "0.0.0.0:4556";

#[derive(Debug, Clone)]
struct Config {
    listen: SocketAddr,
    node_eid: String,
    model: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        let listen_str = std::env::var("BP7_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.into());
        Ok(Self {
            listen: listen_str.parse().with_context(|| format!("parse BP7_LISTEN={listen_str}"))?,
            node_eid: std::env::var("BP7_NODE_EID").unwrap_or_else(|_| DEFAULT_NODE_EID.into()),
            model: std::env::var("BP7_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
        })
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
    let cfg = Arc::new(Config::from_env()?);
    let sock = UdpSocket::bind(cfg.listen).await
        .with_context(|| format!("bind {}", cfg.listen))?;
    let sock = Arc::new(sock);
    info!(listen = %cfg.listen, eid = %cfg.node_eid, "bp7-forager.listen");

    let mut buf = vec![0u8; BUNDLE_BUFFER_BYTES];
    loop {
        let (n, peer) = sock.recv_from(&mut buf).await?;
        let bytes = buf[..n].to_vec();
        let cfg = cfg.clone();
        let sock = sock.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_bundle(cfg, sock, peer, bytes).await {
                warn!(error = ?e, %peer, "bundle.handle");
            }
        });
    }
}

async fn handle_bundle(
    cfg: Arc<Config>,
    sock: Arc<UdpSocket>,
    peer: SocketAddr,
    bytes: Vec<u8>,
) -> Result<()> {
    let bndl = bundle_protocol::Bundle::try_from(bytes.as_slice())
        .map_err(|e| anyhow!("bp7 decode: {e:?}"))?;

    let dest = bndl.primary.destination.to_string();
    let src = bndl.primary.source.to_string();
    if !addressed_to_us(&dest, &cfg.node_eid) {
        info!(%dest, %src, "bundle.drop.not-for-us");
        return Ok(());
    }

    let payload = bndl.payload().ok_or_else(|| anyhow!("bundle has no payload block"))?;
    let prompt = parse_payload(payload, &cfg.model);
    info!(%src, %dest, len = prompt.text.len(), "bundle.recv");

    let reply_text = run_prompt(&cfg, &prompt).await?;
    let mut reply_bundle = build_reply(&cfg.node_eid, &src, reply_text.as_bytes());

    let cbor = reply_bundle.to_cbor();
    sock.send_to(&cbor, peer).await
        .with_context(|| format!("reply send to {peer}"))?;
    info!(%src, dest = %src, bytes = cbor.len(), "bundle.send.reply");
    Ok(())
}

fn addressed_to_us(dest: &str, node_eid: &str) -> bool {
    dest == node_eid || dest.starts_with(node_eid)
}

struct Payload {
    text: String,
    model: String,
    system: Option<String>,
}

fn parse_payload(payload: &[u8], default_model: &str) -> Payload {
    if let Ok(v) = serde_json::from_slice::<Value>(payload) {
        if let Some(text) = v.get("text").and_then(Value::as_str) {
            let model = v.get("modelId").and_then(Value::as_str)
                .unwrap_or(default_model).to_string();
            let system = v.get("system").and_then(Value::as_str).map(str::to_string);
            return Payload { text: text.to_string(), model, system };
        }
    }
    Payload {
        text: String::from_utf8_lossy(payload).into_owned(),
        model: default_model.to_string(),
        system: None,
    }
}

fn unfurled_text(tone: &Value) -> Option<&str> {
    if tone.get("kind").and_then(Value::as_str) != Some(PetalKind::Text.as_str()) {
        return None;
    }
    tone.get("text").and_then(Value::as_str)
}

fn build_reply(my_eid: &str, source_eid: &str, payload: &[u8]) -> bundle_protocol::Bundle {
    let src = bundle_protocol::EndpointID::try_from(my_eid).expect("our EID parses");
    let dst = bundle_protocol::EndpointID::try_from(source_eid).unwrap_or_else(|_| {
        warn!(source_eid, "could not parse source EID; reply will go to dtn:none");
        bundle_protocol::EndpointID::none()
    });
    bundle_protocol::bundle::new_std_payload_bundle(src, dst, payload.to_vec())
}

async fn run_prompt(_cfg: &Config, payload: &Payload) -> Result<String> {
    let hello = json!({
        "roles": ["forager"],
        "version": NESTLING_VERSION,
        "propensity": {
            "statefulness": "stateless",
            "richness":     "lean",
            "wire":         "bp7/dtn-udpcl",
        },
        "chis": ["hello", "prompt", "unfurl", "wilt", "error"],
        "source": "https://github.com/adiled/hum/tree/main/hives/bp7",
    });
    let mut bee = Bee::link(Options::new(HIVE_NAME, "main").hello(hello))
        .await
        .context("link the bp7 bee")?;

    let sid = format!("bp7-{}", now_ms());
    let mut prompt = serde_json::Map::new();
    prompt.insert("chi".into(), json!(Chi::Prompt));
    prompt.insert("rid".into(), Value::String(format!("p-{sid}")));
    prompt.insert("mid".into(), Value::String(hum_identity::HumId::mint().to_string()));
    prompt.insert("sid".into(), Value::String(sid.clone()));
    prompt.insert("text".into(), Value::String(payload.text.clone()));
    prompt.insert("modelId".into(), Value::String(payload.model.clone()));
    if let Some(system) = payload.system.as_ref() {
        prompt.insert("systemPrompt".into(), Value::String(system.clone()));
    }
    bee.send(Value::Object(prompt));

    let mut collected = String::new();
    let answer = loop {
        let tone = match bee.next().await {
            Some(Event::Tone(tone)) => tone,
            Some(Event::Astray) => break Err(anyhow!("humd went away mid-bloom")),
            Some(Event::Refused(why)) => break Err(Refused(why).into()),
            Some(Event::Nestled | Event::Advisory(_)) => continue,
            None => break Err(anyhow!("the client stopped mid-bloom")),
        };
        if tone.get("sid").and_then(Value::as_str) != Some(sid.as_str()) {
            continue;
        }
        match tone.get("chi").and_then(Value::as_str) {
            Some("unfurl") => {
                if let Some(text) = unfurled_text(&tone) {
                    collected.push_str(text);
                }
            }
            Some("wilt") => break Ok(collected),
            Some("error") => {
                let msg = tone.get("message").and_then(Value::as_str).unwrap_or("stream error");
                break Err(anyhow!("humd error: {msg}"));
            }
            _ => {}
        }
    };
    bee.close();
    answer
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
