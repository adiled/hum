//! Persistent bee identity, and the record of who has entered.
//!
//! A bee is its key, and the key is filed under the id it hashes to, so
//! nothing but the key names a bee. A hive brings a bee into being by
//! entering it: minting a key and recording the hive it entered under and
//! the one role it entered for. A hive that keeps track of its bees by
//! something of its own (a job, a requester's fingerprint) enters them
//! under a label: the same hive and label are always the same bee. A bee
//! changes role only by being entered again for another, which files it
//! under another key, so nothing the old key was trusted with carries
//! over. Whoever can read the key is the bee; humd admits only a hello
//! signed by an entered key, so the key file is the trust boundary.
//!
//! File format: raw 32-byte Ed25519 secret seed, mode 0o600, atomic
//! write-and-rename, the same convention as humd's daemon key. An entry
//! is a small JSON file beside it, carrying only public facts.
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
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(seed);
        let hid = Hid::from_pubkey(HidPrefix::Bee, &signing.verifying_key().to_bytes());
        Self { signing, hid }
    }

    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        Self::from_seed(&seed)
    }

    pub fn id(&self) -> String {
        self.hid.to_hex()
    }

    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    pub fn sign_hello(&self, hello: &mut serde_json::Value) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        self.sign_hello_at(hello, now);
    }

    pub fn sign_hello_at(&self, hello: &mut serde_json::Value, now_ms: i64) {
        let sig = self.sign_socket_hello(now_ms);
        if let Some(obj) = hello.as_object_mut() {
            obj.insert("hid".into(), self.id().into());
            obj.insert("from".into(), self.id().into());
            obj.insert("pubkey".into(), sig.pubkey.into());
            obj.insert("signedAt".into(), sig.signed_at.into());
            obj.insert("signature".into(), sig.signature.into());
        }
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

pub fn bee_key_path(id: &str) -> PathBuf {
    hum_paths::bee_key(id)
}

fn plain_id(id: &str) -> Result<()> {
    match Hid::from_hex(id) {
        Ok(hid) if hid.prefix.is_bee() && hum_paths::is_plain_name(id) => Ok(()),
        _ => Err(anyhow!("{id:?} is not a bee id")),
    }
}

pub fn mint_bee_key() -> Result<BeeKey> {
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let key = BeeKey::from_seed(&seed);
    let path = bee_key_path(&key.id());
    persist(&path, &seed)?;
    info!(path = %path.display(), hid = %key.hid.short(), "bee.identity.minted");
    Ok(key)
}

pub fn load_bee_key(id: &str) -> Result<BeeKey> {
    plain_id(id)?;
    let path = bee_key_path(id);
    let bytes = fs::read(&path).with_context(|| format!("read bee key {}", path.display()))?;
    let seed: [u8; 32] = bytes.try_into().map_err(|b: Vec<u8>| {
        anyhow!("bee key at {} is {} bytes, expected 32", path.display(), b.len())
    })?;
    let key = BeeKey::from_seed(&seed);
    if key.id() != id {
        return Err(anyhow!("bee key at {} is not the key of {id}", path.display()));
    }
    trace!(path = %path.display(), hid = %key.hid.short(), "bee.identity.loaded");
    Ok(key)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub hive: String,
    pub pubkey: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exited: bool,
}

fn write_entry(id: &str, entry: &Entry) -> Result<()> {
    let path = hum_paths::bee_entry(id);
    let body = serde_json::to_vec_pretty(entry)?;
    hum_paths::write_atomic(&path, &body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn enter(hive: &str, label: &str, role: &str, key: &BeeKey) -> Result<()> {
    if !hum_paths::is_plain_name(hive) {
        return Err(anyhow!("{hive:?} is not a hive name"));
    }
    if !label.is_empty() && !hum_paths::is_plain_name(label) {
        return Err(anyhow!("{label:?} is not a plain label"));
    }
    if !hum_paths::is_plain_name(role) {
        return Err(anyhow!("{role:?} is not a role"));
    }
    let entry = Entry {
        hive: hive.to_string(),
        pubkey: hex::encode(key.pubkey_bytes()),
        label: label.to_string(),
        role: role.to_string(),
        exited: false,
    };
    write_entry(&key.id(), &entry)?;
    info!(hid = %key.hid.short(), hive, label, role, "bee.entered");
    Ok(())
}

pub fn enter_as(hive: &str, label: &str, role: &str) -> Result<BeeKey> {
    if !hum_paths::hive_installed(hive) {
        return Err(anyhow!("{hive} is not installed on this humd — `hum hive <ref> install` first"));
    }
    if !hum_paths::is_plain_name(label) {
        return Err(anyhow!("{label:?} is not a plain label"));
    }
    if !hum_paths::is_plain_name(role) {
        return Err(anyhow!("{role:?} is not a role"));
    }
    let same: Vec<(String, Entry)> = entered()
        .into_iter()
        .filter(|(_, e)| e.hive == hive && e.label == label)
        .collect();
    let kept = same
        .iter()
        .filter(|(_, e)| e.role == role)
        .min_by_key(|(_, e)| e.exited)
        .or_else(|| same.iter().find(|(_, e)| e.role.is_empty()))
        .map(|(id, _)| id.clone());
    for (id, e) in &same {
        if Some(id) != kept.as_ref() && !e.exited {
            set_exited(id, true)?;
            info!(id, hive, label, from = %e.role, to = role, "bee.exited.for-another-role");
        }
    }
    if let Some(id) = kept {
        let key = load_bee_key(&id)?;
        let mut e = entry(&id).ok_or_else(|| anyhow!("{id} lost its entry"))?;
        if e.exited || e.role != role {
            e.exited = false;
            e.role = role.to_string();
            write_entry(&id, &e)?;
        }
        return Ok(key);
    }
    let key = mint_bee_key()?;
    enter(hive, label, role, &key)?;
    Ok(key)
}

fn set_exited(id: &str, exited: bool) -> Result<bool> {
    plain_id(id)?;
    let Some(mut e) = entry(id) else {
        return Err(anyhow!("{id} never entered on this humd"));
    };
    if e.exited == exited {
        return Ok(false);
    }
    e.exited = exited;
    write_entry(id, &e)?;
    Ok(true)
}

pub fn exit(id: &str) -> Result<bool> {
    set_exited(id, true)
}

pub fn reenter(id: &str) -> Result<bool> {
    load_bee_key(id)?;
    set_exited(id, false)
}

pub fn entry(id: &str) -> Option<Entry> {
    plain_id(id).ok()?;
    let bytes = fs::read(hum_paths::bee_entry(id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn entered() -> Vec<(String, Entry)> {
    let mut out: Vec<(String, Entry)> = fs::read_dir(hum_paths::entered_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".json")?.to_string();
            let entry = entry(&id)?;
            Some((id, entry))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn persist(path: &Path, seed: &[u8; 32]) -> Result<()> {
    hum_paths::write_atomic_private(path, seed)
        .with_context(|| format!("write bee key {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;
    use std::sync::Mutex;
    use tempfile::TempDir;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_minted_bee_is_filed_under_its_id_and_loads_back() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };

        let key = mint_bee_key().expect("mint");
        assert_eq!(key.hid.prefix, HidPrefix::Bee);
        assert!(bee_key_path(&key.id()).ends_with(format!("hum/bees/{}.key", key.id())));
        assert_eq!(load_bee_key(&key.id()).expect("load").hid, key.hid);

        let other = mint_bee_key().expect("mint");
        assert_ne!(key.hid, other.hid, "two bees are two keys");

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn a_key_filed_under_another_id_is_refused() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };

        let key = mint_bee_key().expect("mint");
        let liar = BeeKey::generate().id();
        std::fs::copy(bee_key_path(&key.id()), bee_key_path(&liar)).unwrap();
        assert!(load_bee_key(&liar).is_err());

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn an_id_that_is_a_path_or_no_bee_is_refused() {
        assert!(load_bee_key("../humd").is_err());
        assert!(load_bee_key("").is_err());
        assert!(load_bee_key(&Hid::random_humd().to_hex()).is_err());
        assert!(entry("bee_../x").is_none());
    }

    #[test]
    fn entering_records_the_hive_and_exiting_forgets_it() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", tmp.path()) };

        let key = mint_bee_key().expect("mint");
        assert!(entry(&key.id()).is_none());
        enter("claude-cli", "", "nurse", &key).expect("enter");
        let got = entry(&key.id()).expect("entered");
        assert_eq!(got.hive, "claude-cli");
        assert_eq!(got.role, "nurse");
        assert_eq!(got.pubkey, hex::encode(key.pubkey_bytes()));
        assert!(!got.exited);
        assert_eq!(entered().len(), 1);
        assert!(exit(&key.id()).unwrap());
        assert!(!exit(&key.id()).unwrap());
        let gone = entry(&key.id()).expect("an exited bee is still on record");
        assert!(gone.exited);
        assert_eq!(gone.hive, "claude-cli");
        assert!(reenter(&key.id()).unwrap());
        assert!(!entry(&key.id()).unwrap().exited);
        assert!(exit(&BeeKey::generate().id()).is_err(), "a bee that never entered cannot exit");

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn a_hive_and_label_are_always_the_same_bee() {
        let _guard = ENV_LOCK.lock().unwrap();
        let state = TempDir::new().unwrap();
        let config = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };

        assert!(enter_as("sun", "main", "nurse").is_err(), "an uninstalled hive enters nothing");
        std::fs::create_dir_all(hum_paths::hives_d_dir()).unwrap();
        std::fs::write(hum_paths::hive_installation("sun"), "").unwrap();

        let first = enter_as("sun", "main", "nurse").expect("enter");
        assert_eq!(enter_as("sun", "main", "nurse").unwrap().hid, first.hid);
        assert_ne!(enter_as("sun", "requester-ab", "forager").unwrap().hid, first.hid);
        assert_eq!(entry(&first.id()).unwrap().label, "main");

        exit(&first.id()).unwrap();
        assert_eq!(enter_as("sun", "main", "nurse").unwrap().hid, first.hid, "entering again reenters");
        assert!(!entry(&first.id()).unwrap().exited);
        assert!(enter_as("sun", "../x", "nurse").is_err());
        assert!(enter_as("sun", "main", "../x").is_err());

        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn another_role_is_another_key() {
        let _guard = ENV_LOCK.lock().unwrap();
        let state = TempDir::new().unwrap();
        let config = TempDir::new().unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", state.path()) };
        unsafe { std::env::set_var("XDG_CONFIG_HOME", config.path()) };
        std::fs::create_dir_all(hum_paths::hives_d_dir()).unwrap();
        std::fs::write(hum_paths::hive_installation("sun"), "").unwrap();

        let nurse = enter_as("sun", "main", "nurse").expect("enter");
        let builder = enter_as("sun", "main", "builder").expect("enter again");
        assert_ne!(builder.hid, nurse.hid, "a new role is a new key");
        assert!(entry(&nurse.id()).unwrap().exited, "the old role's key is exited");
        assert_eq!(entry(&builder.id()).unwrap().role, "builder");

        let back = enter_as("sun", "main", "nurse").expect("enter back");
        assert_eq!(back.hid, nurse.hid, "a role comes back to its own key");
        assert!(entry(&builder.id()).unwrap().exited);

        let legacy = mint_bee_key().unwrap();
        enter("sun", "old", "x", &legacy).unwrap();
        let mut e = entry(&legacy.id()).unwrap();
        e.role = String::new();
        write_entry(&legacy.id(), &e).unwrap();
        assert_eq!(enter_as("sun", "old", "forager").unwrap().hid, legacy.hid, "a bee entered before roles takes its first");
        assert_eq!(entry(&legacy.id()).unwrap().role, "forager");

        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    #[test]
    fn a_bee_hid_says_bee_and_nothing_about_its_role() {
        let pubkey = [7u8; 32];
        let bee = Hid::from_pubkey(HidPrefix::Bee, &pubkey);
        assert_eq!(Hid::from_hex(&bee.to_hex()).unwrap(), bee);
        assert!(bee.to_hex().starts_with("bee_"));
        assert!(Hid::from_hex(&format!("wbee_{}", hex::encode(pubkey))).is_err());
        assert_ne!(bee, Hid::from_pubkey(HidPrefix::Humd, &pubkey));
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

        let key = mint_bee_key().expect("mint");
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

        let key = mint_bee_key().expect("mint");
        let other = Hid::random(HidPrefix::Bee);

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

        let key = mint_bee_key().expect("mint");
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
