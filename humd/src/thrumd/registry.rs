//! Connection registry — who is currently reachable, and how.
//!
//! Each [`Reach`] carries the chi scope its client declared in its hello.
//! An empty scope means unspecified, not "wants nothing": a client that
//! declared nothing accepts every chi. That is the only answer available
//! to every bee written before the field existed, and narrowing it would
//! be a wire break.
//!
//! Closing a [`Reach`] ends its connection: the reader stops, what was
//! already queued for the client (a refusal, say) is written, and the
//! socket closes. Taking a client out of the registry alone would leave
//! its socket open and its tones still read.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::{Notify, mpsc};

pub type ClientId = String;

const OUTBOUND_CAPACITY: usize = 1024;

pub struct Reach {
    pub client_id: ClientId,
    tx: mpsc::Sender<Value>,
    audit: Arc<super::audit::Audit>,
    sigils: Mutex<HashSet<String>>,
    chis: Mutex<HashSet<String>>,
    closing: Notify,
}

impl Reach {
    pub fn new(client_id: ClientId) -> (Self, mpsc::Receiver<Value>) {
        Self::audited(client_id, Arc::default())
    }

    pub fn audited(
        client_id: ClientId,
        audit: Arc<super::audit::Audit>,
    ) -> (Self, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel(OUTBOUND_CAPACITY);
        (
            Self {
                client_id,
                tx,
                audit,
                sigils: Mutex::new(HashSet::new()),
                chis: Mutex::new(HashSet::new()),
                closing: Notify::new(),
            },
            rx,
        )
    }

    pub fn send(&self, tone: Value) -> Result<(), mpsc::error::TrySendError<Value>> {
        self.audit.sent(&self.client_id, &tone);
        self.tx.try_send(tone)
    }

    pub fn close(&self) {
        self.closing.notify_one();
    }

    pub async fn closed(&self) {
        self.closing.notified().await;
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

    pub fn replace_chis(&self, chis: Vec<String>) {
        *self.chis.lock() = chis.into_iter().collect();
    }

    pub fn accepts_chi(&self, chi: &str) -> bool {
        let chis = self.chis.lock();
        chis.is_empty() || chis.contains(chi)
    }
}

pub(crate) struct Registry {
    by_id: HashMap<ClientId, std::sync::Arc<Reach>>,
}

impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            by_id: HashMap::new(),
        }
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
        assert!(
            reach.accepts_chi("prompt"),
            "no declared set = no filtering"
        );
        assert!(reach.accepts_chi("tool-call"));
    }

    #[test]
    fn a_client_that_declared_a_list_receives_only_those() {
        let (reach, _rx) = Reach::new("scoped".into());
        reach.replace_chis(vec!["prompt".into(), "chunk".into()]);
        assert!(reach.accepts_chi("prompt"));
        assert!(reach.accepts_chi("chunk"));
        assert!(
            !reach.accepts_chi("tool-call"),
            "an undeclared chi must not reach a scoped client"
        );
    }

    #[test]
    fn declaring_an_empty_list_is_not_scoped() {
        let (reach, _rx) = Reach::new("empty".into());
        reach.replace_chis(Vec::new());
        assert!(reach.accepts_chi("prompt"));
    }

    #[test]
    fn re_declaring_replaces_the_previous_scope() {
        let (reach, _rx) = Reach::new("rehello".into());
        reach.replace_chis(vec!["prompt".into(), "chunk".into()]);
        reach.replace_chis(vec!["prompt".into()]);
        assert!(reach.accepts_chi("prompt"));
        assert!(
            !reach.accepts_chi("chunk"),
            "the earlier, wider scope must not survive a re-hello"
        );
    }
}
