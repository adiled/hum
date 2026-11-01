//! Persistent humd identity.
//!
//! A humd's [`Hid`] is derived from an Ed25519 public key; the
//! corresponding signing key has to survive restarts or the humd's
//! identity churns every boot. This module pins the key to a single file
//! under `$XDG_STATE_HOME/hum/humd.key` (32 raw bytes, mode 0o600).
//!
//! - First boot mints a fresh keypair and persists it.
//! - Subsequent boots load the bytes back and reconstruct `HumdKey`.
//! - Anyone but the daemon user reading the file is a security failure,
//!   hence the strict perm + atomic write-and-rename.
//!
//! Real key management (rotation, cert chains, hardware-backed storage)
//! lives at T2+. This is just the floor: stable identity across boots.
//!
//! The file format is raw 32-byte little-endian Ed25519 secret seed,
//! exactly what [`ed25519_dalek::SigningKey::from_bytes`] expects.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::SigningKey;
use ensemble::HumdKey;
use rand::RngCore;
use tracing::{info, trace};

pub fn key_path() -> PathBuf {
    hum_paths::humd_key()
}

pub fn load_or_mint_key() -> Result<HumdKey> {
    load_or_mint_key_at(&key_path())
}

pub fn load_or_mint_key_at(path: &std::path::Path) -> Result<HumdKey> {
    if path.exists() {
        let bytes = fs::read(path)
            .with_context(|| format!("read humd key {}", path.display()))?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "humd key at {} is {} bytes, expected 32",
                path.display(),
                bytes.len()
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let signing = SigningKey::from_bytes(&arr);
        let key = HumdKey(signing);
        trace!(path = %path.display(), humd_id = %key.hid().short(), "identity.loaded");
        return Ok(key);
    }

    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    let key = HumdKey(signing);
    persist_key(path, &seed)?;
    info!(path = %path.display(), humd_id = %key.hid().short(), "identity.minted");
    Ok(key)
}

pub fn read_key() -> Result<Option<HumdKey>> {
    let path = key_path();
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .with_context(|| format!("read humd key {}", path.display()))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "humd key at {} is {} bytes, expected 32",
            path.display(),
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(Some(HumdKey(SigningKey::from_bytes(&arr))))
}

fn persist_key(path: &std::path::Path, seed: &[u8; 32]) -> Result<()> {
    hum_paths::write_atomic_private(path, seed)
        .with_context(|| format!("write humd key {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn round_trip_through_tempdir() {
        let tmp = TempDir::new().unwrap();
        let path: &Path = &tmp.path().join("humd.key");

        let first = load_or_mint_key_at(path).expect("mint");
        let id1 = first.hid();
        assert!(path.exists(), "key file persisted");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "key file must be 0o600");
        }

        let second = load_or_mint_key_at(path).expect("reload");
        assert_eq!(id1, second.hid(), "humd_id stable across reloads");
    }
}
