//! On-chain HumdRegistry client.
//!
//! Reads any contract implementing
//! [`IHumdRegistry`](../../contracts/src/IHumdRegistry.sol) via plain
//! JSON-RPC `eth_call`. The interface is the standard; the deployed
//! contract is whichever implementation the subnet chose. No alloy /
//! ethers-rs dependency — just `reqwest` + a tiny hand-rolled ABI
//! decoder for the `records(bytes32)` selector.
//!
//! Feature-gated behind the `onchain` cargo feature so default builds
//! don't pull HTTP. Enable with:
//!
//! ```toml
//! ensemble = { ..., features = ["onchain"] }
//! ```
//!
//! Typical use:
//!
//! ```no_run
//! # use ensemble::onchain::HumdRegistryClient;
//! # async fn ex() -> anyhow::Result<()> {
//! let client = HumdRegistryClient::new(
//!     "https://rpc.testnet.arc.network",
//!     "0x1234...",  // registry contract address
//! );
//! let pubkey = [0u8; 32]; // ed25519 pubkey of the humd you're looking up
//! if let Some(record) = client.get(&pubkey).await? {
//!     println!("manifest at {}", record.manifest_uri);
//!     println!("expected hash: 0x{}", hex::encode(record.manifest_hash));
//! }
//! # Ok(()) }
//! ```
//!
//! The ABI layout of `records(bytes32)` is decoded by hand, so the
//! offsets are named below rather than written as bare slice indices.
//! The selector is pinned to the keccak256 of the signature rather
//! than computed, to avoid pulling in a keccak crate.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

const WORD: usize = 32;

const OWNER: usize = 0;
const PUBKEY: usize = 1;
const MANIFEST_HASH: usize = 2;
const MANIFEST_URI: usize = 3;
const UPDATED_AT: usize = 4;

const RECORDS_SELECTOR: [u8; 4] = [0x52, 0xe8, 0x4c, 0x43];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumdRecord {
    pub owner: [u8; 20],
    pub pubkey: [u8; 32],
    pub manifest_hash: [u8; 32],
    pub manifest_uri: String,
    pub updated_at: u64,
}

impl HumdRecord {
    pub fn exists(&self) -> bool {
        self.owner != [0u8; 20]
    }
}

pub struct HumdRegistryClient {
    rpc_url: String,
    contract: [u8; 20],
    http: reqwest::Client,
}

impl HumdRegistryClient {
    pub fn new(rpc_url: impl Into<String>, contract: &str) -> Self {
        let mut bytes = [0u8; 20];
        let stripped = contract.strip_prefix("0x").unwrap_or(contract);
        if let Ok(decoded) = hex::decode(stripped) {
            if decoded.len() == 20 {
                bytes.copy_from_slice(&decoded);
            }
        }
        Self {
            rpc_url: rpc_url.into(),
            contract: bytes,
            http: reqwest::Client::new(),
        }
    }

    pub async fn get(&self, pubkey: &[u8; 32]) -> Result<Option<HumdRecord>> {
        let mut data = Vec::with_capacity(36);
        data.extend_from_slice(&RECORDS_SELECTOR);
        data.extend_from_slice(pubkey);

        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_call",
            "params": [{
                "to": format!("0x{}", hex::encode(self.contract)),
                "data": format!("0x{}", hex::encode(&data)),
            }, "latest"],
        });
        let resp: serde_json::Value = self.http.post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .context("rpc send")?
            .json()
            .await
            .context("rpc parse")?;

        if let Some(err) = resp.get("error") {
            return Err(anyhow!("eth_call error: {err}"));
        }
        let hex_result = resp.get("result")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("no result field"))?;
        let raw = hex::decode(hex_result.strip_prefix("0x").unwrap_or(hex_result))
            .context("decode result hex")?;

        if raw.len() < (UPDATED_AT + 1) * WORD {
            return Err(anyhow!("eth_call result too short: {} bytes", raw.len()));
        }

        let mut owner = [0u8; 20];
        owner.copy_from_slice(&raw[OWNER * WORD + 12..OWNER * WORD + 32]);
        let mut pubkey_out = [0u8; 32];
        pubkey_out.copy_from_slice(&raw[PUBKEY * WORD..(PUBKEY + 1) * WORD]);
        let mut manifest_hash = [0u8; 32];
        manifest_hash.copy_from_slice(&raw[MANIFEST_HASH * WORD..(MANIFEST_HASH + 1) * WORD]);

        let str_offset =
            u256_to_u64(&raw[MANIFEST_URI * WORD..(MANIFEST_URI + 1) * WORD])? as usize;
        let updated_at = u256_to_u64(&raw[UPDATED_AT * WORD..(UPDATED_AT + 1) * WORD])?;

        let manifest_uri = decode_dynamic_string(&raw, str_offset)?;

        let rec = HumdRecord {
            owner,
            pubkey: pubkey_out,
            manifest_hash,
            manifest_uri,
            updated_at,
        };
        if !rec.exists() {
            return Ok(None);
        }
        Ok(Some(rec))
    }
}

fn u256_to_u64(word: &[u8]) -> Result<u64> {
    if word.len() < WORD {
        return Err(anyhow!("u256 word short"));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&word[24..32]);
    Ok(u64::from_be_bytes(buf))
}

fn decode_dynamic_string(raw: &[u8], offset: usize) -> Result<String> {
    if offset + WORD > raw.len() {
        return Err(anyhow!("string head out of range"));
    }
    let len = u256_to_u64(&raw[offset..offset + WORD])? as usize;
    let start = offset + WORD;
    if start + len > raw.len() {
        return Err(anyhow!("string body out of range"));
    }
    Ok(String::from_utf8_lossy(&raw[start..start + len]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humdrecord_exists_zero_owner_is_false() {
        let r = HumdRecord {
            owner: [0u8; 20],
            pubkey: [0u8; 32],
            manifest_hash: [0u8; 32],
            manifest_uri: String::new(),
            updated_at: 0,
        };
        assert!(!r.exists());
    }

    #[test]
    fn humdrecord_exists_nonzero_owner_is_true() {
        let mut r = HumdRecord {
            owner: [0u8; 20],
            pubkey: [0u8; 32],
            manifest_hash: [0u8; 32],
            manifest_uri: String::new(),
            updated_at: 0,
        };
        r.owner[0] = 1;
        assert!(r.exists());
    }

    #[test]
    fn u256_to_u64_reads_trailing_8_bytes() {
        let mut word = [0u8; 32];
        word[24..32].copy_from_slice(&123456789u64.to_be_bytes());
        assert_eq!(u256_to_u64(&word).unwrap(), 123456789);
    }

    #[test]
    fn decode_dynamic_string_round_trip() {
        let s = "hello";
        let mut raw = vec![0u8; 64];
        raw[24..32].copy_from_slice(&(s.len() as u64).to_be_bytes());
        raw[32..32 + s.len()].copy_from_slice(s.as_bytes());
        let got = decode_dynamic_string(&raw, 0).unwrap();
        assert_eq!(got, "hello");
    }
}
