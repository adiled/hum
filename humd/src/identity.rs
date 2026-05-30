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
use std::io::Write;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::SigningKey;
use ensemble::HumdKey;
use rand::RngCore;
use tracing::{info, trace};

pub fn key_path() -> PathBuf {
    hum_paths::humd_key()
}

/// Load the persisted key, minting + persisting a fresh one on first boot.
///
/// Bytes on disk are the raw 32-byte Ed25519 secret seed.
pub fn load_or_mint_key() -> Result<HumdKey> {
    let path = key_path();
    if path.exists() {
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
        let signing = SigningKey::from_bytes(&arr);
        let key = HumdKey(signing);
        trace!(path = %path.display(), humd_id = %key.hid().short(), "identity.loaded");
        return Ok(key);
    }

    // Mint fresh seed and persist.
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    let key = HumdKey(signing);
    persist_key(&path, &seed)?;
    info!(path = %path.display(), humd_id = %key.hid().short(), "identity.minted");
    Ok(key)
}

/// Write 32 bytes atomically (tmp + rename) with mode 0o600.
fn persist_key(path: &std::path::Path, seed: &[u8; 32]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("mkdir -p {}", parent.display()))?;
    }
    let tmp = match path.file_name() {
        Some(name) => {
            let mut tmp_name = name.to_os_string();
            tmp_name.push(".tmp");
            path.with_file_name(tmp_name)
        }
        None => return Err(anyhow!("humd key path has no file name: {}", path.display())),
    };

    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(seed)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all().ok();
    }

    // Belt-and-braces: also chmod after open in case the umask masked our
    // requested mode bits. No-op on non-unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }

    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Mint a key, drop it, reload from the same path — same Hid.
    /// Also checks file perms are 0o600.
    #[test]
    fn round_trip_through_tempdir() {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("XDG_STATE_HOME", tmp.path());

        let first = load_or_mint_key().expect("mint");
        let id1 = first.hid();
        let path = key_path();
        assert!(path.exists(), "key file persisted");

        // Permissions check.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "key file must be 0o600");
        }

        // Reload — same identity.
        let second = load_or_mint_key().expect("reload");
        assert_eq!(id1, second.hid(), "humd_id stable across reloads");

        std::env::remove_var("XDG_STATE_HOME");
    }
}
