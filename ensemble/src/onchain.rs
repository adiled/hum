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

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// One record from the HumdRegistry contract — matches `Record` in
/// `contracts/src/HumdRegistry.sol`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumdRecord {
    /// EVM address that controls this entry. `[0;20]` means "no record".
    pub owner: [u8; 20],
    /// ed25519 pubkey. The off-chain Hid is `sha256(pubkey)`; here
    /// we store the pubkey itself so the contract can derive it.
    pub pubkey: [u8; 32],
    /// keccak256 of the full manifest JSON bytes. Readers MUST verify
    /// `keccak256(fetched_bytes) == manifest_hash` before trusting.
    pub manifest_hash: [u8; 32],
    /// Pointer to the manifest — `ipfs://<cid>` or `https://...`.
    pub manifest_uri: String,
    /// Block timestamp of the latest update.
    pub updated_at: u64,
}

impl HumdRecord {
    /// Convenience: `[0;20]` owner means "no record" in Solidity's
    /// default-zero storage model.
    pub fn exists(&self) -> bool {
        self.owner != [0u8; 20]
    }
}

/// JSON-RPC client for the HumdRegistry contract.
pub struct HumdRegistryClient {
    rpc_url: String,
    /// 20-byte EVM contract address.
    contract: [u8; 20],
    http: reqwest::Client,
}

impl HumdRegistryClient {
    /// Build a client.
    ///
    /// - `rpc_url`: HTTPS JSON-RPC endpoint of the chain (`https://rpc.testnet.arc.network`).
    /// - `contract`: hex 0x-prefixed address of the deployed registry.
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

    /// Look up `records(pubkey)`. Returns `None` if owner is zero
    /// (no record).
    pub async fn get(&self, pubkey: &[u8; 32]) -> Result<Option<HumdRecord>> {
        // Selector: keccak256("records(bytes32)")[..4] = 0x52e84c43
        // Computed by `cast sig "records(bytes32)"` — pinned here to
        // avoid pulling a keccak crate. If the selector ever changes
        // (it won't unless the function signature changes), regenerate.
        const SELECTOR: [u8; 4] = [0x52, 0xe8, 0x4c, 0x43];
        let mut data = Vec::with_capacity(36);
        data.extend_from_slice(&SELECTOR);
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

        // The auto-generated public getter for `records(bytes32)`
        // returns 5 fields in declaration order, ABI-encoded:
        //   (address, bytes32, bytes32, string, uint64)
        // Layout:
        //   word 0:  address (20-byte right-padded in 32 bytes)
        //   word 1:  pubkey (bytes32)
        //   word 2:  manifestHash (bytes32)
        //   word 3:  offset to string head (relative to start of tuple)
        //   word 4:  updatedAt (uint64 right-padded)
        //   then at offset: string length + body padded to 32-byte boundary.
        if raw.len() < 5 * 32 {
            return Err(anyhow!("eth_call result too short: {} bytes", raw.len()));
        }

        let mut owner = [0u8; 20];
        owner.copy_from_slice(&raw[12..32]);
        let mut pubkey_out = [0u8; 32];
        pubkey_out.copy_from_slice(&raw[32..64]);
        let mut manifest_hash = [0u8; 32];
        manifest_hash.copy_from_slice(&raw[64..96]);

        let str_offset = u256_to_u64(&raw[96..128])? as usize;
        // updated_at is a uint64 packed in word 4 (the LAST 8 bytes of
        // the right-padded 32-byte slot).
        let updated_at = u256_to_u64(&raw[128..160])?;

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

/// Read the trailing 8 bytes of a 32-byte big-endian word as u64.
/// (Solidity returns uint64 right-padded into a 32-byte slot.)
fn u256_to_u64(word: &[u8]) -> Result<u64> {
    if word.len() < 32 {
        return Err(anyhow!("u256 word short"));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&word[24..32]);
    Ok(u64::from_be_bytes(buf))
}

/// ABI dynamic string: at `offset`, a 32-byte length followed by the
/// UTF-8 bytes (padded to a 32-byte boundary).
fn decode_dynamic_string(raw: &[u8], offset: usize) -> Result<String> {
    if offset + 32 > raw.len() {
        return Err(anyhow!("string head out of range"));
    }
    let len = u256_to_u64(&raw[offset..offset + 32])? as usize;
    let start = offset + 32;
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
        // ABI dynamic string at offset 0:
        //   word 0:  length (5)
        //   word 1:  "hello" + 27 bytes of zero padding
        let s = "hello";
        let mut raw = vec![0u8; 64];
        raw[24..32].copy_from_slice(&(s.len() as u64).to_be_bytes());
        raw[32..32 + s.len()].copy_from_slice(s.as_bytes());
        let got = decode_dynamic_string(&raw, 0).unwrap();
        assert_eq!(got, "hello");
    }
}
