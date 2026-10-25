//! What the thrum declares a tone may carry.
//!
//! Every chi's body is the struct of the same name in `views`, and every
//! tone may carry the envelope. A key on the wire that is neither is
//! undeclared: no humd may rely on it and no bee should send it. A bee's
//! private data belongs under `ext`. The declared names are read from the
//! structs' own serde shape, so the views are the spec and cannot drift
//! from it; a new chi does not compile until it has a view.

use serde::de::{self, Deserialize, Deserializer, Visitor};
use serde_json::Value;

use crate::chi::Chi;
use crate::envelope::Envelope;
use crate::views::*;

struct Introspect<'a>(&'a mut &'static [&'static str]);

impl<'de> Deserializer<'de> for Introspect<'_> {
    type Error = de::value::Error;

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        *self.0 = fields;
        Err(de::Error::custom("introspected"))
    }

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(de::Error::custom("not a struct"))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

fn fields_of<'de, T: Deserialize<'de>>() -> &'static [&'static str] {
    let mut fields: &'static [&'static str] = &[];
    let _ = T::deserialize(Introspect(&mut fields));
    fields
}

pub fn envelope_keys() -> &'static [&'static str] {
    fields_of::<Envelope>()
}

pub fn body_keys(chi: Chi) -> &'static [&'static str] {
    match chi {
        Chi::Hello => fields_of::<HelloBody>(),
        Chi::Prompt => fields_of::<PromptBody>(),
        Chi::Cancel => fields_of::<CancelBody>(),
        Chi::Cleanup => fields_of::<CleanupBody>(),
        Chi::Curate => fields_of::<CurateBody>(),
        Chi::ReleasePermit => fields_of::<ReleasePermitBody>(),
        Chi::ScoutResult => fields_of::<ScoutResultBody>(),
        Chi::TendrilResult => fields_of::<TendrilResultBody>(),
        Chi::PetalCell => fields_of::<PetalCellBody>(),
        Chi::Breath => fields_of::<BreathBody>(),
        Chi::Unfurl => fields_of::<UnfurlBody>(),
        Chi::Wilt => fields_of::<WiltBody>(),
        Chi::Error => fields_of::<ErrorBody>(),
        Chi::Pulse => fields_of::<PulseBody>(),
        Chi::PermissionAsk => fields_of::<PermissionAskBody>(),
        Chi::Scout => fields_of::<ScoutBody>(),
        Chi::TendrilReach => fields_of::<TendrilReachBody>(),
        Chi::TendrilMeta => fields_of::<TendrilMetaBody>(),
        Chi::TendrilInfo => fields_of::<TendrilInfoBody>(),
        Chi::Echo => fields_of::<EchoBody>(),
        Chi::PerfMark => fields_of::<PerfMarkBody>(),
        Chi::Log => fields_of::<LogBody>(),
        Chi::Drone => fields_of::<DroneBody>(),
        Chi::DroneRetrofit => fields_of::<DroneRetrofitBody>(),
        Chi::PeerAdd => fields_of::<PeerAddBody>(),
        Chi::PeerRemove => fields_of::<PeerRemoveBody>(),
        Chi::Attach => fields_of::<AttachBody>(),
        Chi::Detach => fields_of::<DetachBody>(),
        Chi::WaneSync => fields_of::<WaneSyncBody>(),
        Chi::GossipPublish => fields_of::<GossipPublishBody>(),
        Chi::KadFindNode => fields_of::<KadFindNodeBody>(),
        Chi::KadFindNodeResp => fields_of::<KadFindNodeRespBody>(),
        Chi::Backfill => fields_of::<BackfillBody>(),
    }
}

pub fn undeclared(tone: &Value) -> Option<(Chi, Vec<String>)> {
    let obj = tone.as_object()?;
    let chi: Chi = serde_json::from_value(obj.get("chi")?.clone()).ok()?;
    let envelope = envelope_keys();
    let body = body_keys(chi);
    let stray: Vec<String> = obj
        .keys()
        .filter(|k| !envelope.contains(&k.as_str()) && !body.contains(&k.as_str()))
        .cloned()
        .collect();
    (!stray.is_empty()).then_some((chi, stray))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_spec_is_read_from_the_views() {
        assert!(envelope_keys().contains(&"sentAt"));
        assert!(envelope_keys().contains(&"ext"));
        assert!(body_keys(Chi::Hello).contains(&"hive"));
        assert!(body_keys(Chi::Hello).contains(&"signedAt"));
        assert!(body_keys(Chi::Cancel).is_empty());
    }

    #[test]
    fn every_chi_has_a_struct_shape() {
        for chi in [Chi::Hello, Chi::Prompt, Chi::Unfurl, Chi::Wilt, Chi::TendrilReach, Chi::Breath] {
            assert!(!body_keys(chi).is_empty(), "{chi:?} has no declared body");
        }
    }

    #[test]
    fn a_declared_tone_has_nothing_undeclared() {
        let tone = json!({"chi": "cancel", "rid": "r", "sid": "s", "ext": {"mine": {"x": 1}}});
        assert_eq!(undeclared(&tone), None);
    }

    #[test]
    fn a_stray_key_is_named() {
        let tone = json!({"chi": "hello", "rid": "r", "hive": "h", "toolNames": ["a"]});
        let (chi, stray) = undeclared(&tone).unwrap();
        assert_eq!(chi, Chi::Hello);
        assert_eq!(stray, ["toolNames"]);
    }

    #[test]
    fn a_tone_that_is_no_chi_is_not_judged() {
        assert_eq!(undeclared(&json!({"chi": "nonsense", "x": 1})), None);
        assert_eq!(undeclared(&json!("not an object")), None);
    }
}
