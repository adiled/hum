//! Revoked bee identities, as consulted at hello time.
//!
//! A bee's hid is the hash of its public key. That gives pseudonymity and
//! no accountability — there is no authority behind a name, and nothing
//! to withdraw when one behaves badly. This is the local half of that:
//! an operator lists a hid and the socket refuses it.
//!
//! Local, not global. It binds this daemon, not the mesh. A bee refused
//! here can still reach a different humd. Closing that gap needs the
//! on-chain registry to carry revocations, which is a contract change
//! rather than a code one — see `ensemble/src/onchain.rs`.
//!
//! What revocation does bind to is the identity binding from the socket
//! handshake. A revoked hid is refused whether or not the bee proved the
//! hid, so dropping its signature to slip past the check does not help.
//!
//! The on-disk format lives in [`hum_paths::Revocations`] so `humctl` can
//! edit the list without depending on the daemon. This wrapper is the
//! daemon-side handle.
//!
//! The set is read once at boot, never per hello. Re-reading would put
//! disk on the admission path, and admission is the one path that must
//! not stall. A revocation therefore takes effect on the next daemon
//! start, which `humctl revoke` says out loud.

use std::path::Path;
use std::sync::Arc;

use parking_lot::RwLock;

#[derive(Clone, Default)]
pub struct Revocations {
    inner: Arc<RwLock<hum_paths::Revocations>>,
}

impl Revocations {
    pub fn load(path: &Path) -> Self {
        Self {
            inner: Arc::new(RwLock::new(hum_paths::Revocations::read_from(path))),
        }
    }

    pub fn is_revoked(&self, hid_hex: &str) -> bool {
        self.inner.read().contains(hid_hex)
    }

    pub fn len(&self) -> usize {
        self.inner.read().revoked.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    pub(crate) fn set_for_test(&self, list: Vec<String>) {
        let mut guard = self.inner.write();
        *guard = hum_paths::Revocations {
            version: 1,
            revoked: list,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_hid_is_revoked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        hum_paths::Revocations {
            version: 1,
            revoked: vec!["bee_dead".into()],
        }
        .write_to(&path)
        .unwrap();
        let rev = Revocations::load(&path);
        assert!(rev.is_revoked("bee_dead"));
        assert!(!rev.is_revoked("bee_other"));
    }

    #[test]
    fn an_absent_file_revokes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(Revocations::load(&tmp.path().join("nope.json")).is_empty());
    }
}
