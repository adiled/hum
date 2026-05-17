//! `paid-oracle` — an x402-style paid oracle nestling.
//!
//! Sells one piece of data per USDC payment. Wire:
//!
//! 1. Counterparty sends `chi:"tool-call"` `{ name: "quote", args: { pair } }`.
//! 2. We reply `chi:"error"` with `code: 402` carrying the payment terms
//!    (price, recipient, chain, nonce) — same shape as HTTP 402 / x402.
//! 3. Counterparty pays on-chain (Base/Arc/whichever) and resubmits the
//!    same tool-call with `paymentProof: { txHash, nonce }`.
//! 4. We verify the tx on the configured RPC, then reply
//!    `chi:"tool-result"` with the actual price.
//!
//! Replay protection: every challenge mints a fresh UUID nonce.
//! Acceptance requires that nonce to appear in the tx input data
//! (so a single payment can't be spent twice on different challenges).
//!
//! No LLM, no on-chain writes. Read-only HTTP to CoinGecko (price)
//! and the configured RPC (verify). Defaults are tuned for Base
//! mainnet; configure via env for Arc / Sepolia / etc.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thrum_core::{Chi, THRUM_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{info, warn};

const NESTLING_NAME: &str = "paid-oracle";
const NESTLING_VERSION: &str = env!("CARGO_PKG_VERSION");
const QUOTE_TOOL: &str = "quote";
/// Quote price in atomic USDC (6 decimals). `50_000` = $0.05.
const QUOTE_PRICE_ATOMIC: u64 = 50_000;

#[derive(Debug, Clone)]
struct Config {
    /// thrum socket: humd's NDJSON UnixStream.
    sock_path: String,
    /// Where on-chain payments must land. EVM address hex, 0x-prefixed.
    pay_to: String,
    /// JSON-RPC endpoint for the chain we accept payment on.
    rpc_url: String,
    /// Human label for the chain (returned in challenges + manifest).
    chain: String,
    /// USDC contract address on the chain (Base mainnet default).
    usdc_contract: String,
    /// Where to fetch underlying price from. Free / no-API-key by default.
    price_url: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
            format!("/run/user/{}", unsafe { libc::geteuid() })
        });
        let default_sock = format!("{runtime}/hum/thrum.sock");
        Ok(Self {
            sock_path: std::env::var("HUM_THRUM_SOCK").unwrap_or(default_sock),
            pay_to: std::env::var("PAID_ORACLE_PAY_TO")
                .context("PAID_ORACLE_PAY_TO (your EVM address) is required")?,
            rpc_url: std::env::var("PAID_ORACLE_RPC")
                .unwrap_or_else(|_| "https://mainnet.base.org".into()),
            chain: std::env::var("PAID_ORACLE_CHAIN")
                .unwrap_or_else(|_| "base-mainnet".into()),
            usdc_contract: std::env::var("PAID_ORACLE_USDC")
                .unwrap_or_else(|_| "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into()),
            price_url: std::env::var("PAID_ORACLE_PRICE_URL").unwrap_or_else(|_| {
                "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd".into()
            }),
        })
    }
}

/// Pending challenge: minted on the first call, consumed (or expired) on retry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Challenge {
    nonce: String,
    rid: String,
    pair: String,
    issued_at_ms: i64,
}

/// In-memory state. A real deploy would persist nonces to disk so an
/// oracle restart doesn't lose replay-protection.
#[derive(Default)]
struct State {
    pending: Mutex<HashMap<String, Challenge>>,
    spent: Mutex<HashSet<String>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")))
        .init();

    let cfg = Config::from_env()?;
    let state = Arc::new(State::default());
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let sock = UnixStream::connect(&cfg.sock_path).await
        .with_context(|| format!("connect {}", cfg.sock_path))?;
    let (rd, wr) = sock.into_split();
    let wr = Arc::new(tokio::sync::Mutex::new(wr));
    let mut lines = BufReader::new(rd).lines();

    send(&wr, &hello(&cfg)).await?;
    info!(nestling = NESTLING_NAME, "thrum.handshake.sent");

    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        let tone: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "thrum.parse");
                continue;
            }
        };
        let chi = tone.get("chi").and_then(Value::as_str).unwrap_or("");
        match chi {
            "breath" => continue,
            "tool-call" => {
                let st = state.clone();
                let cfg = cfg.clone();
                let wr = wr.clone();
                let http = http.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_tool_call(&cfg, &st, &http, &wr, tone).await {
                        warn!(error = ?e, "tool-call.handle");
                    }
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn hello(cfg: &Config) -> Value {
    json!({
        "chi": Chi::Hello,
        "rid": format!("hello-{}", uuid::Uuid::new_v4()),
        "from": NESTLING_NAME,
        "nestling": NESTLING_NAME,
        "version": NESTLING_VERSION,
        "protoVersion": THRUM_VERSION,
        "propensity": {
            "statefulness": "stateless",
            "richness": "lean",
            "wire": "x402/tool-call"
        },
        "chi": ["hello", "tool-call", "tool-result", "error"],
        "source": "https://github.com/adiled/hum/tree/main/nestlings/paid-oracle",
        "x402": {
            "chain": cfg.chain,
            "pay_to": cfg.pay_to,
            "usdc_contract": cfg.usdc_contract,
            "price_atomic_usdc": QUOTE_PRICE_ATOMIC.to_string(),
        }
    })
}

async fn send(wr: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>, tone: &Value) -> Result<()> {
    let mut buf = serde_json::to_string(tone)?;
    buf.push('\n');
    let mut g = wr.lock().await;
    g.write_all(buf.as_bytes()).await?;
    Ok(())
}

async fn handle_tool_call(
    cfg: &Config,
    state: &Arc<State>,
    http: &reqwest::Client,
    wr: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    tone: Value,
) -> Result<()> {
    let name = tone.get("name").and_then(Value::as_str).unwrap_or("");
    if name != QUOTE_TOOL {
        return Ok(()); // not ours
    }
    let rid = tone.get("rid").and_then(Value::as_str).unwrap_or("").to_string();
    let sid = tone.get("sid").and_then(Value::as_str).unwrap_or("").to_string();
    let from = tone.get("from").and_then(Value::as_str).unwrap_or("").to_string();
    let args = tone.get("args").cloned().unwrap_or(json!({}));
    let pair = args.get("pair").and_then(Value::as_str).unwrap_or("ETH/USDC").to_string();

    // Already paid?
    if let Some(proof) = tone.get("paymentProof") {
        let tx = proof.get("txHash").and_then(Value::as_str).unwrap_or("");
        let nonce = proof.get("nonce").and_then(Value::as_str).unwrap_or("");
        if tx.is_empty() || nonce.is_empty() {
            return reply_error(wr, &rid, &sid, &from, 400, "paymentProof requires txHash + nonce").await;
        }
        // Has this nonce been spent already?
        if state.spent.lock().contains(nonce) {
            return reply_error(wr, &rid, &sid, &from, 409, "nonce already spent").await;
        }
        // Was this nonce ever issued?
        let challenge = state.pending.lock().get(nonce).cloned();
        let Some(_challenge) = challenge else {
            return reply_error(wr, &rid, &sid, &from, 410, "unknown or expired nonce").await;
        };
        // Verify on-chain.
        if let Err(e) = verify_payment(http, cfg, tx, nonce).await {
            warn!(tx, nonce, error = ?e, "verify.failed");
            return reply_error(wr, &rid, &sid, &from, 402, &format!("payment unverified: {e}")).await;
        }
        // Mark consumed + serve.
        state.spent.lock().insert(nonce.to_string());
        state.pending.lock().remove(nonce);
        let price = fetch_price(http, &cfg.price_url).await
            .unwrap_or_else(|e| {
                warn!(error = ?e, "price.fetch.failed");
                "0.0".into()
            });
        let result = json!({
            "chi": Chi::ToolResult,
            "rid": rid,
            "sid": sid,
            "to": from,
            "callId": tone.get("callId").cloned().unwrap_or(Value::Null),
            "result": {
                "pair": pair,
                "px": price,
                "source": cfg.price_url,
                "expires_at_ms": now_ms() + 30_000,
            }
        });
        send(wr, &result).await?;
        return Ok(());
    }

    // First call — mint a challenge.
    let nonce = uuid::Uuid::new_v4().to_string();
    let challenge = Challenge {
        nonce: nonce.clone(),
        rid: rid.clone(),
        pair: pair.clone(),
        issued_at_ms: now_ms(),
    };
    state.pending.lock().insert(nonce.clone(), challenge);

    let err = json!({
        "chi": Chi::Error,
        "rid": rid,
        "sid": sid,
        "to": from,
        "callId": tone.get("callId").cloned().unwrap_or(Value::Null),
        "code": 402,
        "message": "payment required",
        "x402": {
            "chain": cfg.chain,
            "pay_to": cfg.pay_to,
            "asset": cfg.usdc_contract,
            "asset_kind": "erc20",
            "price_atomic": QUOTE_PRICE_ATOMIC.to_string(),
            "nonce": nonce,
            "memo": format!("Encode this nonce ({}) in the transfer's input data so we can bind your payment to this quote.", &nonce[..8]),
        }
    });
    send(wr, &err).await?;
    Ok(())
}

async fn reply_error(
    wr: &Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    rid: &str,
    sid: &str,
    to: &str,
    code: u16,
    message: &str,
) -> Result<()> {
    let err = json!({
        "chi": Chi::Error,
        "rid": rid,
        "sid": sid,
        "to": to,
        "code": code,
        "message": message,
    });
    send(wr, &err).await?;
    Ok(())
}

async fn fetch_price(http: &reqwest::Client, url: &str) -> Result<String> {
    let v: Value = http.get(url).send().await?.json().await?;
    // CoinGecko shape: { "ethereum": { "usd": 3850.42 } }
    let n = v.as_object()
        .and_then(|m| m.values().next())
        .and_then(|inner| inner.as_object())
        .and_then(|m| m.values().next())
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow!("unexpected price shape: {v}"))?;
    Ok(format!("{n}"))
}

/// JSON-RPC `eth_getTransactionByHash`. Checks: tx exists, value ≥ expected,
/// recipient matches `pay_to`, and the nonce is present in tx input data.
async fn verify_payment(
    http: &reqwest::Client,
    cfg: &Config,
    tx_hash: &str,
    nonce: &str,
) -> Result<()> {
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getTransactionByHash",
        "params": [tx_hash],
    });
    let resp: Value = http.post(&cfg.rpc_url).json(&req).send().await?.json().await?;
    let tx = resp.get("result").cloned()
        .filter(|v| !v.is_null())
        .ok_or_else(|| anyhow!("tx not found"))?;

    // ERC-20 transfer: `to` is the USDC contract, `input` encodes
    // (selector, recipient, amount). Native transfer: `to` is the
    // recipient directly. We accept either shape; the recipient field
    // is the source of truth.
    let to = tx.get("to").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let input = tx.get("input").and_then(Value::as_str).unwrap_or("");
    let value_hex = tx.get("value").and_then(Value::as_str).unwrap_or("0x0");

    let (paid_recipient, paid_amount): (String, u128) =
        if to.eq_ignore_ascii_case(&cfg.usdc_contract.to_lowercase()) {
            // ERC-20 transfer(address,uint256) — selector 0xa9059cbb (4 bytes)
            // followed by 32-byte recipient (left-padded) + 32-byte amount.
            let stripped = input.strip_prefix("0x").unwrap_or(input);
            if !stripped.to_lowercase().starts_with("a9059cbb") || stripped.len() < 8 + 64 + 64 {
                return Err(anyhow!("not an ERC-20 transfer call"));
            }
            let recipient = format!("0x{}", &stripped[8 + 24..8 + 64]);
            let amount = u128::from_str_radix(&stripped[8 + 64..8 + 64 + 64], 16)
                .map_err(|_| anyhow!("bad ERC-20 amount"))?;
            (recipient, amount)
        } else {
            let stripped = value_hex.strip_prefix("0x").unwrap_or(value_hex);
            let amount = u128::from_str_radix(stripped, 16).unwrap_or(0);
            (to, amount)
        };

    if !paid_recipient.eq_ignore_ascii_case(&cfg.pay_to.to_lowercase()) {
        return Err(anyhow!(
            "wrong recipient: paid={} expected={}",
            paid_recipient, cfg.pay_to
        ));
    }
    if paid_amount < QUOTE_PRICE_ATOMIC as u128 {
        return Err(anyhow!(
            "underpaid: {} < {} atomic USDC",
            paid_amount, QUOTE_PRICE_ATOMIC
        ));
    }
    // Nonce binding: counterparty MUST append the nonce bytes to tx.input.
    // For ERC-20 we tolerate trailing extra data (most clients accept it);
    // for native we look at the input field directly.
    let nonce_hex = hex::encode(nonce.as_bytes());
    if !input.to_lowercase().contains(&nonce_hex.to_lowercase()) {
        return Err(anyhow!("nonce not bound in tx input"));
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
