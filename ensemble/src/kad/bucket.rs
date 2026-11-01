use std::collections::VecDeque;

use crate::HumdAddr;

use super::KAD_K;

#[derive(Debug, Default, Clone)]
pub struct KBucket {
    entries: VecDeque<HumdAddr>,
}

impl KBucket {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(KAD_K),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &HumdAddr> {
        self.entries.iter()
    }

    pub fn insert(&mut self, addr: HumdAddr) {
        if let Some(pos) = self.entries.iter().position(|e| e.id == addr.id) {
            let mut existing = self.entries.remove(pos).expect("position valid");
            for hint in &addr.hints {
                if !existing.hints.iter().any(|h| h == hint) {
                    existing.hints.push(hint.clone());
                }
            }
            self.entries.push_back(existing);
            return;
        }
        if self.entries.len() >= KAD_K {
            self.entries.pop_front();
        }
        self.entries.push_back(addr);
    }

    pub fn retain(&mut self, keep: impl Fn(&HumdAddr) -> bool) -> bool {
        let before = self.entries.len();
        self.entries.retain(|a| keep(a));
        self.entries.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hid;

    #[test]
    fn kbucket_lru_evicts_oldest_when_full() {
        let mut b = KBucket::new();
        let ids: Vec<Hid> = (0..KAD_K + 1).map(|_| Hid::random_humd()).collect();
        for id in &ids {
            b.insert(HumdAddr::new(*id));
        }
        assert_eq!(b.len(), KAD_K);
        assert!(b.iter().all(|a| a.id != ids[0]));
        assert!(b.iter().any(|a| a.id == *ids.last().unwrap()));
    }

    #[test]
    fn kbucket_refresh_moves_to_back() {
        let mut b = KBucket::new();
        let a_id = Hid::random_humd();
        let b_id = Hid::random_humd();
        b.insert(HumdAddr::new(a_id));
        b.insert(HumdAddr::new(b_id));
        b.insert(HumdAddr::new(a_id));
        let order: Vec<Hid> = b.iter().map(|x| x.id).collect();
        assert_eq!(order, vec![b_id, a_id]);
    }
}
