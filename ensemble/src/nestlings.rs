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
    ///
    /// Plural intentionally: `chi` is the sacred discriminator on every
    /// tone (one chi per tone). `chis` is the **list** a nestling
    /// advertises — what kinds of tones it speaks. Different concepts,
    /// different names.
    #[serde(default, alias = "chi")]
    pub chis: Vec<String>,
    /// Free-form pointer to the nestling's docs/source. Untrusted —
    /// readers MUST NOT auto-fetch; surface to humans only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Network surface this nestler binds to receive outside-world
    /// traffic. `None` for nestlers that don't open a port (libraries,
    /// stdio CLIs). Two nestlers with the same kind + same `bind` are
    /// colocated nestleds — disambiguated by [`Self::nestler_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<BindAddr>,
    /// Per-instance id, distinct from the kind name. Lets two nestlers
    /// of the same nestling kind register without the manifest
    /// collapsing into one. Minted by the nestler (any unique string —
    /// UUID, pid+timestamp, etc.) or by humd at hello-accept time if
    /// the nestler doesn't supply one.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "nestlerId")]
    pub nestler_id: Option<String>,
    /// Wire role. `"nestler"` (the default, faces an outside wire and
    /// translates to thrum) vs `"perch"` (the model-side: accepts
    /// `chi:"prompt"` for advertised models and emits `chi:"chunk"`/
    /// `"finish"`). humd consults this to route prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Model ids this entry can serve. Meaningful only when
    /// `role == "perch"`; nestlers may carry an empty list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// Network address a nestler advertises. Optional fields stay optional
/// so non-network nestlers can emit `bind: None` cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindAddr {
    /// `"127.0.0.1"`, `"0.0.0.0"`, `"::1"`, etc.
    pub host: String,
    /// Post-bind port. A nestler that requested port 0 reports the
    /// kernel-assigned port here.
    pub port: u16,
    /// `"http"`, `"grpc"`, `"udp"`, `"sse"`, … free-form. None when the
    /// scheme is obvious from context (the nestling's `propensity.wire`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
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
            chis: Vec::new(),
            source: None,
            bind: None,
            role: None,
            models: Vec::new(),
            nestler_id: None,
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
        assert!(back.chis.is_empty());
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
            .with_chis(["hello", "gossip-publish", "tool-call", "tool-result"])
            .with_source("https://github.com/example/mm-nestling");
        let j = serde_json::to_value(&m).unwrap();
        let back: NestlingManifest = serde_json::from_value(j).unwrap();
        assert_eq!(back.chis.len(), 4);
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
