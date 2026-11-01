use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::{Hid, HumdAddr};

use super::table::RoutingTable;

pub struct KadState {
    pub table: Mutex<RoutingTable>,
    pending: Mutex<HashMap<String, oneshot::Sender<Vec<HumdAddr>>>>,
}

impl KadState {
    pub fn new(me: Hid) -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(RoutingTable::new(me)),
            pending: Mutex::new(HashMap::new()),
        })
    }

    pub fn forget(&self, id: &Hid) -> bool {
        self.table.lock().forget(id)
    }

    pub fn snapshot(&self) -> Vec<HumdAddr> {
        self.table.lock().entries()
    }

    pub fn restore(self: &Arc<Self>, addrs: Vec<HumdAddr>) -> usize {
        let mut table = self.table.lock();
        addrs
            .into_iter()
            .filter(|a| table.insert(a.clone()))
            .count()
    }

    pub fn note_peer(&self, addr: HumdAddr) {
        self.table.lock().insert(addr);
    }

    pub fn register_query(&self, query_id: String) -> oneshot::Receiver<Vec<HumdAddr>> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(query_id, tx);
        rx
    }

    pub fn cancel_query(&self, query_id: &str) {
        self.pending.lock().remove(query_id);
    }

    pub fn deliver_response(&self, query_id: &str, closest: Vec<HumdAddr>) -> bool {
        let tx = self.pending.lock().remove(query_id);
        if let Some(tx) = tx {
            let _ = tx.send(closest);
            true
        } else {
            false
        }
    }

    pub fn closest_to(&self, target: &Hid, count: usize) -> Vec<HumdAddr> {
        self.table.lock().closest_to(target, count)
    }

    pub fn get(&self, id: &Hid) -> Option<HumdAddr> {
        self.table.lock().get(id)
    }
}
