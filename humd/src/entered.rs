//! The bees entered on this humd.
//!
//! A hive enters a bee with `hum bee enter`, which leaves a record of the
//! bee's key and hive on disk; exiting marks it, reentering clears it.
//! humd admits a hello only when it is signed by an entered key and names
//! the hive that key entered under. The record is read at every hello and
//! never cached, so an exit is honoured from the very next hello; a bee
//! already nestled when it exits stays until it reconnects.

use std::collections::HashMap;
use std::sync::Arc;

use ensemble::Hid;
use hum_identity::Entry;
use parking_lot::RwLock;

#[derive(Clone)]
pub struct EnteredBees {
    memory: Option<Arc<RwLock<HashMap<String, Entry>>>>,
}

impl EnteredBees {
    pub fn on_disk() -> Self {
        Self { memory: None }
    }

    pub fn in_memory() -> Self {
        Self {
            memory: Some(Arc::default()),
        }
    }

    pub fn enter(&self, hive: &str, key: &hum_identity::BeeKey) {
        if let Some(m) = &self.memory {
            m.write().insert(
                key.id(),
                Entry {
                    hive: hive.to_string(),
                    pubkey: hex::encode(key.pubkey_bytes()),
                    exited: false,
                },
            );
        }
    }

    pub fn exit(&self, id: &str) {
        if let Some(m) = &self.memory
            && let Some(e) = m.write().get_mut(id)
        {
            e.exited = true;
        }
    }

    pub fn entry(&self, hid: &Hid) -> Option<Entry> {
        let id = hid.to_hex();
        match &self.memory {
            Some(m) => m.read().get(&id).cloned(),
            None => hum_identity::entry(&id),
        }
    }

    pub fn admits(&self, hid: &Hid, hive: &str) -> bool {
        self.entry(hid).is_some_and(|e| {
            !e.exited
                && e.hive == hive
                && hex::decode(&e.pubkey)
                    .is_ok_and(|pk| Hid::from_pubkey(hum_identity::HidPrefix::Bee, &pk) == *hid)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bee_is_admitted_only_under_the_hive_it_entered() {
        let bees = EnteredBees::in_memory();
        let key = hum_identity::BeeKey::generate();
        assert!(!bees.admits(&key.hid, "humfs"));
        bees.enter("humfs", &key);
        assert!(bees.admits(&key.hid, "humfs"));
        assert!(!bees.admits(&key.hid, "claude-cli"));
        bees.exit(&key.id());
        assert!(!bees.admits(&key.hid, "humfs"));
    }

    #[test]
    fn an_entry_whose_key_is_not_the_bees_admits_nothing() {
        let bees = EnteredBees::in_memory();
        let key = hum_identity::BeeKey::generate();
        let other = hum_identity::BeeKey::generate();
        bees.memory.as_ref().unwrap().write().insert(
            key.id(),
            Entry {
                hive: "humfs".into(),
                pubkey: hex::encode(other.pubkey_bytes()),
                exited: false,
            },
        );
        assert!(!bees.admits(&key.hid, "humfs"));
    }
}
