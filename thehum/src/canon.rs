//! Canonical serialization for hashing + signing.
//!
//! Determinism rule: two peers serializing the same logical Event must
//! produce identical bytes. `serde_json` doesn't sort keys by default,
//! so we walk the Value, copy maps into BTreeMap (alphabetic), and
//! re-emit as compact JSON.

use serde_json::Value;
use std::collections::BTreeMap;

/// Sorted-key canonical JSON bytes for the event MINUS the `sig` field.
/// Hash + signature are computed over this output.
pub fn canonical_bytes(event_without_sig: &Value) -> Vec<u8> {
    let sorted = sort_value(event_without_sig);
    serde_json::to_vec(&sorted).expect("canonical serialization")
}

fn sort_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (k, vv) in map {
                if k == "sig" { continue; }
                sorted.insert(k.clone(), sort_value(vv));
            }
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_value).collect()),
        other => other.clone(),
    }
}

/// Same shape but for an Event built in-code (not parsed from JSON).
/// Excludes `sig` from the hash + sign domain.
pub fn canonical_bytes_of(event: &crate::Event) -> Vec<u8> {
    let v = serde_json::to_value(event).expect("event → value");
    canonical_bytes(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn map_key_order_is_normalized() {
        let a = canonical_bytes(&json!({"b": 1, "a": 2, "c": 3}));
        let b = canonical_bytes(&json!({"a": 2, "c": 3, "b": 1}));
        let c = canonical_bytes(&json!({"c": 3, "b": 1, "a": 2}));
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn nested_maps_sorted() {
        let a = canonical_bytes(&json!({"x": {"b": 1, "a": 2}}));
        let b = canonical_bytes(&json!({"x": {"a": 2, "b": 1}}));
        assert_eq!(a, b);
    }

    #[test]
    fn sig_is_stripped() {
        let with_sig = canonical_bytes(&json!({"a": 1, "sig": "deadbeef"}));
        let without = canonical_bytes(&json!({"a": 1}));
        assert_eq!(with_sig, without);
    }

    #[test]
    fn arrays_preserve_order() {
        let a = canonical_bytes(&json!([3, 1, 2]));
        let b = canonical_bytes(&json!([3, 1, 2]));
        let c = canonical_bytes(&json!([1, 2, 3]));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
