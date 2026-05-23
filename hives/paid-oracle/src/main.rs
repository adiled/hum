//! `paid-oracle` — an x402-style paid oracle bee.
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

const HIVE_NAME: &str = "paid-oracle";
const BEE_VERSION: &str = env!("CARGO_PKG_VERSION");
const QUOTE_TOOL: &str = "quote";
const CAPABILITY: &str = "x402:quote";
/// Quote price in atomic USDC (6 decimals). `50_000` = $0.05.
/// Display price in cents (0.01 USD units). Atomic amount is computed
/// at runtime from the configured chain's USDC decimals so the same
/// $0.05 quote works on chains where USDC has 6 decimals (Base, Eth
/// mainnet) and on Arc where USDC is the native gas token with 18.
const QUOTE_PRICE_CENTS: u64 = 5;

/// How payment lands on-chain.
///
/// - `Native`: USDC is the chain's native gas token (Arc). Payment is
///   a plain transfer where `tx.value` carries the atomic amount and
///   `tx.to` is the recipient. The nonce must appear in `tx.input`.
/// - `Erc20`: USDC is an ERC-20 contract (Base, Eth mainnet). The
///   transfer hits the contract; we decode the `transfer(addr,uint)`
///   selector + args to find recipient + amount. Nonce must appear in
///   `tx.input` (after the standard selector + args).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayKind {
    Native,
    Erc20,
}

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
    /// USDC contract address on the chain (or zero address for native).
    usdc_contract: String,
    /// Decimals USDC uses on this chain. Base/Eth: 6. Arc: 18.
    decimals: u32,
    /// Whether payment is a native transfer (Arc) or an ERC-20 call.
    pay_kind: PayKind,
    /// Where to fetch underlying price from. Free / no-API-key by default.
    price_url: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
            format!("/run/user/{}", unsafe { libc::geteuid() })
        });
        let default_sock = format!("{runtime}/hum/thrum.sock");
        let pay_kind = match std::env::var("PAID_ORACLE_PAY_KIND")
            .unwrap_or_else(|_| "native".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "native" => PayKind::Native,
            "erc20" | "erc-20" => PayKind::Erc20,
            other => anyhow::bail!("PAID_ORACLE_PAY_KIND must be 'native' or 'erc20' (got '{}')", other),
        };
        let decimals: u32 = std::env::var("PAID_ORACLE_DECIMALS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(match pay_kind {
                // Arc-native USDC is 18 decimals; Base/Eth USDC ERC-20 is 6.
                PayKind::Native => 18,
                PayKind::Erc20 => 6,
            });
        Ok(Self {
            sock_path: std::env::var("HUM_THRUM_SOCK").unwrap_or(default_sock),
            pay_to: std::env::var("PAID_ORACLE_PAY_TO")
                .context("PAID_ORACLE_PAY_TO (your EVM address) is required")?,
            rpc_url: std::env::var("PAID_ORACLE_RPC")
                .unwrap_or_else(|_| "https://rpc.testnet.arc.network".into()),
            chain: std::env::var("PAID_ORACLE_CHAIN")
                .unwrap_or_else(|_| "arc-testnet".into()),
            usdc_contract: std::env::var("PAID_ORACLE_USDC")
                .unwrap_or_else(|_| match pay_kind {
                    PayKind::Native => "0x0000000000000000000000000000000000000000".into(),
                    PayKind::Erc20 => "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into(),
                }),
            decimals,
            pay_kind,
            price_url: std::env::var("PAID_ORACLE_PRICE_URL").unwrap_or_else(|_| {
                "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd".into()
            }),
        })
    }

    /// Atomic USDC amount for the quote at this chain's decimals.
    /// $0.05 × 10^decimals / 100.
    fn quote_atomic(&self) -> u128 {
        let scale = 10u128.checked_pow(self.decimals).unwrap_or(u128::MAX);
        (QUOTE_PRICE_CENTS as u128).saturating_mul(scale) / 100
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
    info!(hive = HIVE_NAME, "thrum.handshake.sent");

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
        "from": HIVE_NAME,
        "bee": ["forager"],
        "hive": HIVE_NAME,
        "version": BEE_VERSION,
        "protoVersion": THRUM_VERSION,
        "provides": [CAPABILITY],
        "tools": [
            {
                "name": QUOTE_TOOL,
                "description": "Get a real-time market quote for a token pair (e.g. ETH-USD). Paid per call via on-chain USDC; first call returns chi:error 402 with terms, asker pays + resubmits with paymentProof.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pair": { "type": "string", "description": "Pair symbol like ETH-USD." },
                        "paymentProof": {
                            "type": "object",
                            "description": "Optional. Set on resubmission after paying.",
                            "properties": {
                                "txHash": { "type": "string" },
                                "nonce":  { "type": "string" }
                            }
                        }
                    },
                    "required": ["pair"]
                }
            }
        ],
        "propensity": {
            "statefulness": "stateless",
            "richness": "lean",
            "wire": "x402/tool-call"
        },
        "chis": ["hello", "tool-call", "tool-result", "error"],
        "source": "https://github.com/adiled/hum/tree/main/hives/paid-oracle",
        "x402": {
            "chain": cfg.chain,
            "pay_to": cfg.pay_to,
            "usdc_contract": cfg.usdc_contract,
            "decimals": cfg.decimals,
            "pay_kind": match cfg.pay_kind {
                PayKind::Native => "native",
                PayKind::Erc20 => "erc20",
            },
            "price_atomic_usdc": cfg.quote_atomic().to_string(),
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
            "asset_kind": match cfg.pay_kind {
                PayKind::Native => "native",
                PayKind::Erc20 => "erc20",
            },
            "decimals": cfg.decimals,
            "price_atomic": cfg.quote_atomic().to_string(),
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

    // Two shapes:
    //   - Native (Arc): `to` is the recipient directly; `tx.value`
    //     carries the atomic amount. USDC is the native gas token.
    //   - ERC-20 (Base, Eth mainnet): `to` is the USDC contract; the
    //     `input` field encodes `transfer(address,uint256)` —
    //     selector 0xa9059cbb + 32-byte recipient + 32-byte amount.
    let to = tx.get("to").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let input = tx.get("input").and_then(Value::as_str).unwrap_or("");
    let value_hex = tx.get("value").and_then(Value::as_str).unwrap_or("0x0");

    let (paid_recipient, paid_amount): (String, u128) = match cfg.pay_kind {
        PayKind::Native => {
            let stripped = value_hex.strip_prefix("0x").unwrap_or(value_hex);
            let amount = u128::from_str_radix(stripped, 16).unwrap_or(0);
            (to, amount)
        }
        PayKind::Erc20 => {
            if !to.eq_ignore_ascii_case(&cfg.usdc_contract.to_lowercase()) {
                return Err(anyhow!(
                    "tx.to {} doesn't match expected USDC contract {}",
                    to, cfg.usdc_contract
                ));
            }
            let stripped = input.strip_prefix("0x").unwrap_or(input);
            if !stripped.to_lowercase().starts_with("a9059cbb") || stripped.len() < 8 + 64 + 64 {
                return Err(anyhow!("not an ERC-20 transfer call"));
            }
            let recipient = format!("0x{}", &stripped[8 + 24..8 + 64]);
            let amount = u128::from_str_radix(&stripped[8 + 64..8 + 64 + 64], 16)
                .map_err(|_| anyhow!("bad ERC-20 amount"))?;
            (recipient, amount)
        }
    };

    if !paid_recipient.eq_ignore_ascii_case(&cfg.pay_to.to_lowercase()) {
        return Err(anyhow!(
            "wrong recipient: paid={} expected={}",
            paid_recipient, cfg.pay_to
        ));
    }
    let expected = cfg.quote_atomic();
    if paid_amount < expected {
        return Err(anyhow!(
            "underpaid: {} < {} atomic USDC",
            paid_amount, expected
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
