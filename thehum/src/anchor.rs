//! On-chain anchoring of state roots.
//!
//! `AnchorBackend` is implemented per network (Ethereum, Base, Arc).
//! `thehum::anchor()` posts the most recent state root + signed
//! attestation. Provides discoverability, timestamping, dispute
//! settlement, and reputation when a network chooses to adopt the
//! contract entry per humd.

use anyhow::Result;
use async_trait::async_trait;

use crate::{StateRoot, TheHum};

/// One on-chain anchor receipt. Returned by backends.
#[derive(Debug, Clone)]
pub struct AnchorReceipt {
    pub network: String,
    pub tx_hash: String,
    pub block_number: Option<u64>,
}

/// Per-network backend. `submit` posts the (hid, root, height) tuple
/// signed by the humd's signing key. Backends translate to whatever the
/// chain expects (EVM call, transaction, etc).
#[async_trait]
pub trait AnchorBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn submit(
        &self,
        hid: &str,
        root: &StateRoot,
        height: u64,
        sig_hex: &str,
    ) -> Result<AnchorReceipt>;
}

impl TheHum {
    /// Anchor the most recent snapshot to the configured network. The
    /// caller passes the same `root` that `snapshot()` returned plus
    /// the height it was taken at. Signature uses thehum's signing key.
    pub async fn anchor(
        &self,
        backend: &dyn AnchorBackend,
        root: &StateRoot,
        height: u64,
    ) -> Result<AnchorReceipt> {
        let payload = [
            self.author_hid.as_bytes(),
            b":",
            &height.to_be_bytes(),
            b":",
            root,
        ].concat();
        let sig_hex = crate::sign::sign_canonical(&self.signing_key, &payload);
        let receipt = backend.submit(&self.author_hid, root, height, &sig_hex).await?;
        let body = serde_json::json!({
            "network": receipt.network,
            "tx_hash": receipt.tx_hash,
            "block_number": receipt.block_number,
            "height": height,
            "root": hex::encode(root),
        });
        self.append("anchor", None, crate::HumId::mint(), body).await?;
        Ok(receipt)
    }
}

/// Stub backend used by tests; submits to memory.
pub struct InMemoryAnchor {
    pub name: String,
    pub log: parking_lot::Mutex<Vec<(String, StateRoot, u64, String)>>,
}

impl InMemoryAnchor {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), log: Default::default() }
    }
}

#[async_trait]
impl AnchorBackend for InMemoryAnchor {
    fn name(&self) -> &str { &self.name }

    async fn submit(
        &self,
        hid: &str,
        root: &StateRoot,
        height: u64,
        sig_hex: &str,
    ) -> Result<AnchorReceipt> {
        self.log.lock().push((hid.to_string(), *root, height, sig_hex.to_string()));
        let n = self.log.lock().len();
        Ok(AnchorReceipt {
            network: self.name.clone(),
            tx_hash: format!("mem-{n}"),
            block_number: Some(n as u64),
        })
    }
}

/// Ethereum/EVM backend scaffold. Wires up to a JSON-RPC endpoint via
/// reqwest and submits a typed call to a HumdRegistry contract:
///
/// ```solidity
/// function anchor(bytes32 hid, bytes32 root, uint256 height, bytes calldata sig) external;
/// ```
///
/// v0 is a scaffold that prepares the calldata; real submission needs
/// a funded signer (the chain-side address) and is intentionally
/// deferred — production users wire their own wallet (Foundry script,
/// metamask-snap, etc) and call `prepare_calldata`.
pub struct EvmAnchor {
    pub network: String,
    pub registry_address: String,
    pub rpc_url: String,
}

impl EvmAnchor {
    pub fn prepare_calldata(&self, hid: &str, root: &StateRoot, height: u64, sig_hex: &str) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + 32 * 5);
        bytes.extend_from_slice(&[0x4e, 0x71, 0xd9, 0x2d]); // placeholder selector
        let mut hid_bytes = [0u8; 32];
        let raw = hex::decode(hid.trim_start_matches("humd_")).unwrap_or_default();
        let n = raw.len().min(32);
        hid_bytes[..n].copy_from_slice(&raw[..n]);
        bytes.extend_from_slice(&hid_bytes);
        bytes.extend_from_slice(root);
        let mut height_bytes = [0u8; 32];
        height_bytes[24..].copy_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&height_bytes);
        let mut offset = [0u8; 32];
        offset[31] = 0xa0;
        bytes.extend_from_slice(&offset);
        let sig_bytes = hex::decode(sig_hex).unwrap_or_default();
        let mut sig_len = [0u8; 32];
        sig_len[24..].copy_from_slice(&(sig_bytes.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&sig_len);
        let padded_len = (sig_bytes.len() + 31) & !31;
        let mut padded = vec![0u8; padded_len];
        padded[..sig_bytes.len()].copy_from_slice(&sig_bytes);
        bytes.extend_from_slice(&padded);
        bytes
    }
}

#[async_trait]
impl AnchorBackend for EvmAnchor {
    fn name(&self) -> &str { &self.network }

    async fn submit(
        &self,
        _hid: &str,
        _root: &StateRoot,
        _height: u64,
        _sig_hex: &str,
    ) -> Result<AnchorReceipt> {
        anyhow::bail!("EvmAnchor: v0 ships calldata-prep only; production users wire their own signer. Call prepare_calldata() and submit via your wallet.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    #[tokio::test]
    async fn in_memory_anchor_records_and_appends() {
        let tmp = TempDir::new().unwrap();
        let key = SigningKey::generate(&mut OsRng);
        let t = TheHum::open(tmp.path(), key, crate::Config::default()).unwrap();
        let backend = InMemoryAnchor::new("test");
        let root: StateRoot = [9u8; 32];
        let receipt = t.anchor(&backend, &root, 42).await.unwrap();
        assert_eq!(receipt.network, "test");
        assert_eq!(backend.log.lock().len(), 1);

        let events = t.range(t.author_hid(), 0).unwrap();
        assert_eq!(events.last().unwrap().chi, "anchor");
    }

    #[test]
    fn evm_calldata_is_padded_correctly() {
        let evm = EvmAnchor {
            network: "evm-test".into(),
            registry_address: "0x0".into(),
            rpc_url: "http://localhost:8545".into(),
        };
        let data = evm.prepare_calldata("humd_aabbcc", &[1u8; 32], 100, "deadbeef");
        assert!(data.len() >= 196, "selector(4) + 5×32-byte words + padded dynamic data");
        assert_eq!(data.len() % 32, 4, "post-selector tail aligns to 32-byte words");
    }
}
