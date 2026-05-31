//! Persistent bee identity. Mirrors humd's `identity.rs` but tags
//! the resulting [`Hid`] with the bee role (worker / forager).
//!
//! Each bee install gets its own Ed25519 keypair at
//! `$XDG_STATE_HOME/hum/bees/<kind>.key`. The pubkey hashes to a
//! stable role-prefixed [`Hid`] (`wbee_<hex>` / `fbee_<hex>`) that
//! survives reconnect, restart, even daemon swap. The bee carries
//! this hid in every `chi:"hello"` so humd's manifest registry is
//! keyed by identity, not by transient thrum connection id.
//!
//! File format: raw 32-byte Ed25519 secret seed, mode 0o600, atomic
//! write-and-rename. Same convention as humd's daemon key — the only
//! difference is the dir + role tagging.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::SigningKey;
use ensemble::{Hid, HidPrefix};
use rand::RngCore;
use tracing::{info, trace};

/// One bee's persistent identity: signing key + derived [`Hid`].
/// Cheap to clone the bytes; the signing key is on the stack inside
/// the wrapper.
#[derive(Debug)]
pub struct BeeKey {
    pub signing: SigningKey,
    pub hid: Hid,
}

impl BeeKey {
    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }
}

pub fn bee_key_path(kind: &str) -> PathBuf {
    hum_paths::bee_key(kind)
}

/// Load the bee's persisted key, minting + persisting a fresh one
/// on first boot. The returned [`Hid`] is derived from the pubkey
/// with the given role `prefix`.
pub fn load_or_mint_bee_key(kind: &str, prefix: HidPrefix) -> Result<BeeKey> {
    let path = bee_key_path(kind);
    if path.exists() {
        let bytes = fs::read(&path)
            .with_context(|| format!("read bee key {}", path.display()))?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "bee key at {} is {} bytes, expected 32",
                path.display(),
                bytes.len()
            ));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let signing = SigningKey::from_bytes(&arr);
        let hid = Hid::from_pubkey(prefix, &signing.verifying_key().to_bytes());
        trace!(path = %path.display(), %kind, hid = %hid.short(), "bee.identity.loaded");
        return Ok(BeeKey { signing, hid });
    }

    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    persist(&path, &seed)?;
    let hid = Hid::from_pubkey(prefix, &signing.verifying_key().to_bytes());
    info!(path = %path.display(), %kind, hid = %hid.short(), "bee.identity.minted");
    Ok(BeeKey { signing, hid })
}

fn persist(path: &Path, seed: &[u8; 32]) -> Result<()> {
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
        None => return Err(anyhow!("bee key path has no file name: {}", path.display())),
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
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serialize tests because they share the XDG_STATE_HOME env var.
    // Without this lock, parallel cargo-test threads race the env and
    // the second test sees the first's tempdir (or an empty value).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn round_trip_worker_key_then_different_kind() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_var("XDG_STATE_HOME", tmp.path());

        let first = load_or_mint_bee_key("claude-cli", HidPrefix::Wbee).expect("mint");
        assert_eq!(first.hid.prefix, HidPrefix::Wbee);
        let id1 = first.hid;

        let second = load_or_mint_bee_key("claude-cli", HidPrefix::Wbee).expect("reload");
        assert_eq!(id1, second.hid, "wbee hid stable across reloads");

        let other = load_or_mint_bee_key("humfs", HidPrefix::Fbee).expect("mint");
        assert_ne!(id1, other.hid);
        assert_eq!(other.hid.prefix, HidPrefix::Fbee);

        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn key_path_uses_xdg_state_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_var("XDG_STATE_HOME", tmp.path());
        let path = bee_key_path("foo");
        assert!(path.starts_with(tmp.path()), "path {:?}", path);
        assert!(path.ends_with("hum/bees/foo.key"));
        std::env::remove_var("XDG_STATE_HOME");
    }
}
