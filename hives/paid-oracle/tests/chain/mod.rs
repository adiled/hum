//! The world the oracle reads, served on loopback: a chain answering
//! JSON-RPC by what it holds, and a price source answering a set status
//! and body.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

#[derive(Default)]
pub struct Ledger {
    pub txs: HashMap<String, Value>,
    pub receipts: HashMap<String, Value>,
    pub head: u64,
    pub price: (u16, String),
    pub price_hits: usize,
}

#[derive(Clone)]
pub struct Chain {
    pub url: String,
    pub ledger: Arc<Mutex<Ledger>>,
}

impl Chain {
    pub async fn start() -> Chain {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let ledger = Arc::new(Mutex::new(Ledger {
            head: 100,
            price: (200, json!({"ethereum": {"usd": 3456.78}}).to_string()),
            ..Ledger::default()
        }));
        let held = ledger.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let held = held.clone();
                tokio::spawn(async move {
                    let Some((head, body)) = read(&mut sock).await else { return };
                    let (status, body) = if head.starts_with("POST") {
                        (200, rpc(&held, &body))
                    } else {
                        let mut l = held.lock().unwrap();
                        l.price_hits += 1;
                        l.price.clone()
                    };
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                });
            }
        });
        Chain { url, ledger }
    }

    pub fn rpc_url(&self) -> String {
        format!("{}/rpc", self.url)
    }

    pub fn price_url(&self) -> String {
        format!("{}/price", self.url)
    }

    pub fn price(&self, status: u16, body: &str) {
        self.ledger.lock().unwrap().price = (status, body.to_string());
    }

    pub fn price_hits(&self) -> usize {
        self.ledger.lock().unwrap().price_hits
    }

    pub fn head(&self, head: u64) {
        self.ledger.lock().unwrap().head = head;
    }

    /// A transaction, and its receipt when `mined` names its block.
    pub fn hold(&self, hash: &str, tx: Value, receipt: Option<Value>) {
        let mut l = self.ledger.lock().unwrap();
        l.txs.insert(hash.to_string(), tx);
        match receipt {
            Some(r) => l.receipts.insert(hash.to_string(), r),
            None => l.receipts.remove(hash),
        };
    }
}

async fn read(sock: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                .unwrap_or(0usize);
            if body.len() >= len {
                return Some((head.to_string(), body[..len].to_string()));
            }
        }
    }
}

fn rpc(ledger: &Mutex<Ledger>, body: &str) -> String {
    let req: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let hash = req["params"][0].as_str().unwrap_or("").to_string();
    let l = ledger.lock().unwrap();
    let result = match req["method"].as_str().unwrap_or("") {
        "eth_getTransactionByHash" => l.txs.get(&hash).cloned().unwrap_or(Value::Null),
        "eth_getTransactionReceipt" => l.receipts.get(&hash).cloned().unwrap_or(Value::Null),
        "eth_blockNumber" => json!(format!("0x{:x}", l.head)),
        m => return json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": format!("no {m}")}}).to_string(),
    };
    json!({"jsonrpc": "2.0", "id": req["id"], "result": result}).to_string()
}

pub fn word(address: &str) -> String {
    format!("0x{:0>64}", address.trim_start_matches("0x").to_lowercase())
}

pub fn hex_of(n: u128) -> String {
    format!("0x{n:x}")
}

pub fn input_with(nonce: &str) -> String {
    format!("0x{}", hex::encode(nonce.as_bytes()))
}

/// A native transfer of `value` to `to`, its input the nonce's bytes.
pub fn native(to: &str, value: u128, input: &str) -> Value {
    json!({ "to": to, "value": hex_of(value), "input": input })
}

/// A call on `contract`, its input `input`, with no value.
pub fn call(contract: &str, input: &str) -> Value {
    json!({ "to": contract, "value": "0x0", "input": input })
}

/// `transfer(to, amount)` calldata with the nonce's bytes after it.
pub fn transfer_input(to: &str, amount: u128, nonce: &str) -> String {
    format!(
        "0xa9059cbb{}{:064x}{}",
        word(to).trim_start_matches("0x"),
        amount,
        hex::encode(nonce.as_bytes())
    )
}

pub fn transfer_log(token: &str, from: &str, to: &str, amount: u128) -> Value {
    json!({
        "address": token,
        "topics": [TRANSFER, word(from), word(to)],
        "data": format!("0x{amount:064x}"),
    })
}

pub fn receipt(status: &str, block: u64, logs: Vec<Value>) -> Value {
    json!({ "status": status, "blockNumber": format!("0x{block:x}"), "logs": logs })
}
