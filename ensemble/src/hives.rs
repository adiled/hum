//! On-mesh bee registry.
//!
//! Anybody can build a bee in their own repo, import [`thrum_core`]
//! for the wire contract, handshake against their local humd, and humd
//! will advertise their bee to the ensemble. Discovery is a gossip
//! topic — no central registry, no PR to this repo required.
//!
//! Two seams:
//!
//! - [`Ensemble::hive_advertise`]: publish a [`HiveManifest`] on
//!   [`ANNOUNCE_TOPIC`]. Called by humd whenever a local nestler completes
//!   handshake (and on a slow re-advertise heartbeat).
//! - [`Ensemble::hive_discover`]: subscribe to [`ANNOUNCE_TOPIC`] and
//!   filter by name. Returns `(Hid, HiveManifest)` pairs.
//!
//! The wire envelope is a plain `gossip-publish` tone with a structured
//! payload — no new `chi` value needed. Manifests are loose JSON so a
//! future-dated humd can ship extra fields without breaking older peers.
//! The payload is a [`HiveAnnounce`], a thin envelope so future tones
//! (deprecate, retract) can live on the same topic without ambiguity.
//!
//! [`ANNOUNCE_TOPIC`] is stable across `THRUM_VERSION` bumps so old humds
//! keep hearing each other when bee shapes evolve. Versioning lives inside
//! the manifest.
//!
//! `source` is untrusted: surface it to humans, never auto-fetch it.
//!
//! `chis` is the list a bee advertises, distinct from the `chi`
//! discriminator on every tone. Empty means unspecified, and a bee
//! advertising a chi humd cannot serve is a warning, not an error.
//!
//! `hid` is the stable identity, distinct from the transient thrum
//! `client_id` humd assigns per connection. It is the dedup key: humd
//! keys a bee by `hid`, falling back to `nestler_id`, then `name`.
//! `nestler_id` is what lets two nestlers of the same kind register
//! without their manifests collapsing into one.

use serde::{Deserialize, Serialize};

pub const ANNOUNCE_TOPIC: &str = "hum/hives/announce";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiveManifest {
    pub name: String,
    pub version: String,
    pub proto_version: String,
    #[serde(default)]
    pub propensity: Propensity,
    #[serde(default, alias = "chi")]
    pub chis: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<BindAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "nestlerId")]
    pub nestler_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bee: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hid: Option<crate::Hid>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hid_verified: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindAddr {
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Propensity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statefulness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub richness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<String>,
}

impl HiveManifest {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        proto_version: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            proto_version: proto_version.into(),
            propensity: Propensity::default(),
            chis: Vec::new(),
            source: None,
            bind: None,
            bee: Vec::new(),
            models: Vec::new(),
            tools: Vec::new(),
            provides: Vec::new(),
            nestler_id: None,
            hid: None,
            hid_verified: false,
        }
    }

    pub fn with_propensity(mut self, propensity: Propensity) -> Self {
        self.propensity = propensity;
        self
    }

    pub fn with_bind(mut self, bind: BindAddr) -> Self {
        self.bind = Some(bind);
        self
    }

    pub fn with_nestler_id(mut self, id: impl Into<String>) -> Self {
        self.nestler_id = Some(id.into());
        self
    }

    pub fn with_chis<I, S>(mut self, chis: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.chis = chis.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HiveAnnounce {
    Advertise {
        humd_id: String,
        manifest: Box<HiveManifest>,
    },
    Retract { humd_id: String, name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_minimal() {
        let m = HiveManifest::new("market-maker", "0.1.0", "0.7.0");
        let j = serde_json::to_value(&m).unwrap();
        let back: HiveManifest = serde_json::from_value(j).unwrap();
        assert_eq!(back.name, "market-maker");
        assert_eq!(back.proto_version, "0.7.0");
        assert!(back.chis.is_empty());
        assert!(back.source.is_none());
    }

    #[test]
    fn manifest_roundtrips_full() {
        let m = HiveManifest::new("market-maker", "0.1.0", "0.7.0")
            .with_propensity(Propensity {
                statefulness: Some("stateless".into()),
                richness: Some("medium".into()),
                wire: Some("custom/mm-v0".into()),
            })
            .with_chis(["hello", "gossip-publish", "tool-call", "tool-result"])
            .with_source("https://github.com/example/mm-bee");
        let j = serde_json::to_value(&m).unwrap();
        let back: HiveManifest = serde_json::from_value(j).unwrap();
        assert_eq!(back.chis.len(), 4);
        assert_eq!(back.propensity.statefulness.as_deref(), Some("stateless"));
        assert_eq!(
            back.source.as_deref(),
            Some("https://github.com/example/mm-bee")
        );
    }

    #[test]
    fn announce_envelope_tags_kind() {
        let env = HiveAnnounce::Advertise {
            humd_id: "deadbeef".into(),
            manifest: Box::new(HiveManifest::new("mm", "0.1.0", "0.7.0")),
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"kind\":\"advertise\""));
        let back: HiveAnnounce = serde_json::from_str(&s).unwrap();
        match back {
            HiveAnnounce::Advertise { humd_id, manifest } => {
                assert_eq!(humd_id, "deadbeef");
                assert_eq!(manifest.name, "mm");
            }
            _ => panic!("wrong variant"),
        }
    }
}
