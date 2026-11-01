use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::{Hid, HumdAddr};

use super::distance::XorDistance;
use super::state::KadState;
use super::wire::{find_node_tone, mint_query_id};
use super::KAD_K;

#[derive(Debug, Clone)]
pub enum KadFindOutcome {
    Found(HumdAddr),
    Exhausted,
    TimedOut,
}

impl KadFindOutcome {
    pub fn into_option(self) -> Option<HumdAddr> {
        match self {
            KadFindOutcome::Found(addr) => Some(addr),
            _ => None,
        }
    }
}

pub(crate) struct LookupShortlist {
    shortlist: Vec<HumdAddr>,
    queried: HashSet<Hid>,
    target: Hid,
}

impl LookupShortlist {
    pub(crate) fn new(target: Hid, seed: Vec<HumdAddr>) -> Self {
        let mut s = Self {
            shortlist: Vec::new(),
            queried: HashSet::new(),
            target,
        };
        for addr in seed {
            s.insert(addr);
        }
        s
    }

    pub(crate) fn insert(&mut self, addr: HumdAddr) {
        if self.shortlist.iter().any(|a| a.id == addr.id) {
            return;
        }
        self.shortlist.push(addr);
        let target = self.target;
        self.shortlist.sort_by(|a, b| {
            XorDistance::distance(&target, &a.id).cmp(&XorDistance::distance(&target, &b.id))
        });
        self.shortlist.truncate(KAD_K);
    }

    pub(crate) fn next_unqueried(&self, alpha: usize) -> Vec<HumdAddr> {
        self.shortlist
            .iter()
            .filter(|a| !self.queried.contains(&a.id))
            .take(alpha)
            .cloned()
            .collect()
    }

    pub(crate) fn mark_queried(&mut self, id: Hid) {
        self.queried.insert(id);
    }

    pub(crate) fn closest_distance(&self) -> [u8; 32] {
        self.shortlist
            .first()
            .map(|a| XorDistance::distance(&self.target, &a.id))
            .unwrap_or([0xff; 32])
    }

    pub(crate) fn closest(&self) -> Option<&HumdAddr> {
        self.shortlist.first()
    }
}

pub(crate) async fn query_peer(
    kad: &Arc<KadState>,
    conn: &Arc<dyn crate::PeerConnection>,
    stats: &Arc<crate::SendStats>,
    me: &Hid,
    target: &Hid,
    per_query_timeout: Duration,
) -> Vec<HumdAddr> {
    let query_id = mint_query_id();
    let rid = format!("kad-find-{}", &query_id[..8]);
    let rx = kad.register_query(query_id.clone());
    let tone = find_node_tone(&rid, &query_id, target, me);
    if crate::send_bounded(conn, tone, stats).await.is_err() {
        kad.cancel_query(&query_id);
        return Vec::new();
    }
    match tokio::time::timeout(per_query_timeout, rx).await {
        Ok(Ok(closest)) => closest,
        _ => {
            kad.cancel_query(&query_id);
            Vec::new()
        }
    }
}
