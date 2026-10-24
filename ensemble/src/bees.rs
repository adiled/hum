//! On-mesh bee registry.
//!
//! Anybody can build a bee in their own repo, handshake it against a
//! humd that has its hive installed, and that humd gossips it to the
//! ensemble. No central registry, no PR to this repo.
//!
//! [`ANNOUNCE_TOPIC`] keeps its old path, and a manifest still reads
//! `name` as its hive and `bee` as its roles, so humds of different
//! versions keep hearing each other. Versioning lives inside the
//! manifest; unknown fields are tolerated so newer humds can add more.
//!
//! `roles` is a claim, never part of a bee's identity. `source` is
//! untrusted: surface it to humans, never auto-fetch it.

use serde::{Deserialize, Serialize};

pub const ANNOUNCE_TOPIC: &str = "hum/hives/announce";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeeManifest {
    #[serde(alias = "name")]
    pub hive: String,
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
    #[serde(default, alias = "bee", skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
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

impl BeeManifest {
    pub fn new(
        hive: impl Into<String>,
        version: impl Into<String>,
        proto_version: impl Into<String>,
    ) -> Self {
        Self {
            hive: hive.into(),
            version: version.into(),
            proto_version: proto_version.into(),
            propensity: Propensity::default(),
            chis: Vec::new(),
            source: None,
            bind: None,
            roles: Vec::new(),
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

    pub fn with_roles<I, S>(mut self, roles: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    pub fn plays(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BeeAnnounce {
    Advertise {
        humd_id: String,
        manifest: Box<BeeManifest>,
    },
    Retract { humd_id: String, name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_minimal() {
        let m = BeeManifest::new("market-maker", "0.1.0", "0.7.0");
        let j = serde_json::to_value(&m).unwrap();
        let back: BeeManifest = serde_json::from_value(j).unwrap();
        assert_eq!(back.hive, "market-maker");
        assert_eq!(back.proto_version, "0.7.0");
        assert!(back.chis.is_empty());
        assert!(back.source.is_none());
    }

    #[test]
    fn manifest_roundtrips_full() {
        let m = BeeManifest::new("market-maker", "0.1.0", "0.7.0")
            .with_propensity(Propensity {
                statefulness: Some("stateless".into()),
                richness: Some("medium".into()),
                wire: Some("custom/mm-v0".into()),
            })
            .with_chis(["hello", "gossip-publish", "tool-call", "tool-result"])
            .with_source("https://github.com/example/mm-bee");
        let j = serde_json::to_value(&m).unwrap();
        let back: BeeManifest = serde_json::from_value(j).unwrap();
        assert_eq!(back.chis.len(), 4);
        assert_eq!(back.propensity.statefulness.as_deref(), Some("stateless"));
        assert_eq!(
            back.source.as_deref(),
            Some("https://github.com/example/mm-bee")
        );
    }

    #[test]
    fn a_manifest_from_an_older_humd_still_reads() {
        let old = serde_json::json!({
            "name": "claude-cli",
            "version": "0.1.0",
            "proto_version": "0.7.0",
            "bee": ["worker"],
            "hid": "wbee_0101010101010101010101010101010101010101010101010101010101010101",
        });
        let m: BeeManifest = serde_json::from_value(old).unwrap();
        assert_eq!(m.hive, "claude-cli");
        assert!(m.plays("worker"));
        assert!(!m.plays("forager"));
        let now = serde_json::to_value(&m).unwrap();
        assert_eq!(now["hive"], "claude-cli");
        assert_eq!(now["roles"], serde_json::json!(["worker"]));
    }

    #[test]
    fn announce_envelope_tags_kind() {
        let env = BeeAnnounce::Advertise {
            humd_id: "deadbeef".into(),
            manifest: Box::new(BeeManifest::new("mm", "0.1.0", "0.7.0")),
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"kind\":\"advertise\""));
        let back: BeeAnnounce = serde_json::from_str(&s).unwrap();
        match back {
            BeeAnnounce::Advertise { humd_id, manifest } => {
                assert_eq!(humd_id, "deadbeef");
                assert_eq!(manifest.hive, "mm");
            }
            _ => panic!("wrong variant"),
        }
    }
}
