//! Persistent bee identity.
//!
//! Mirrors humd's `identity.rs` but tags the resulting [`Hid`] with the
//! bee role (worker / forager). Each bee install gets its own Ed25519
//! keypair at `$XDG_STATE_HOME/hum/bees/<kind>.key`. The pubkey hashes
//! to a stable role-prefixed [`Hid`] (`wbee_<hex>` / `fbee_<hex>`) that
//! survives reconnect, restart, even daemon swap. The bee carries this
//! hid in every `chi:"hello"` so humd's manifest registry is keyed by
//! identity, not by transient thrum connection id.
//!
//! File format: raw 32-byte Ed25519 secret seed, mode 0o600, atomic
//! write-and-rename. Same convention as humd's daemon key — the only
//! difference is the dir + role tagging.
//!
//! This is a *leaf* crate for remote hives: it depends only on
//! `hum-paths` (for the seed path) and its own bundled `ids` module
//! for [`Hid`]. No daemon, mesh, or nest machinery.
//!
//! The socket hello is signed over the hid and the timestamp only —
//! nothing else on the hello. The timestamp binds freshness, so a
//! recorded hello cannot be replayed after the skew window. It is
//! domain-separated from the ensemble peer handshake: a bee talking to
//! its own humd over a unix socket and two humds over iroh are
//! different wires with different signing keys, and sharing a domain
//! would let one be mistaken for the other.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use ed25519_dalek::{Signer, SigningKey};
mod hid;
mod humid;
pub use hid::*;
pub use humid::*;
use rand::RngCore;
use tracing::{info, trace};

#[derive(Debug)]
pub struct BeeKey {
    pub signing: SigningKey,
    pub hid: Hid,
}

impl BeeKey {
    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    pub fn sign_socket_hello(&self, signed_at_ms: i64) -> SocketSignature {
        let msg = socket_hello_message(&self.hid, signed_at_ms);
        let sig = self.signing.sign(&msg);
        SocketSignature {
            pubkey: hex::encode(self.pubkey_bytes()),
            signed_at: signed_at_ms,
            signature: hex::encode(sig.to_bytes()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SocketSignature {
    pub pubkey: String,
    pub signed_at: i64,
    pub signature: String,
}

pub const BEE_SOCKET_DOMAIN: &str = "hum-bee-socket-v1";

pub fn socket_hello_message(hid: &Hid, signed_at_ms: i64) -> Vec<u8> {
    format!("{}:{}:{}", BEE_SOCKET_DOMAIN, hid.to_hex(), signed_at_ms).into_bytes()
}

pub fn bee_key_path(kind: &str) -> PathBuf {
    hum_paths::bee_key(kind)
}

pub fn load_or_mint_bee_key(kind: &str, prefix: HidPrefix) -> Result<BeeKey> {
    let path = bee_key_path(kind);
    if path.exists() {
        let bytes = fs::read(&path).with_context(|| format!("read bee key {}", path.display()))?;
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
        fs::create_dir_all(parent).with_context(|| format!("mkdir -p {}", parent.display()))?;
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
    use ed25519_dalek::Verifier;
    use std::sync::Mutex;
    use tempfile::TempDir;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn round_trip_worker_key_then_different_kind() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };

        let first = load_or_mint_bee_key("claude-cli", HidPrefix::Wbee).expect("mint");
        assert_eq!(first.hid.prefix, HidPrefix::Wbee);
        let id1 = first.hid;

        let second = load_or_mint_bee_key("claude-cli", HidPrefix::Wbee).expect("reload");
        assert_eq!(id1, second.hid, "wbee hid stable across reloads");

        let other = load_or_mint_bee_key("humfs", HidPrefix::Fbee).expect("mint");
        assert_ne!(id1, other.hid);
        assert_eq!(other.hid.prefix, HidPrefix::Fbee);

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn key_path_uses_xdg_state_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };
        let path = bee_key_path("foo");
        assert!(path.starts_with(tmp.path()), "path {:?}", path);
        assert!(path.ends_with("hum/bees/foo.key"));
        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn a_signature_binds_the_hid_it_was_signed_for() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };

        let key = load_or_mint_bee_key("sig-bind", HidPrefix::Wbee).expect("mint");
        let sig = key.sign_socket_hello(1_700_000_000_000);
        let (pubkey_hex, signed_at, sig_hex) = (sig.pubkey, sig.signed_at, sig.signature);
        assert_eq!(pubkey_hex, hex::encode(key.pubkey_bytes()));

        let msg = socket_hello_message(&key.hid, signed_at);
        let sig_bytes = hex::decode(&sig_hex).expect("sig hex");
        assert_eq!(sig_bytes.len(), 64, "ed25519 signature is 64 bytes");

        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let verifying = key.signing.verifying_key();
        verifying
            .verify(&msg, &ed25519_dalek::Signature::from_bytes(&sig_arr))
            .expect("signature must verify against the signing key");

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn a_signature_does_not_verify_for_a_different_hid() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };

        let key = load_or_mint_bee_key("sig-rebind", HidPrefix::Wbee).expect("mint");
        let other = Hid::random(HidPrefix::Wbee);

        let sig = key.sign_socket_hello(1_700_000_000_000);
        let (signed_at, sig_hex) = (sig.signed_at, sig.signature);
        let msg = socket_hello_message(&other, signed_at);
        let sig_bytes = hex::decode(&sig_hex).expect("sig hex");
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let verifying = key.signing.verifying_key();
        assert!(
            verifying
                .verify(&msg, &ed25519_dalek::Signature::from_bytes(&sig_arr))
                .is_err(),
            "signature bound to one hid must not verify for another"
        );

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn the_timestamp_is_part_of_what_is_signed() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };

        let key = load_or_mint_bee_key("sig-replay", HidPrefix::Wbee).expect("mint");
        let sig = key.sign_socket_hello(1_700_000_000_000);
        let (signed_at, sig_hex) = (sig.signed_at, sig.signature);
        let sig_bytes = hex::decode(&sig_hex).expect("sig hex");
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes);
        let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);
        let verifying = key.signing.verifying_key();
        for other in [1_700_000_000_001, 1_600_000_000_000, 0] {
            let msg = socket_hello_message(&key.hid, other);
            assert!(
                verifying.verify(&msg, &signature).is_err(),
                "a signature at {signed_at} must not verify at {other}"
            );
        }

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }
}
