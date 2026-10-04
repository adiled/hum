use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use thrum_core::{Chi, THRUM_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UdpSocket, UnixStream};
use tracing::{info, warn};

const HIVE_NAME: &str = "bp7-forager";
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
    sock_path: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        let listen_str = std::env::var("BP7_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.into());
        Ok(Self {
            listen: listen_str.parse().with_context(|| format!("parse BP7_LISTEN={listen_str}"))?,
            node_eid: std::env::var("BP7_NODE_EID").unwrap_or_else(|_| DEFAULT_NODE_EID.into()),
            model: std::env::var("BP7_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
            sock_path: hum_paths::thrum_sock_resolved().to_string_lossy().into_owned(),
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

fn chunk_text(tone: &Value) -> Option<&str> {
    let part = tone.get("part")?;
    if part.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    part.get("text").and_then(Value::as_str)
}

fn build_reply(my_eid: &str, source_eid: &str, payload: &[u8]) -> bundle_protocol::Bundle {
    let src = bundle_protocol::EndpointID::try_from(my_eid).expect("our EID parses");
    let dst = bundle_protocol::EndpointID::try_from(source_eid).unwrap_or_else(|_| {
        warn!(source_eid, "could not parse source EID; reply will go to dtn:none");
        bundle_protocol::EndpointID::none()
    });
    bundle_protocol::bundle::new_std_payload_bundle(src, dst, payload.to_vec())
}

async fn run_prompt(cfg: &Config, payload: &Payload) -> Result<String> {
    let stream = UnixStream::connect(&cfg.sock_path).await
        .with_context(|| format!("connect {}", cfg.sock_path))?;
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();

    let hid = hum_identity::load_or_mint_bee_key(HIVE_NAME, hum_identity::HidPrefix::Fbee)
        .map(|k| k.hid.to_hex())
        .unwrap_or_default();

    let hello = json!({
        "chi": Chi::Hello,
        "rid": hum_identity::HumId::mint().to_string(),
        "from": HIVE_NAME,
        "hid": hid,
        "bee": ["forager"],
        "version": NESTLING_VERSION,
        "protoVersion": THRUM_VERSION,
        "propensity": {
            "statefulness": "stateless",
            "richness":     "lean",
            "wire":         "bp7/dtn-udpcl",
        },
        "chis": ["hello", "prompt", "chunk", "finish", "error"],
        "source": "https://github.com/adiled/hum/tree/main/hives/bp7",
    });
    write_line(&mut wr, &hello).await?;

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
    write_line(&mut wr, &Value::Object(prompt)).await?;

    let mut collected = String::new();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if tone.get("sid").and_then(Value::as_str) != Some(sid.as_str()) {
            continue;
        }
        match tone.get("chi").and_then(Value::as_str) {
            Some("chunk") => {
                if let Some(text) = chunk_text(&tone) {
                    collected.push_str(text);
                }
            }
            Some("finish") => break,
            Some("error") => {
                let msg = tone.get("message").and_then(Value::as_str).unwrap_or("stream error");
                return Err(anyhow!("humd error: {msg}"));
            }
            _ => {}
        }
    }
    Ok(collected)
}

async fn write_line(wr: &mut tokio::net::unix::OwnedWriteHalf, tone: &Value) -> Result<()> {
    let mut buf = serde_json::to_string(tone)?;
    buf.push('\n');
    wr.write_all(buf.as_bytes()).await?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
