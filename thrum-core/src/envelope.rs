//! Envelope and Tone.
//!
//! The envelope holds the fields every tone may carry. The Tone wraps an
//! envelope with the chi-specific body as a raw JSON map — typed views
//! per chi belong in a follow-up crate so this one stays small.
//!
//! `chi` and `rid` are required; everything else is situational. `ext`
//! is the bee-private extension bag — thrum core ignores it, each bee
//! owns its own key (e.g. `ext.opencode.serverUrl`).
//!
//! The body lives in a raw `serde_json::Map` because tone shapes vary
//! per chi. Code that needs strongly typed access decodes the body into
//! the appropriate per-chi struct. Serializing a `Tone` flattens body
//! fields up next to the envelope ones into one flat wire object.
//!
//! Envelope fields win that merge — a body can never shadow `chi`,
//! `rid` or any other envelope key. Deserializing splits them the other
//! way: the envelope keys are pulled out and everything left is body.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::chi::Chi;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub chi: Chi,
    pub rid: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigil: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wane: Option<u64>,
    #[serde(rename = "sentAt", skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dusk: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<BTreeMap<String, BTreeMap<String, Value>>>,
}

impl Envelope {
    pub fn new(chi: Chi, rid: impl Into<String>) -> Self {
        Self {
            chi,
            rid: rid.into(),
            from: None,
            to: None,
            sigil: None,
            sid: None,
            wane: None,
            sent_at: None,
            dusk: None,
            mid: None,
            ext: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tone {
    pub envelope: Envelope,
    pub body: Map<String, Value>,
}

impl Tone {
    pub fn new(envelope: Envelope) -> Self {
        Self { envelope, body: Map::new() }
    }

    pub fn with_body(envelope: Envelope, body: Map<String, Value>) -> Self {
        Self { envelope, body }
    }

    pub fn chi(&self) -> Chi {
        self.envelope.chi
    }

    pub fn rid(&self) -> &str {
        &self.envelope.rid
    }
}

impl Serialize for Tone {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let env_value = serde_json::to_value(&self.envelope).map_err(serde::ser::Error::custom)?;
        let Value::Object(mut merged) = env_value else {
            return Err(serde::ser::Error::custom("envelope did not serialize to an object"));
        };
        for (k, v) in &self.body {
            merged.entry(k.clone()).or_insert_with(|| v.clone());
        }
        merged.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for Tone {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let mut all: Map<String, Value> = Map::deserialize(de)?;
        const ENV_KEYS: &[&str] =
            &["chi", "rid", "from", "to", "sigil", "sid", "wane", "sentAt", "dusk", "mid", "ext"];
        let mut env_map = Map::with_capacity(ENV_KEYS.len());
        for k in ENV_KEYS {
            if let Some(v) = all.remove(*k) {
                env_map.insert((*k).to_string(), v);
            }
        }
        let envelope: Envelope =
            serde_json::from_value(Value::Object(env_map)).map_err(serde::de::Error::custom)?;
        Ok(Tone { envelope, body: all })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tone_round_trips_with_flat_wire_shape() {
        let wire = json!({
            "chi": "tool-call",
            "rid": "abc-1",
            "sid": "s1",
            "from": "humd",
            "name": "Read",
            "args": { "path": "/tmp/x" },
            "callId": "call-9",
        });
        let tone: Tone = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(tone.chi(), Chi::ToolCall);
        assert_eq!(tone.rid(), "abc-1");
        assert_eq!(tone.envelope.sid.as_deref(), Some("s1"));
        assert_eq!(tone.body.get("name").and_then(|v| v.as_str()), Some("Read"));

        let back = serde_json::to_value(&tone).unwrap();
        assert_eq!(back, wire);
    }
}
