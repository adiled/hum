//! `paid-oracle`: an attendant that sells one price per USDC payment,
//! x402-style.
//!
//! A reach of `quote` without `paymentProof` is answered with the terms:
//! what to pay, to whom, and a nonce. The forager pays on chain with the
//! nonce's bytes in the transaction's input, and reaches again with
//! `paymentProof: { txHash, nonce }`. The oracle reads the transaction,
//! its receipt and the chain's head from its RPC, and answers with the
//! price when they pay the terms.
//!
//! Every answer is a `chi:"tendril-result"` under the reach's `callId`,
//! a refusal with `isError`, since a nurse hears no `error`.
//!
//! A nonce carries a MAC under a key kept in the hive's state directory,
//! so the oracle holds no challenge and a nonce outlives a restart. A
//! transaction redeems one call: its hash is filed under `redeemed/`
//! before the price is answered, and nothing is filed when the price
//! cannot be had.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use hmac::{Hmac, Mac};
use hum_thrum::{Bee, Event, Options, Refused};
use serde_json::{json, Value};
use sha2::Sha256;
use thrum_core::Chi;
use tracing::{info, warn};

const HIVE_NAME: &str = "paid-oracle";
const BEE_VERSION: &str = env!("CARGO_PKG_VERSION");
const QUOTE_TOOL: &str = "quote";
const QUOTE_PRICE_CENTS: u128 = 5;
const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayKind {
    Native,
    Erc20,
}

impl PayKind {
    fn as_str(self) -> &'static str {
        match self {
            PayKind::Native => "native",
            PayKind::Erc20 => "erc20",
        }
    }
}

#[derive(Debug, Clone)]
struct Config {
    pay_to: String,
    rpc_url: String,
    chain: String,
    usdc_contract: String,
    decimals: u32,
    pay_kind: PayKind,
    price_url: String,
    price_atomic: u128,
    confirmations: u64,
}

impl Config {
    fn from_env() -> Result<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let var = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let parsed = |k: &str, default| -> Result<u64> {
            var(k).map_or(Ok(default), |v| v.parse().map_err(|e| anyhow!("{k}={v}: {e}")))
        };
        let pay_kind = match var("PAID_ORACLE_PAY_KIND").map(|v| v.to_ascii_lowercase()).as_deref() {
            None | Some("native") => PayKind::Native,
            Some("erc20" | "erc-20") => PayKind::Erc20,
            Some(other) => bail!("PAID_ORACLE_PAY_KIND must be native or erc20, not {other}"),
        };
        let decimals = parsed("PAID_ORACLE_DECIMALS", if pay_kind == PayKind::Native { 18 } else { 6 })?;
        let price_atomic = u32::try_from(decimals)
            .ok()
            .and_then(|d| 10u128.checked_pow(d))
            .and_then(|scale| scale.checked_mul(QUOTE_PRICE_CENTS))
            .map(|cents| cents / 100)
            .filter(|p| *p > 0)
            .ok_or_else(|| anyhow!("PAID_ORACLE_DECIMALS={decimals} cannot price a quote of {QUOTE_PRICE_CENTS} cents"))?;
        let confirmations = parsed("PAID_ORACLE_CONFIRMATIONS", 1)?;
        if confirmations == 0 {
            bail!("PAID_ORACLE_CONFIRMATIONS must be at least 1");
        }
        let pay_to = var("PAID_ORACLE_PAY_TO").context("PAID_ORACLE_PAY_TO, the EVM address USDC must reach, is required")?;
        let usdc_contract = var("PAID_ORACLE_USDC").unwrap_or_else(|| match pay_kind {
            PayKind::Native => "0x0000000000000000000000000000000000000000".into(),
            PayKind::Erc20 => "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(),
        });
        Ok(Self {
            pay_to: address("PAID_ORACLE_PAY_TO", pay_to)?,
            rpc_url: var("PAID_ORACLE_RPC").unwrap_or_else(|| "https://rpc.testnet.arc.network".into()),
            chain: var("PAID_ORACLE_CHAIN").unwrap_or_else(|| "arc-testnet".into()),
            usdc_contract: address("PAID_ORACLE_USDC", usdc_contract)?,
            decimals: decimals as u32,
            pay_kind,
            price_url: var("PAID_ORACLE_PRICE_URL").unwrap_or_else(|| {
                "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd".into()
            }),
            price_atomic,
            confirmations,
        })
    }
}

fn address(name: &str, v: String) -> Result<String> {
    let hex = v.strip_prefix("0x").unwrap_or("");
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{name}={v} is not an EVM address, 0x and 40 hex digits");
    }
    Ok(v)
}

/// What the oracle keeps: the key its nonces carry a MAC under, and a
/// file per redeemed transaction.
struct State {
    key: [u8; 32],
    redeemed: PathBuf,
}

impl State {
    fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("nonce.key");
        let key = match std::fs::read(&path) {
            Ok(bytes) => bytes.try_into().map_err(|_| anyhow!("{} is not a 32-byte key", path.display()))?,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let mut key = [0u8; 32];
                getrandom::getrandom(&mut key).map_err(|e| anyhow!("no randomness for the nonce key: {e}"))?;
                hum_paths::write_atomic_private(&path, &key).with_context(|| format!("write {}", path.display()))?;
                key
            }
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        let redeemed = dir.join("redeemed");
        std::fs::create_dir_all(&redeemed).with_context(|| format!("create {}", redeemed.display()))?;
        Ok(Self { key, redeemed })
    }

    fn mac(&self, seed: &[u8]) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("hmac takes any key");
        mac.update(seed);
        mac
    }

    fn mint(&self) -> Result<String> {
        let mut seed = [0u8; 16];
        getrandom::getrandom(&mut seed).map_err(|e| anyhow!("no randomness for a nonce: {e}"))?;
        let tag = self.mac(&seed).finalize().into_bytes();
        Ok(format!("{}{}", hex::encode(seed), hex::encode(&tag[..16])))
    }

    fn issued(&self, nonce: &str) -> bool {
        match hex::decode(nonce) {
            Ok(bytes) if bytes.len() == 32 => self.mac(&bytes[..16]).verify_truncated_left(&bytes[16..]).is_ok(),
            _ => false,
        }
    }

    fn is_redeemed(&self, tx: &str) -> bool {
        self.redeemed.join(tx).exists()
    }

    /// Files `tx` as redeemed; false when it already was.
    fn redeem(&self, tx: &str, nonce: &str) -> std::io::Result<bool> {
        let path = self.redeemed.join(tx);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => return Err(e),
        };
        let filed = writeln!(file, "{nonce}")
            .and_then(|()| file.sync_all())
            .and_then(|()| File::open(&self.redeemed)?.sync_all());
        if filed.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        filed.map(|()| true)
    }
}

/// What a reach is answered with: words for a cell, JSON for a forager.
#[derive(Debug)]
struct Answer {
    output: String,
    result: Value,
    is_error: bool,
}

fn refused(code: u16, why: impl Into<String>) -> Answer {
    let why = why.into();
    Answer { result: json!({ "code": code.to_string(), "message": why }), output: why, is_error: true }
}

struct Oracle {
    cfg: Config,
    state: State,
    http: reqwest::Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .init();

    let oracle = Arc::new(Oracle {
        cfg: Config::from_env()?,
        state: State::open(&hum_paths::hive_state_dir(HIVE_NAME))?,
        http: reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?,
    });

    let mut bee = Bee::link(Options::new(HIVE_NAME, "main", thrum_core::Role::Attendant).hello(hello(&oracle.cfg)))
        .await
        .context("link the paid-oracle bee")?;
    info!(hive = HIVE_NAME, bee = %bee.id(), "thrum.nestled");
    let wr = bee.sender();

    while let Some(event) = bee.next().await {
        let tone = match event {
            Event::Tone(tone) => tone,
            Event::Refused(why) => return Err(Refused(why).into()),
            Event::Nestled | Event::Astray | Event::Advisory(_) => continue,
        };
        if tone.get("chi").and_then(Value::as_str) == Some("tendril-reach") {
            let (oracle, wr) = (oracle.clone(), wr.clone());
            tokio::spawn(async move {
                let answer = oracle.answer(&tone).await;
                wr.send(reply(&tone, answer));
            });
        }
    }
    Ok(())
}

fn hello(cfg: &Config) -> Value {
    json!({
        "version": BEE_VERSION,
        "tools": [
            {
                "name": QUOTE_TOOL,
                "description": format!(
                    "The price {} answers, sold per call for {} atomic USDC on {}. A reach without paymentProof is answered with the terms; pay them on chain with the nonce in the transaction's input, then reach again with paymentProof.",
                    cfg.price_url, cfg.price_atomic, cfg.chain
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "paymentProof": {
                            "type": "object",
                            "description": "The paying transaction and the nonce its terms gave. Without it, the reach is answered with the terms.",
                            "properties": {
                                "txHash": { "type": "string" },
                                "nonce":  { "type": "string" }
                            },
                            "required": ["txHash", "nonce"]
                        }
                    }
                }
            }
        ],
        "propensity": {
            "statefulness": "stateless",
            "richness": "lean",
            "wire": "x402/tendril"
        },
        "chis": ["hello", "tendril-reach", "tendril-result"],
        "source": "https://github.com/adiled/hum/tree/main/hives/paid-oracle",
        "ext": { "paid-oracle": { "x402": {
            "chain": cfg.chain,
            "pay_to": cfg.pay_to,
            "usdc_contract": cfg.usdc_contract,
            "decimals": cfg.decimals,
            "pay_kind": cfg.pay_kind.as_str(),
            "price_atomic_usdc": cfg.price_atomic.to_string(),
            "confirmations": cfg.confirmations,
        } } }
    })
}

fn reply(tone: &Value, answer: Answer) -> Value {
    let mut reply = json!({
        "chi": Chi::TendrilResult,
        "callId": tone.get("callId").cloned().unwrap_or(Value::Null),
        "output": answer.output,
        "result": answer.result,
        "isError": answer.is_error,
    });
    if let Some(sid) = tone.get("sid").filter(|s| s.is_string()) {
        reply["sid"] = sid.clone();
    }
    reply
}

fn terms(cfg: &Config, nonce: &str) -> Answer {
    let how = match cfg.pay_kind {
        PayKind::Native => "as a transaction's value".to_string(),
        PayKind::Erc20 => format!("as a transfer of the token {}", cfg.usdc_contract),
    };
    Answer {
        output: format!(
            "payment required: {} atomic USDC to {} on {}, {how}, with the nonce {nonce} as bytes in the transaction's input; once it has {} confirmations, reach {QUOTE_TOOL} again with paymentProof {{ txHash, nonce }}",
            cfg.price_atomic, cfg.pay_to, cfg.chain, cfg.confirmations
        ),
        result: json!({
            "code": "402",
            "message": "payment required",
            "x402": {
                "chain": cfg.chain,
                "pay_to": cfg.pay_to,
                "asset": cfg.usdc_contract,
                "asset_kind": cfg.pay_kind.as_str(),
                "decimals": cfg.decimals,
                "price_atomic": cfg.price_atomic.to_string(),
                "confirmations": cfg.confirmations,
                "nonce": nonce,
            }
        }),
        is_error: true,
    }
}

impl Oracle {
    async fn answer(&self, tone: &Value) -> Answer {
        let name = tone.get("name").and_then(Value::as_str).unwrap_or("");
        if name != QUOTE_TOOL {
            return refused(404, format!("paid-oracle serves {QUOTE_TOOL}, not {name}"));
        }
        match tone.pointer("/args/paymentProof").filter(|p| !p.is_null()) {
            None => match self.state.mint() {
                Ok(nonce) => terms(&self.cfg, &nonce),
                Err(e) => refused(503, format!("no nonce could be minted: {e:#}")),
            },
            Some(proof) => self.redeem(proof).await.unwrap_or_else(|refusal| refusal),
        }
    }

    async fn redeem(&self, proof: &Value) -> Result<Answer, Answer> {
        let field = |k: &str| proof.get(k).and_then(Value::as_str).unwrap_or("");
        let (tx, nonce) = (field("txHash").to_ascii_lowercase(), field("nonce"));
        let hash = tx.strip_prefix("0x").unwrap_or("");
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || nonce.is_empty() {
            return Err(refused(400, "paymentProof takes txHash, 0x and 64 hex digits, and the nonce the terms gave"));
        }
        if !self.state.issued(nonce) {
            return Err(refused(410, "this oracle never issued that nonce"));
        }
        let spent = || refused(409, "that transaction has already redeemed its call");
        if self.state.is_redeemed(&tx) {
            return Err(spent());
        }
        self.verify(&tx, nonce).await?;
        let not_spent = "the payment is not spent; reach again with the same proof";
        let px = price(&self.http, &self.cfg.price_url).await.map_err(|e| {
            warn!(error = %format!("{e:#}"), "price.unavailable");
            refused(503, format!("the price is unavailable ({e:#}); {not_spent}"))
        })?;
        match self.state.redeem(&tx, nonce) {
            Ok(true) => {}
            Ok(false) => return Err(spent()),
            Err(e) => return Err(refused(503, format!("the redemption could not be filed ({e}); {not_spent}"))),
        }
        info!(tx, px, "quote.sold");
        let result = json!({ "px": px, "source": self.cfg.price_url, "expires_at_ms": now_ms() + 30_000 });
        Ok(Answer { output: result.to_string(), result, is_error: false })
    }

    async fn verify(&self, tx_hash: &str, nonce: &str) -> Result<(), Answer> {
        let cfg = &self.cfg;
        let unread = |e: anyhow::Error| refused(503, format!("the RPC could not be read ({e:#}); reach again with the same proof"));
        let early = |why: String| refused(425, format!("{why}; reach again with the same proof once it is"));
        let unpaid = |why: String| refused(402, format!("the transaction does not pay the terms: {why}"));
        let call = |method: &'static str, params: Value| rpc(&self.http, &cfg.rpc_url, method, params);

        let tx = call("eth_getTransactionByHash", json!([tx_hash])).await.map_err(unread)?;
        let receipt = call("eth_getTransactionReceipt", json!([tx_hash])).await.map_err(unread)?;
        let Some(mined) = receipt.get("blockNumber").and_then(uint).filter(|_| tx.is_object()) else {
            return Err(early("it is not mined yet".into()));
        };
        if receipt.get("status").and_then(uint) != Some(1) {
            return Err(unpaid("it reverted".into()));
        }
        let head = call("eth_blockNumber", json!([])).await.map_err(unread)?;
        let head = uint(&head).ok_or_else(|| unread(anyhow!("eth_blockNumber answered {head}")))?;
        let deep = head.saturating_add(1).saturating_sub(mined);
        if deep < u128::from(cfg.confirmations) {
            return Err(early(format!("it has {deep} of {} confirmations", cfg.confirmations)));
        }
        let input = tx.get("input").and_then(Value::as_str).and_then(|i| hex::decode(i.trim_start_matches("0x")).ok());
        if !input.is_some_and(|i| i.windows(nonce.len()).any(|w| w == nonce.as_bytes())) {
            return Err(unpaid("its input does not carry the nonce".into()));
        }
        let paid = match cfg.pay_kind {
            PayKind::Native => paid_as_value(cfg, &tx),
            PayKind::Erc20 => paid_in_logs(cfg, &receipt),
        };
        if paid < cfg.price_atomic {
            return Err(unpaid(format!("it pays {paid} of {} atomic USDC to {}", cfg.price_atomic, cfg.pay_to)));
        }
        Ok(())
    }
}

/// A native payment: the value of a transaction sent to `pay_to`.
fn paid_as_value(cfg: &Config, tx: &Value) -> u128 {
    let to = tx.get("to").and_then(Value::as_str).unwrap_or("");
    if !to.eq_ignore_ascii_case(&cfg.pay_to) {
        return 0;
    }
    tx.get("value").and_then(uint).unwrap_or(0)
}

/// An ERC-20 payment: every `Transfer` the asset logged to `pay_to` from
/// anyone else, summed.
fn paid_in_logs(cfg: &Config, receipt: &Value) -> u128 {
    let to = word(&cfg.pay_to);
    let logs = receipt.get("logs").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    logs.iter()
        .filter(|log| log.get("address").and_then(Value::as_str).is_some_and(|a| a.eq_ignore_ascii_case(&cfg.usdc_contract)))
        .filter(|log| {
            let topics: Vec<String> = log.get("topics").and_then(Value::as_array).into_iter().flatten()
                .filter_map(|t| t.as_str().map(str::to_ascii_lowercase))
                .collect();
            topics.len() == 3 && topics[0] == TRANSFER && topics[2] == to && topics[1] != to
        })
        .filter_map(|log| log.get("data").and_then(uint))
        .fold(0, u128::saturating_add)
}

/// An address as a 32-byte topic.
fn word(address: &str) -> String {
    format!("0x{:0>64}", address.trim_start_matches("0x").to_ascii_lowercase())
}

/// A quantity in hex, saturating at `u128::MAX`.
fn uint(v: &Value) -> Option<u128> {
    let digits = v.as_str()?.strip_prefix("0x")?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.len() > 32 {
        return Some(u128::MAX);
    }
    Some(if digits.is_empty() { 0 } else { u128::from_str_radix(digits, 16).ok()? })
}

async fn rpc(http: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let resp: Value = http.post(url).json(&req).send().await?.error_for_status()?.json().await?;
    if let Some(e) = resp.get("error").filter(|e| !e.is_null()) {
        bail!("{method}: {e}");
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

/// The price at `url`, which answers as CoinGecko's simple price does:
/// `{ "<id>": { "<currency>": <number> } }`, one id holding one positive
/// number.
async fn price(http: &reqwest::Client, url: &str) -> Result<String> {
    let v: Value = http.get(url).send().await?.error_for_status()?.json().await?;
    let only = |v: &Value| v.as_object().filter(|m| m.len() == 1).and_then(|m| m.values().next().cloned());
    only(&v)
        .as_ref()
        .and_then(only)
        .and_then(|n| n.as_f64())
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n.to_string())
        .ok_or_else(|| anyhow!("it answered {v}, not one id holding one positive number"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "../tests/chain/mod.rs"]
mod chain;

#[cfg(test)]
mod tests;
