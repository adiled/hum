//! `bp7-forager` — DTN bridge to hum. RFC 9171 Bundle Protocol v7.
//!
//! The interplanetary internet protocol. Store-and-forward across
//! light-second-to-light-hour delays. NASA's ION speaks it, ESA tests
//! it on the ISS, lunar Gateway plans to use it. Real protocol, used
//! today for the actual solar system.
//!
//! This bee is a **leaf node** — we terminate bundles addressed
//! to our EID and don't forward to other DTN nodes. The convergence
//! layer is UDP (UDPCL, the simplest of the BP7 transports). Plug into
//! a real DTN router (ION, µPCN, hDTN) by pointing your router's
//! routing table at our UDP port; bundles for our EID will arrive,
//! and our replies will be one-hop bundles back to the bundle source.
//!
//! Inbound:
//!   1. UDP receive — CBOR-encoded BP7 bundle
//!   2. Decode; check destination EID matches our service
//!   3. Extract payload bytes; parse as either plain UTF-8 (treated as
//!      the prompt text) or JSON `{ text, modelId?, system? }`
//!   4. Open thrum, send `chi:"hello"` + `chi:"prompt"`, collect
//!      `chi:"chunk"` until `chi:"finish"`
//!   5. Build a reply bundle: dest = original source EID, source =
//!      our EID, payload = the accumulated text
//!   6. Send the reply back over UDP to the same peer (assume direct
//!      adjacency for v1)
//!
//! Latency model: BP7 was designed for one-way delays of MINUTES to
//! HOURS. We don't care how long humd takes to answer — there's no
//! timeout on the bee side. The DTN router on the other end
//! handles custody-transfer + retries.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use thrum_core::{Chi, THRUM_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UdpSocket, UnixStream};
use tracing::{info, warn};

const NESTLING_NAME: &str = "bp7-forager";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default DTN convergence-layer port for UDP. ION and µPCN both
/// listen here by default.
const DEFAULT_LISTEN: &str = "0.0.0.0:4556";

#[derive(Debug, Clone)]
struct Config {
    listen: SocketAddr,
    /// EID of THIS node — e.g. `dtn://hum.local/inference`. Bundles
    /// addressed to this EID get handled; everything else is dropped.
    node_eid: String,
    model: String,
    sock_path: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
            format!("/run/user/{}", unsafe { libc::geteuid() })
        });
        let listen_str = std::env::var("BP7_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.into());
        Ok(Self {
            listen: listen_str.parse().with_context(|| format!("parse BP7_LISTEN={listen_str}"))?,
            node_eid: std::env::var("BP7_NODE_EID")
                .unwrap_or_else(|_| "dtn://hum.local/inference".into()),
            model: std::env::var("BP7_MODEL").unwrap_or_else(|_| "claude-sonnet-4".into()),
            sock_path: std::env::var("HUM_THRUM_SOCK")
                .unwrap_or_else(|_| format!("{runtime}/hum/thrum.sock")),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
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

    let mut buf = vec![0u8; 65536];
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
    // bp7's Bundle::try_from(&[u8]) returns the decoded bundle.
    let bndl = bundle_protocol::Bundle::try_from(bytes.as_slice())
        .map_err(|e| anyhow!("bp7 decode: {e:?}"))?;

    let dest = bndl.primary.destination.to_string();
    let src = bndl.primary.source.to_string();
    if dest != cfg.node_eid && !dest.starts_with(&cfg.node_eid) {
        // Not for us — drop. Real DTN routers would forward; we don't.
        info!(%dest, %src, "bundle.drop.not-for-us");
        return Ok(());
    }

    let payload = bndl.payload().ok_or_else(|| anyhow!("bundle has no payload block"))?;
    let (text, model, system) = parse_payload(payload, &cfg.model);
    info!(%src, %dest, len = text.len(), "bundle.recv");

    let reply_text = run_prompt(&cfg, &text, &model, system.as_deref()).await?;
    let mut reply_bundle = build_reply(&cfg.node_eid, &src, reply_text.as_bytes());

    let cbor = reply_bundle.to_cbor();
    sock.send_to(&cbor, peer).await
        .with_context(|| format!("reply send to {peer}"))?;
    info!(%src, dest = %src, bytes = cbor.len(), "bundle.send.reply");
    Ok(())
}

/// Payload may be:
///   - plain UTF-8 text → that's the prompt
///   - JSON `{ "text": "...", "modelId": "...", "system": "..." }`
fn parse_payload(payload: &[u8], default_model: &str) -> (String, String, Option<String>) {
    if let Ok(v) = serde_json::from_slice::<Value>(payload) {
        if let Some(text) = v.get("text").and_then(Value::as_str) {
            let model = v.get("modelId").and_then(Value::as_str)
                .unwrap_or(default_model).to_string();
            let system = v.get("system").and_then(Value::as_str).map(str::to_string);
            return (text.to_string(), model, system);
        }
    }
    let text = String::from_utf8_lossy(payload).into_owned();
    (text, default_model.to_string(), None)
}

fn build_reply(my_eid: &str, source_eid: &str, payload: &[u8]) -> bundle_protocol::Bundle {
    let src = bundle_protocol::EndpointID::try_from(my_eid).expect("our EID parses");
    let dst = bundle_protocol::EndpointID::try_from(source_eid).unwrap_or_else(|_| {
        warn!(source_eid, "could not parse source EID; reply will go to dtn:none");
        bundle_protocol::EndpointID::none()
    });
    bundle_protocol::bundle::new_std_payload_bundle(src, dst, payload.to_vec())
}

async fn run_prompt(
    cfg: &Config,
    text: &str,
    model: &str,
    system: Option<&str>,
) -> Result<String> {
    let stream = UnixStream::connect(&cfg.sock_path).await
        .with_context(|| format!("connect {}", cfg.sock_path))?;
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();

    // Persisted forager identity — humd dedupes by this fbee_ hid.
    let hid = nest_common::load_or_mint_bee_key(NESTLING_NAME, ensemble::HidPrefix::Fbee)
        .map(|k| k.hid.to_hex())
        .unwrap_or_default();

    let hello = json!({
        "chi": Chi::Hello,
        "rid": format!("hello-{}", now_ms()),
        "from": NESTLING_NAME,
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
    prompt.insert("sid".into(), Value::String(sid.clone()));
    prompt.insert("text".into(), Value::String(text.to_string()));
    prompt.insert("modelId".into(), Value::String(model.to_string()));
    if let Some(s) = system {
        prompt.insert("systemPrompt".into(), Value::String(s.to_string()));
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
                if let Some(part) = tone.get("part") {
                    if part.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            collected.push_str(t);
                        }
                    }
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
