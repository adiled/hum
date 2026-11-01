//! `gsm-modem`: hum on a burner phone. A forager that reads texts from a
//! USB GSM modem over AT commands and answers each with a bloom.
//!
//! Tested-shape modems:
//!   - Huawei E303 / E353 / E3531 (cheap dongle, appears as
//!     /dev/ttyUSB[0-3]; pick the AT port, usually ttyUSB0)
//!   - SIMCom SIM800 / SIM900 / SIM7600
//!   - Quectel EC25 / EG91
//!
//! The port is opened once, for exclusive use, and split: one task reads
//! what the modem pushes, and each reply writes through the other half.
//! Every text from one number shares a sid, so a nurse that keeps a cell
//! per sigil grows each text on from the last (the same convention as
//! `hives/twilio-sms`).

mod gsm;
mod hum;
mod modem;

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use thrum_core::Chi;
use tokio::io::{AsyncRead, AsyncWrite, WriteHalf};
use tokio_serial::SerialPortBuilderExt;
use tracing::{info, warn};

use crate::hum::Hum;
use crate::modem::{Modem, Text};

pub const HIVE_NAME: &str = "gsm-modem";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq)]
struct Config {
    device: String,
    baud: u32,
    model: Option<String>,
    system: String,
    reply_limit: usize,
}

impl Config {
    fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// An empty setting is an unset one.
    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let var = |name: &str| var(name).filter(|v| !v.trim().is_empty());
        Self {
            device: var("HUM_GSM_DEVICE").unwrap_or_else(|| "/dev/ttyUSB0".into()),
            baud: var("HUM_GSM_BAUD").and_then(|s| s.trim().parse().ok()).unwrap_or(115200),
            model: var("HUM_GSM_MODEL"),
            system: var("HUM_GSM_SYSTEM").unwrap_or_else(|| {
                "You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS.".into()
            }),
            reply_limit: var("HUM_GSM_REPLY_LIMIT").and_then(|s| s.trim().parse().ok()).unwrap_or(1500),
        }
    }
}

fn sid_for(phone: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"gsm-modem:");
    h.update(phone.as_bytes());
    hex::encode(&h.finalize()[..8])
}

fn hello() -> Value {
    json!({
        "version": NESTLING_VERSION,
        "propensity": { "statefulness": "stateful", "richness": "lean", "wire": "gsm/at-cmd-sms" },
        "chis": ["hello", "prompt", "unfurl", "wilt", "error"],
        "source": "https://github.com/adiled/hum/tree/main/hives/gsm-modem",
    })
}

fn prompt(cfg: &Config, sid: &str, text: &Text) -> Value {
    let mut prompt = Map::new();
    prompt.insert("chi".into(), json!(Chi::Prompt));
    prompt.insert("sid".into(), json!(sid));
    prompt.insert("text".into(), json!(text.body));
    if let Some(model) = &cfg.model {
        prompt.insert("modelId".into(), json!(model));
    }
    prompt.insert("systemPrompt".into(), json!(cfg.system));
    prompt.insert("ext".into(), json!({ "gsm-modem": { "from": text.from } }));
    Value::Object(prompt)
}

async fn serve<P>(cfg: Arc<Config>, port: P, hum: Arc<Hum>) -> Result<()>
where
    P: AsyncRead + AsyncWrite + Send + 'static,
{
    let (reader, writer) = tokio::io::split(port);
    let modem = Arc::new(Modem::new(writer));
    modem.init().await.context("initialise the modem")?;
    info!("gsm-modem.ready");
    modem::read(reader, |text| {
        let (cfg, modem, hum) = (cfg.clone(), modem.clone(), hum.clone());
        tokio::spawn(async move {
            if let Err(e) = answer(&cfg, &modem, &hum, text).await {
                warn!(error = ?e, "gsm-modem.answer");
            }
        });
    })
    .await;
    Err(anyhow!("the serial port closed"))
}

async fn answer<P: AsyncWrite>(cfg: &Config, modem: &Modem<WriteHalf<P>>, hum: &Hum, text: Text) -> Result<()> {
    let sid = sid_for(&text.from);
    let grown = hum.grow(&sid, prompt(cfg, &sid, &text)).await;
    let reply = match grown.trim() {
        "" => "(no reply)",
        reply => reply,
    };
    modem.send(&text.from, &gsm::fit(reply, cfg.reply_limit)).await.context("send the reply")
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
    let cfg = Arc::new(Config::from_env());
    info!(device = %cfg.device, baud = cfg.baud, "gsm-modem.open");
    let port = tokio_serial::new(&cfg.device, cfg.baud)
        .open_native_async()
        .with_context(|| format!("open {}", cfg.device))?;
    let hum = Hum::start(hum::link(hello())?);
    serve(cfg, port, hum).await
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use hum_identity::BeeKey;
    use hum_thrum::{Bee, Endpoint, Options};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::sync::Mutex;

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    /// humd with a nurse behind it, on a socket of its own. It admits the
    /// bee and answers a prompt `echo: <text>`, or `slow <ms> <text>`
    /// after ms; `gone <text>` breaks off as nurse-gone, after a petal,
    /// the first two times. Each prompt it heard, and when, is in `heard`.
    struct Humd {
        heard: Arc<std::sync::Mutex<Vec<(std::time::Instant, Value)>>>,
        wilted: Arc<std::sync::Mutex<Vec<(std::time::Instant, String)>>>,
    }

    fn sock() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("gsm-humd-{}-{n}.sock", std::process::id()))
    }

    async fn humd() -> (Humd, Arc<Hum>) {
        let path = sock();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let humd = Humd { heard: Default::default(), wilted: Default::default() };
        let (heard, wilted) = (humd.heard.clone(), humd.wilted.clone());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, w) = stream.into_split();
            let w = Arc::new(Mutex::new(w));
            let say = move |w: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>, tone: Value| async move {
                let mut line = serde_json::to_vec(&tone).unwrap();
                line.push(b'\n');
                w.lock().await.write_all(&line).await.unwrap();
            };
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let tone: Value = serde_json::from_str(&line).unwrap();
                match tone["chi"].as_str() {
                    Some("hello") => {
                        say(w.clone(), json!({"chi": "echo", "rid": tone["rid"], "ok": true})).await;
                        say(w.clone(), json!({"chi": "breath", "sigils": {}})).await;
                    }
                    Some("prompt") => {
                        let nth = {
                            let mut heard = heard.lock().unwrap();
                            heard.push((std::time::Instant::now(), tone.clone()));
                            heard.iter().filter(|(_, p)| p["text"] == tone["text"]).count()
                        };
                        let (w, wilted) = (w.clone(), wilted.clone());
                        tokio::spawn(async move {
                            let sid = tone["sid"].as_str().unwrap().to_string();
                            let mut text = tone["text"].as_str().unwrap().to_string();
                            let words: Vec<&str> = text.split(' ').collect();
                            if words[0] == "gone" && nth <= 2 {
                                say(w.clone(), json!({"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": "half"})).await;
                                say(w, json!({"chi": "error", "sid": sid, "code": "nurse-gone", "message": "the peer growing it left"})).await;
                                return;
                            }
                            if words.len() > 2 && words[0] == "slow" {
                                let ms: u64 = words[1].parse().unwrap();
                                tokio::time::sleep(Duration::from_millis(ms)).await;
                                text = words[2..].join(" ");
                            }
                            for part in ["echo: ", &text] {
                                say(w.clone(), json!({"chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": part})).await;
                            }
                            wilted.lock().unwrap().push((std::time::Instant::now(), sid.clone()));
                            say(w, json!({"chi": "wilt", "sid": sid, "reason": "stop"})).await;
                        });
                    }
                    _ => {}
                }
            }
        });
        let opts = Options::new(HIVE_NAME, "main", thrum_core::Role::Forager)
            .hello(hello())
            .endpoint(Endpoint::Unix(path));
        (humd, Hum::start(Bee::spawn(opts, BeeKey::generate())))
    }

    /// The modem end of the port: it takes the init, pushes texts as
    /// `+CMT`, and hands back each reply the bee sends as (number, body).
    struct Phone<P> {
        port: P,
        got: Vec<u8>,
    }

    impl<P: AsyncRead + AsyncWrite + Unpin> Phone<P> {
        async fn new(port: P) -> Self {
            let mut phone = Phone { port, got: Vec::new() };
            let init: String = modem::INIT.iter().map(|c| format!("{c}\r")).collect();
            while !phone.got.starts_with(init.as_bytes()) {
                phone.more().await;
            }
            phone.got.drain(..init.len());
            phone
        }

        async fn more(&mut self) {
            let mut buf = [0u8; 4096];
            let n = tokio::time::timeout(WAIT, self.port.read(&mut buf)).await.expect("the bee wrote nothing").unwrap();
            assert!(n > 0, "the bee closed the port");
            self.got.extend_from_slice(&buf[..n]);
        }

        async fn text(&mut self, from: &str, body: &[u8]) {
            let header = format!("\r\n+CMT: \"{from}\",\"\",\"25/05/17,12:00:00+00\"\r\n");
            self.port.write_all(&[header.as_bytes(), body, b"\r\n"].concat()).await.unwrap();
        }

        async fn reply(&mut self) -> (String, Vec<u8>) {
            while !self.got.contains(&0x1A) {
                self.more().await;
            }
            let end = self.got.iter().position(|b| *b == 0x1A).unwrap();
            let send: Vec<u8> = self.got.drain(..=end).collect();
            let send = &send[..send.len() - 1];
            let head = b"AT+CMGS=\"";
            assert!(send.starts_with(head), "not an AT+CMGS: {:?}", String::from_utf8_lossy(send));
            let close = send.windows(2).position(|w| w == b"\"\r").unwrap();
            (gsm::decode(&send[head.len()..close]), send[close + 2..].to_vec())
        }
    }

    fn defaults() -> Config {
        Config::from_vars(|_| None)
    }

    async fn bee_on(cfg: Config) -> (Humd, Phone<tokio::io::DuplexStream>) {
        let (humd, hum) = humd().await;
        let (here, there) = tokio::io::duplex(1 << 16);
        tokio::spawn(serve(Arc::new(cfg), here, hum));
        (humd, Phone::new(there).await)
    }

    #[tokio::test]
    async fn a_text_is_prompted_whole_and_answered() {
        let (humd, mut phone) = bee_on(defaults()).await;
        phone.text("+14155551234", b"line one\nline two \x02").await;
        let (to, body) = phone.reply().await;
        assert_eq!(to, "+14155551234");
        assert_eq!(gsm::decode(&body), "echo: line one\nline two $");
        let heard = humd.heard.lock().unwrap();
        let prompt = &heard[0].1;
        assert_eq!(prompt["text"], "line one\nline two $");
        assert_eq!(prompt["sid"], sid_for("+14155551234"));
        assert!(prompt.get("modelId").is_none(), "a prompt names a model only when one is set: {prompt}");
    }

    #[tokio::test]
    async fn a_model_is_named_when_one_is_set() {
        let cfg = Config::from_vars(|name| (name == "HUM_GSM_MODEL").then(|| "claude-haiku-4-5".into()));
        let (humd, mut phone) = bee_on(cfg).await;
        phone.text("+1", b"hi").await;
        phone.reply().await;
        assert_eq!(humd.heard.lock().unwrap()[0].1["modelId"], "claude-haiku-4-5");
    }

    #[tokio::test]
    async fn a_long_reply_is_cut_on_a_character_and_sent_in_gsm() {
        let mut cfg = defaults();
        cfg.reply_limit = 12;
        let (_humd, mut phone) = bee_on(cfg).await;
        phone.text("+1", &gsm::encode("éééééééééé")).await;
        let (_, body) = phone.reply().await;
        assert_eq!(body.len(), 12);
        assert_eq!(gsm::decode(&body), "echo: ééé...");
        assert!(body.iter().all(|b| *b < 0x80));
    }

    #[tokio::test]
    async fn texts_at_once_each_get_their_own_reply() {
        let (humd, mut phone) = bee_on(defaults()).await;
        phone.text("+1", b"slow 400 one").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        phone.text("+1", b"two").await;
        phone.text("+2", b"slow 100 three").await;
        let mut replies = HashMap::<String, Vec<String>>::new();
        for _ in 0..3 {
            let (to, body) = phone.reply().await;
            replies.entry(to).or_default().push(gsm::decode(&body));
        }
        assert_eq!(replies["+1"], ["echo: one", "echo: two"]);
        assert_eq!(replies["+2"], ["echo: three"]);
        let heard = humd.heard.lock().unwrap();
        let wilted = humd.wilted.lock().unwrap();
        let first_wilt = wilted.iter().find(|(_, sid)| *sid == sid_for("+1")).unwrap().0;
        let second = heard.iter().find(|(_, p)| p["text"] == "two").unwrap().0;
        assert!(second >= first_wilt, "the second text of +1 was prompted while the first was open");
        let other = heard.iter().find(|(_, p)| p["text"] == "slow 100 three").unwrap().0;
        assert!(other < first_wilt, "+2 waited behind +1");
    }

    #[tokio::test]
    async fn a_bloom_lost_to_a_gone_nurse_is_grown_again() {
        let (humd, mut phone) = bee_on(defaults()).await;
        phone.text("+1", b"gone again").await;
        let (_, body) = phone.reply().await;
        assert_eq!(gsm::decode(&body), "echo: gone again");
        assert_eq!(humd.heard.lock().unwrap().len(), hum::TRIES);
    }

    #[test]
    fn an_empty_setting_keeps_its_default() {
        let empty = Config::from_vars(|_| Some(" ".into()));
        assert_eq!(empty, defaults());
        assert_eq!(
            defaults(),
            Config {
                device: "/dev/ttyUSB0".into(),
                baud: 115200,
                model: None,
                system: "You are a concise assistant. Keep replies under 1000 characters since they're sent as SMS.".into(),
                reply_limit: 1500,
            }
        );
    }

    #[test]
    fn the_orchfile_hands_every_setting_to_the_bee() {
        let src = include_str!("main.rs");
        let names: Vec<&str> = src.split("var(\"").skip(1).filter_map(|s| s.split('"').next()).filter(|n| n.starts_with("HUM_GSM_")).collect();
        assert_eq!(names.len(), 5, "{names:?}");
        let orch = include_str!("../Orchfile");
        let lines: Vec<&str> = orch.lines().map(str::trim).collect();
        for name in names {
            assert!(lines.contains(&format!("ARG {name}=").as_str()), "no ARG {name}");
            assert!(lines.contains(&format!("ENV {name}=${{{name}}}").as_str()), "no ENV {name}");
        }
    }

    #[tokio::test]
    async fn one_open_port_is_read_and_written() {
        let (master, slave) = tokio_serial::SerialStream::pair().unwrap();
        let (_humd, hum) = humd().await;
        tokio::spawn(serve(Arc::new(defaults()), slave, hum));
        let mut phone = Phone::new(master).await;
        phone.text("+1", b"ping").await;
        let (to, body) = phone.reply().await;
        assert_eq!((to.as_str(), body.as_slice()), ("+1", b"echo: ping".as_slice()));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_device_opens_once() {
        use tokio_serial::SerialPort;
        let (_master, slave) = tokio_serial::SerialStream::pair().unwrap();
        let path = slave.name().unwrap();
        drop(slave);
        let _held = tokio_serial::new(&path, 115200).open_native_async().unwrap();
        assert!(tokio_serial::new(&path, 115200).open_native_async().is_err(), "a second open succeeded");
    }
}
