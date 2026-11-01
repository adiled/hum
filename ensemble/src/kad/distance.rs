use crate::Hid;

pub struct XorDistance;

impl XorDistance {
    pub fn distance(a: &Hid, b: &Hid) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (slot, (x, y)) in out.iter_mut().zip(a.as_bytes().iter().zip(b.as_bytes())) {
            *slot = x ^ y;
        }
        out
    }

    pub fn bucket_index(me: &Hid, peer: &Hid) -> usize {
        let d = Self::distance(me, peer);
        for (i, byte) in d.iter().enumerate() {
            if *byte != 0 {
                return i * 8 + byte.leading_zeros() as usize;
            }
        }
        256
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_distance_is_symmetric() {
        let a = Hid::random_humd();
        let b = Hid::random_humd();
        assert_eq!(XorDistance::distance(&a, &b), XorDistance::distance(&b, &a));
        assert_eq!(XorDistance::distance(&a, &a), [0u8; 32]);
    }

    #[test]
    fn bucket_index_zero_for_self() {
        let me = Hid::random_humd();
        assert_eq!(XorDistance::bucket_index(&me, &me), 256);
    }

    #[test]
    fn bucket_index_high_for_close_ids() {
        let mut bytes = [0u8; 32];
        bytes[31] = 0x01;
        let me = Hid::from([0u8; 32]);
        let other = Hid::from(bytes);
        assert_eq!(XorDistance::bucket_index(&me, &other), 255);
    }

    #[test]
    fn bucket_index_zero_for_inverted_top_bit() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0x80;
        let me = Hid::from([0u8; 32]);
        let other = Hid::from(bytes);
        assert_eq!(XorDistance::bucket_index(&me, &other), 0);
    }
}
