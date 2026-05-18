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
}

impl Reach {
    pub fn new(client_id: ClientId) -> (Self, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel(OUTBOUND_CAPACITY);
        (
            Self {
                client_id,
                tx,
                sigils: Mutex::new(HashSet::new()),
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
}

pub struct Registry {
    by_id: HashMap<ClientId, std::sync::Arc<Reach>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { by_id: HashMap::new() }
    }

    pub fn insert(&mut self, reach: std::sync::Arc<Reach>) {
        self.by_id.insert(reach.client_id.clone(), reach);
    }

    pub fn remove(&mut self, client_id: &str) {
        self.by_id.remove(client_id);
    }

    pub fn get(&self, client_id: &str) -> Option<&Reach> {
        self.by_id.get(client_id).map(|r| r.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Reach> {
        self.by_id.values().map(|r| r.as_ref())
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }
}
