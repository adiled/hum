//! Revoked bee identities.
//!
//! A bee's hid is the hash of its public key. That gives pseudonymity and
//! no accountability — there is no authority behind a name, and nothing
//! to withdraw when one behaves badly. This is the local half of that:
//! an operator lists a hid here and the socket refuses it.
//!
//! Local, not global. It binds the *operator's* humd, not the mesh. A bee
//! refused here can still reach a different daemon. Closing that gap
//! needs the on-chain registry to carry revocations, which is a contract
//! change rather than a code one — see `ensemble/src/onchain.rs`.
//!
//! What revocation does bind to is the identity binding from the socket
//! handshake. A revoked hid is refused whether or not the bee proved the
//! hid, so dropping its signature to slip past the check does not help.

use std::io;
use std::path::Path;

use parking_lot::RwLock;
use tracing::{trace, warn};

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct OnDisk {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    revoked: Vec<String>,
}

/// The set of withdrawn hids, loaded once at boot and consulted per hello.
///
/// Cloning is cheap — the set lives behind an `Arc` and is not re-read per
/// hello, so a revocation takes effect on the next daemon start rather
/// than immediately. That is a deliberate trade: re-reading a file on
/// every hello puts disk on the admission path, and admission is the one
/// place that must not stall.
#[derive(Clone, Default)]
pub struct Revocations {
    inner: std::sync::Arc<RwLock<Vec<String>>>,
}

impl Revocations {
    /// Load from disk. An absent or unparseable file yields an empty set:
    /// a corrupt revocation file must not lock every bee out of the
    /// socket, which is a worse failure than admitting a revoked one.
    pub fn load(path: &Path) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                trace!(target: "revoke", "revoke.load.absent");
                Vec::new()
            }
            Err(e) => {
                warn!(target: "revoke", "revoke.load.read_failed err={e}");
                Vec::new()
            }
        };
        let list = if bytes.is_empty() {
            Vec::new()
        } else {
            match serde_json::from_slice::<OnDisk>(&bytes) {
                Ok(disk) => disk.revoked,
                Err(e) => {
                    warn!(target: "revoke", "revoke.load.parse_failed err={e} — treating as empty");
                    Vec::new()
                }
            }
        };
        Self {
            inner: std::sync::Arc::new(RwLock::new(list)),
        }
    }

    /// Whether `hid_hex` has been withdrawn.
    pub fn is_revoked(&self, hid_hex: &str) -> bool {
        self.inner.read().iter().any(|h| h == hid_hex)
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Replace the set and persist it. For tests and CLI paths that need
    /// to change the list without restarting the daemon.
    pub fn set_and_save(&self, list: Vec<String>, path: &Path) -> io::Result<()> {
        {
            let mut guard = self.inner.write();
            *guard = list.clone();
        }
        save(path, &list)
    }
}

pub fn save(path: &Path, revoked: &[String]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let disk = OnDisk {
        version: 1,
        revoked: revoked.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&disk)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn an_absent_file_revokes_nothing() {
        let tmp = TempDir::new().unwrap();
        let rev = Revocations::load(&tmp.path().join("nope.json"));
        assert!(rev.is_empty());
        assert!(!rev.is_revoked("wbee_abc"));
    }

    #[test]
    fn a_listed_hid_is_revoked() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        let rev = Revocations::load(&path);
        rev.set_and_save(vec!["wbee_dead".into()], &path).unwrap();
        assert!(rev.is_revoked("wbee_dead"));
        assert!(!rev.is_revoked("wbee_other"));
    }

    #[test]
    fn a_revoked_list_survives_a_reload() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        Revocations::load(&path)
            .set_and_save(vec!["wbee_dead".into()], &path)
            .unwrap();
        let reopened = Revocations::load(&path);
        assert!(reopened.is_revoked("wbee_dead"));
    }

    #[test]
    fn a_corrupt_file_revokes_nothing_rather_than_everything() {
        // Failing closed here would lock every bee out of the socket. An
        // operator can see the parse warning; they cannot debug a daemon
        // that refuses to admit anything.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let rev = Revocations::load(&path);
        assert!(rev.is_empty());
    }
}