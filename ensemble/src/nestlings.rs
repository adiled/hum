//! On-mesh nestling registry.
//!
//! Anybody can build a nestling in their own repo, import [`thrum_core`]
//! for the wire contract, handshake against their local humd, and humd
//! will advertise their nestling to the ensemble. Discovery is a gossip
//! topic — no central registry, no PR to this repo required.
//!
//! Two seams:
//!
//! - [`Ensemble::nestling_advertise`]: publish a [`NestlingManifest`] on
//!   [`ANNOUNCE_TOPIC`]. Called by humd whenever a local nestler completes
//!   handshake (and on a slow re-advertise heartbeat).
//! - [`Ensemble::nestling_discover`]: subscribe to [`ANNOUNCE_TOPIC`] and
//!   filter by name. Returns `(HumdId, NestlingManifest)` pairs.
//!
//! The wire envelope is a plain `gossip-publish` tone with a structured
//! payload — no new `chi` value needed. Manifests are loose JSON so a
//! future-dated humd can ship extra fields without breaking older peers.

use serde::{Deserialize, Serialize};

/// Gossip topic that carries every nestling advertise + heartbeat.
///
/// Stable across THRUM_VERSION bumps so old humds keep hearing each other
/// when nestling shapes evolve. Versioning lives inside the manifest.
pub const ANNOUNCE_TOPIC: &str = "hum/nestlings/announce";

/// Self-description a nestler hands its local humd, which humd then
/// broadcasts on [`ANNOUNCE_TOPIC`] so other humds in the ensemble can
/// discover it.
///
/// The `chi` field is *advisory* — what the nestling intends to send
/// and receive. Other humds use it to decide if their own nestlings can
/// transact with this one. Mismatches are warnings, not hard errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestlingManifest {
    /// Short identifier — `"market-maker"`, `"openai-server"`. Becomes
    /// the lookup key in [`Ensemble::nestling_discover`].
    pub name: String,
    /// Semver of the nestling's own release. Independent of `proto_version`.
    pub version: String,
    /// `THRUM_VERSION` the nestling speaks. Receivers warn on mismatch.
    pub proto_version: String,
    /// Statefulness × richness × wire-shape — see `nestlings/README.md`.
    #[serde(default)]
    pub propensity: Propensity,
    /// Chi values the nestling sends or expects to receive. Kebab-case,
    /// matches the wire form of [`thrum_core::Chi`]. Empty = unspecified
    /// (assume only the universal handshake subset).
    #[serde(default)]
    pub chi: Vec<String>,
    /// Free-form pointer to the nestling's docs/source. Untrusted —
    /// readers MUST NOT auto-fetch; surface to humans only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Three orthogonal axes from `nestlings/README.md`. Strings on the wire
/// so adding a new dimension or value never breaks parsers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Propensity {
    /// `"stateful" | "convention-stateful" | "stateless" | "transport-only"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statefulness: Option<String>,
    /// `"rich" | "medium" | "lean" | "opaque"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub richness: Option<String>,
    /// Free-form wire shape — `"openai/chat-completions"`, `"vercel-ai/v3"`,
    /// `"grpc/bidi"`, `"custom"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<String>,
}

impl NestlingManifest {
    /// Minimum useful manifest — name + version + proto_version. All
    /// other fields default.
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
            chi: Vec::new(),
            source: None,
        }
    }

    pub fn with_propensity(mut self, propensity: Propensity) -> Self {
        self.propensity = propensity;
        self
    }

    pub fn with_chi<I, S>(mut self, chi: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.chi = chi.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Payload shape carried inside the gossip-publish tone. Wrapping the
/// manifest in a thin envelope lets future tones (deprecate, retract)
/// live on the same topic without ambiguity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NestlingAnnounce {
    /// Nestling is live on `humd_id`.
    Advertise {
        humd_id: String,
        manifest: NestlingManifest,
    },
    /// Nestling has shut down on `humd_id`.
    Retract { humd_id: String, name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_minimal() {
        let m = NestlingManifest::new("market-maker", "0.1.0", "0.7.0");
        let j = serde_json::to_value(&m).unwrap();
        let back: NestlingManifest = serde_json::from_value(j).unwrap();
        assert_eq!(back.name, "market-maker");
        assert_eq!(back.proto_version, "0.7.0");
        assert!(back.chi.is_empty());
        assert!(back.source.is_none());
    }

    #[test]
    fn manifest_roundtrips_full() {
        let m = NestlingManifest::new("market-maker", "0.1.0", "0.7.0")
            .with_propensity(Propensity {
                statefulness: Some("stateless".into()),
                richness: Some("medium".into()),
                wire: Some("custom/mm-v0".into()),
            })
            .with_chi(["hello", "gossip-publish", "tool-call", "tool-result"])
            .with_source("https://github.com/example/mm-nestling");
        let j = serde_json::to_value(&m).unwrap();
        let back: NestlingManifest = serde_json::from_value(j).unwrap();
        assert_eq!(back.chi.len(), 4);
        assert_eq!(back.propensity.statefulness.as_deref(), Some("stateless"));
        assert_eq!(back.source.as_deref(), Some("https://github.com/example/mm-nestling"));
    }

    #[test]
    fn announce_envelope_tags_kind() {
        let env = NestlingAnnounce::Advertise {
            humd_id: "deadbeef".into(),
            manifest: NestlingManifest::new("mm", "0.1.0", "0.7.0"),
        };
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"kind\":\"advertise\""));
        let back: NestlingAnnounce = serde_json::from_str(&s).unwrap();
        match back {
            NestlingAnnounce::Advertise { humd_id, manifest } => {
                assert_eq!(humd_id, "deadbeef");
                assert_eq!(manifest.name, "mm");
            }
            _ => panic!("wrong variant"),
        }
    }
}
