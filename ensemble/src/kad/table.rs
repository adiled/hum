use crate::{Hid, HumdAddr};

use super::distance::XorDistance;

#[derive(Debug)]
pub struct RoutingTable {
    me: Hid,
    buckets: Vec<super::bucket::KBucket>,
}

impl RoutingTable {
    pub fn new(me: Hid) -> Self {
        Self {
            me,
            buckets: (0..256).map(|_| super::bucket::KBucket::new()).collect(),
        }
    }

    pub fn me(&self) -> Hid {
        self.me
    }

    pub fn insert(&mut self, addr: HumdAddr) -> bool {
        if addr.id == self.me {
            return false;
        }
        let idx = XorDistance::bucket_index(&self.me, &addr.id);
        if idx >= 256 {
            return false;
        }
        self.buckets[idx].insert(addr);
        true
    }

    pub fn closest_to(&self, target: &Hid, count: usize) -> Vec<HumdAddr> {
        let mut all: Vec<HumdAddr> = self
            .buckets
            .iter()
            .flat_map(|b| b.iter().cloned())
            .collect();
        all.sort_by(|a, b| {
            XorDistance::distance(target, &a.id).cmp(&XorDistance::distance(target, &b.id))
        });
        all.truncate(count);
        all
    }

    pub fn forget(&mut self, id: &Hid) -> bool {
        let mut dropped = false;
        for bucket in &mut self.buckets {
            dropped |= bucket.retain(|a| &a.id != id);
        }
        dropped
    }

    pub fn entries(&self) -> Vec<HumdAddr> {
        self.buckets
            .iter()
            .flat_map(|b| b.iter().cloned())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: &Hid) -> Option<HumdAddr> {
        let idx = XorDistance::bucket_index(&self.me, id);
        if idx >= 256 {
            return None;
        }
        self.buckets[idx].iter().find(|a| &a.id == id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_table_closest_orders_by_distance() {
        let me = Hid::from([0u8; 32]);
        let mut t = RoutingTable::new(me);
        let mut near = [0u8; 32];
        near[31] = 0x01;
        let mut mid = [0u8; 32];
        mid[15] = 0x01;
        let mut far = [0u8; 32];
        far[0] = 0x80;

        t.insert(HumdAddr::new(Hid::from(far)));
        t.insert(HumdAddr::new(Hid::from(near)));
        t.insert(HumdAddr::new(Hid::from(mid)));

        let closest = t.closest_to(&me, 3);
        assert_eq!(closest.len(), 3);
        assert_eq!(closest[0].id, Hid::from(near));
        assert_eq!(closest[1].id, Hid::from(mid));
        assert_eq!(closest[2].id, Hid::from(far));
    }

    #[test]
    fn routing_table_drops_self_inserts() {
        let me = Hid::random_humd();
        let mut t = RoutingTable::new(me);
        assert!(!t.insert(HumdAddr::new(me)));
        assert!(t.is_empty());
    }
}
