//! Connection registry — who is currently reachable, and how.

use std::collections::{HashMap, HashSet};

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::mpsc;

pub type ClientId = String;

/// Outbound queue depth. Sized to absorb a turn's worth of chunk tones
/// without back-pressuring the daemon's hot path; if a client truly can't
/// drain this fast, we'd rather drop than stall the broadcast caller.
const OUTBOUND_CAPACITY: usize = 1024;

/// A live bee we can reach. Outbound goes through the channel; the
/// per-connection writer task drains and writes to the socket.
pub struct Reach {
    pub client_id: ClientId,
    tx: mpsc::Sender<Value>,
    sigils: Mutex<HashSet<String>>,
    /// Chi values this client declared in its hello. Empty means it
    /// declared none, which is the same as "unspecified" — such a client
    /// receives everything, because every bee predating this field has
    /// an empty set and refusing it traffic would be a wire break.
    chis: Mutex<HashSet<String>>,
}

impl Reach {
    pub fn new(client_id: ClientId) -> (Self, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel(OUTBOUND_CAPACITY);
        (
            Self {
                client_id,
                tx,
                sigils: Mutex::new(HashSet::new()),
                chis: Mutex::new(HashSet::new()),
            },
            rx,
        )
    }

    /// Non-blocking send. Errors if the queue is full or the receiver is
    /// gone — caller decides whether to drop or log.
    pub fn send(&self, tone: Value) -> Result<(), mpsc::error::TrySendError<Value>> {
        self.tx.try_send(tone)
    }

    pub fn add_sigil(&self, sigil: String) {
        self.sigils.lock().insert(sigil);
    }

    pub fn has_sigil(&self, sigil: &str) -> bool {
        self.sigils.lock().contains(sigil)
    }

    pub fn sigil_count(&self) -> usize {
        self.sigils.lock().len()
    }

    /// Record the chi values a client declared in its hello. Replaces any
    /// previous set so a re-hello that narrows scope actually narrows it.
    pub fn set_chis(&self, chis: Vec<String>) {
        *self.chis.lock() = chis.into_iter().collect();
    }

    /// Whether this client should be handed a tone for `chi`.
    ///
    /// A client that declared nothing receives everything — that is the
    /// backward-compatible answer, and it is the only one available to
    /// every bee written before this field existed. A client that declared
    /// a list receives only those.
    pub fn declares_chi(&self, chi: &str) -> bool {
        let chis = self.chis.lock();
        chis.is_empty() || chis.contains(chi)
    }
}

pub(crate) struct Registry {
    by_id: HashMap<ClientId, std::sync::Arc<Reach>>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self { by_id: HashMap::new() }
    }

    pub(crate) fn insert(&mut self, reach: std::sync::Arc<Reach>) {
        self.by_id.insert(reach.client_id.clone(), reach);
    }

    pub(crate) fn remove(&mut self, client_id: &str) {
        self.by_id.remove(client_id);
    }

    pub(crate) fn get(&self, client_id: &str) -> Option<&Reach> {
        self.by_id.get(client_id).map(|r| r.as_ref())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Reach> {
        self.by_id.values().map(|r| r.as_ref())
    }

    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_that_declared_nothing_receives_everything() {
        let (reach, _rx) = Reach::new("unspecified".into());
        assert!(reach.declares_chi("prompt"), "no declared set = no filtering");
        assert!(reach.declares_chi("tool-call"));
    }

    #[test]
    fn a_client_that_declared_a_list_receives_only_those() {
        let (reach, _rx) = Reach::new("scoped".into());
        reach.set_chis(vec!["prompt".into(), "chunk".into()]);
        assert!(reach.declares_chi("prompt"));
        assert!(reach.declares_chi("chunk"));
        assert!(
            !reach.declares_chi("tool-call"),
            "an undeclared chi must not reach a scoped client"
        );
    }

    #[test]
    fn declaring_an_empty_list_is_not_scoped() {
        // A hello with `chis: []` means "unspecified", not "wants nothing".
        // Every bee that sends the field but knows none is in that boat.
        let (reach, _rx) = Reach::new("empty".into());
        reach.set_chis(Vec::new());
        assert!(reach.declares_chi("prompt"));
    }

    #[test]
    fn re_declaring_replaces_the_previous_scope() {
        // A re-hello that narrows scope must actually narrow, or a bee
        // could widen its subscription by saying it once and never
        // revoking.
        let (reach, _rx) = Reach::new("rehello".into());
        reach.set_chis(vec!["prompt".into(), "chunk".into()]);
        reach.set_chis(vec!["prompt".into()]);
        assert!(reach.declares_chi("prompt"));
        assert!(
            !reach.declares_chi("chunk"),
            "the earlier, wider scope must not survive a re-hello"
        );
    }
}
