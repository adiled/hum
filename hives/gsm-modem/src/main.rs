//! `gsm-modem` — drive a USB GSM modem over AT commands.
//!
//! Tested-shape modems:
//!   - Huawei E303 / E353 / E3531 (cheap dongle, appears as
//!     /dev/ttyUSB[0-3] — pick the AT-port, usually ttyUSB0)
//!   - SIMCom SIM800 / SIM900 / SIM7600
//!   - Quectel EC25 / EG91
//!
//! Init sequence:
//!   AT                       — alive
//!   AT+CMGF=1                — text mode (not PDU)
//!   AT+CSCS="GSM"            — default 7-bit charset
//!   AT+CNMI=2,2,0,0,0        — push new SMS as +CMT URCs
//!
//! Receive:
//!   +CMT: "+14155551234","","25/05/17,12:00:00+00"
//!   Hi there
//!
//! Send:
//!   AT+CMGS="+14155551234"<CR>
//!   Hi back<Ctrl-Z>
//!
//! Per-phone-number sid keeps the conversation continuous (same
//! convention as `hives/twilio-sms`).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bytes::BytesMut;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thrum_core::{Chi, THRUM_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tokio_util::codec::{Decoder, FramedRead};
use tracing::{info, warn};

const NESTLING_NAME: &str = "gsm-modem";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");
const CTRL_Z: u8 = 0x1A;

#[derive(Debug, Clone)]
struct Config {
    device: String,
    baud: u32,
    model: String,
    system: String,
    reply_limit: usize,
    sock_path: String,
}

impl Config {
    fn from_env() -> Self {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
            format!("/run/user/{}", unsafe { libc::geteuid() })
        });
        Self {
            device: std::env::var("HUM_GSM_DEVICE").unwrap_or_else(|_| "/dev/ttyUSB0".into()),
            baud: std::env::var("HUM_GSM_BAUD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(115200),
            model: std::env::var("HUM_GSM_MODEL").unwrap_or_else(|_| "claude-haiku-4.5".into()),
            system: std::env::var("HUM_GSM_SYSTEM").unwrap_or_else(|_| {
                "You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS.".into()
            }),
            reply_limit: std::env::var("HUM_GSM_REPLY_LIMIT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1500),
            sock_path: std::env::var("HUM_THRUM_SOCK")
                .unwrap_or_else(|_| format!("{runtime}/hum/thrum.sock")),
        }
    }
}

fn sigil_for_phone(phone: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"gsm-modem:");
    h.update(phone.as_bytes());
    hex::encode(&h.finalize()[..8])
}

/// Newline-framed line decoder over the modem byte stream.
struct LineCodec;
impl Decoder for LineCodec {
    type Item = String;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<String>, Self::Error> {
        if let Some(nl) = src.iter().position(|&b| b == b'\n') {
            let line = src.split_to(nl + 1);
            let line = std::str::from_utf8(&line[..nl])
                .unwrap_or("")
                .trim_end_matches('\r')
                .to_string();
            return Ok(Some(line));
        }
        Ok(None)
    }
}

#[derive(Debug, Clone)]
struct IncomingSms {
    from: String,
    body: String,
}

async fn handle_sms(cfg: Arc<Config>, sms: IncomingSms, writer: Arc<Mutex<SerialStream>>) -> Result<()> {
    let sid = sigil_for_phone(&sms.from);

    let sock = UnixStream::connect(&cfg.sock_path)
        .await
        .with_context(|| format!("connect {}", cfg.sock_path))?;
    let (rd, mut wr) = sock.into_split();
    let mut lines = BufReader::new(rd).lines();

    // Persisted forager identity — humd dedupes by this fbee_ hid.
    let hid = nest_common::load_or_mint_bee_key(NESTLING_NAME, ensemble::HidPrefix::Fbee)
        .map(|k| k.hid.to_hex())
        .unwrap_or_default();

    // Hello (per WIRE.md §Handshake).
    let hello = json!({
        "chi": Chi::Hello,
        "rid": format!("hello-{}", now_ms()),
        "from": NESTLING_NAME,
        "hid": hid,
        "bee": ["forager"],
        "version": NESTLING_VERSION,
        "protoVersion": THRUM_VERSION,
        "propensity": { "statefulness": "stateful", "richness": "lean", "wire": "gsm/at-cmd-sms" },
        "chis": ["hello", "prompt", "chunk", "finish", "error"],
        "source": "https://github.com/adiled/hum/tree/main/hives/gsm-modem",
    });
    write_line(&mut wr, &hello).await?;

    // Prompt.
    let prompt = json!({
        "chi": Chi::Prompt,
        "rid": format!("sms-{}", now_ms()),
        "sid": sid,
        "text": sms.body,
        "modelId": cfg.model,
        "systemPrompt": cfg.system,
        "ext": { "gsm-modem": { "from": sms.from } },
    });
    write_line(&mut wr, &prompt).await?;

    // Collect chunks until finish/error.
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
            Some("finish") | Some("error") => break,
            _ => {}
        }
    }

    let mut reply = collected.trim().to_string();
    if reply.is_empty() {
        reply = "(no reply)".into();
    }
    if reply.len() > cfg.reply_limit {
        reply.truncate(cfg.reply_limit.saturating_sub(1));
        reply.push('…');
    }

    send_sms(&writer, &sms.from, &reply).await?;
    Ok(())
}

async fn write_line(wr: &mut tokio::net::unix::OwnedWriteHalf, tone: &Value) -> Result<()> {
    let mut buf = serde_json::to_string(tone)?;
    buf.push('\n');
    wr.write_all(buf.as_bytes()).await?;
    Ok(())
}

async fn send_sms(writer: &Arc<Mutex<SerialStream>>, to: &str, body: &str) -> Result<()> {
    let cmd = format!("AT+CMGS=\"{}\"\r", to);
    {
        let mut w = writer.lock().await;
        w.write_all(cmd.as_bytes()).await?;
        w.flush().await?;
    }
    // Brief breath so the modem prints "> " before we send the body.
    tokio::time::sleep(Duration::from_millis(200)).await;
    {
        let mut w = writer.lock().await;
        w.write_all(body.as_bytes()).await?;
        w.write_all(&[CTRL_Z]).await?;
        w.flush().await?;
    }
    Ok(())
}

async fn init_modem(writer: &Arc<Mutex<SerialStream>>) -> Result<()> {
    for cmd in ["AT", "AT+CMGF=1", "AT+CSCS=\"GSM\"", "AT+CNMI=2,2,0,0,0"] {
        {
            let mut w = writer.lock().await;
            w.write_all(cmd.as_bytes()).await?;
            w.write_all(b"\r").await?;
            w.flush().await?;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cfg = Arc::new(Config::from_env());
    info!(device = %cfg.device, baud = cfg.baud, "gsm-modem.open");

    // Split serial: writes go through the shared Mutex; reads use a
    // separate handle wrapped in FramedRead.
    let reader_port = tokio_serial::new(&cfg.device, cfg.baud)
        .open_native_async()
        .with_context(|| format!("open {}", cfg.device))?;
    let writer_port = tokio_serial::new(&cfg.device, cfg.baud)
        .open_native_async()
        .with_context(|| format!("open {} (writer)", cfg.device))?;
    let writer = Arc::new(Mutex::new(writer_port));
    let mut frames = FramedRead::new(reader_port, LineCodec);

    init_modem(&writer).await?;
    info!("gsm-modem.ready");

    use futures_util::StreamExt;
    let mut pending_cmt: Option<String> = None;
    while let Some(line_res) = frames.next().await {
        let line: String = match line_res {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "serial.read");
                continue;
            }
        };
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("+CMT:") {
            // Format: +CMT: "<from>","","<timestamp>"
            pending_cmt = rest
                .splitn(2, '"')
                .nth(1)
                .and_then(|s| s.split('"').next())
                .map(str::to_string);
            continue;
        }
        if let Some(from) = pending_cmt.take() {
            let sms = IncomingSms { from, body: line };
            let cfg = cfg.clone();
            let writer = writer.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_sms(cfg, sms, writer).await {
                    warn!(error = ?e, "handle_sms");
                }
            });
        }
        // OK / ERROR / unsolicited URCs we don't react to are dropped.
    }
    Err(anyhow!("serial stream ended"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
