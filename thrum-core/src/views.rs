//! Canonical per-chi tone bodies.
//!
//! One struct per chi, field names renamed to EXACTLY the wire keys the
//! runtime produces. This module is the single source of truth for body
//! shapes; codegen emits the TS/Python/Go/C mirrors from it, and the
//! golden-message test locks the bytes.
//!
//! Optional fields are also `skip_serializing_if` so absent fields are
//! absent from the wire, mirroring how builders construct tones.
//!
//! A hello names the `hive` the bee belongs to and the `role` it was
//! entered for, and proves the bee is its `hid` by a signature from the
//! key that hashes to it. A forager whose blooms build where it is says
//! `builds`.
//!
//! A nurse says how many more cells it can take in `headroom`, on its
//! hello and on each cell pulse; humd routes by it.
//!
//! A bloom opens with a `prompt` and closes with a `wilt`. The prompt
//! carries its petals in `content` and how the cell should grow the
//! bloom: `temperature`, `prune`, `season`, `frost`, `seed`, `trellis`,
//! `tendrils`, `sepal`. What the cell grows comes back as `unfurl` tones,
//! each growing one petal named by its kind until the petal is `closed`,
//! and the nectar the bloom took rides on its wilt. A prompt's `cwd` is
//! where the bloom builds, `hum://<humdid>/<path>`.
//!
//! Each `*Body` struct is the body of exactly one chi, named after it.
//! `DroneLoad`, `Petal`, `Grain`, `Prune`, `Tendrils` and `Sepal` are
//! shapes nested in a body rather than chis of their own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloBody {
    #[serde(rename = "protoVersion")]
    pub proto_version: String,
    pub hive: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builds: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub hid: String,
    pub pubkey: String,
    #[serde(rename = "signedAt")]
    pub signed_at: i64,
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chis: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub propensity: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headroom: Option<Headroom>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Headroom {
    #[serde(rename = "freeSlots")]
    pub free_slots: u32,
    #[serde(rename = "totalSlots")]
    pub total_slots: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "modelId")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "systemPrompt")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Petal>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "allowedTools")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prune: Option<Prune>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub season: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frost: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trellis: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tendrils: Option<Tendrils>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sepal: Option<Sepal>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Petal {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grain: Option<Grain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub propolis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brix: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bouquet: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grain {
    pub kind: String,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prune {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tendrils {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reach: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sepal {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreathBody {
    pub sigils: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "protoVersion")]
    pub proto_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnfurlBody {
    pub petal: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sap: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grain: Option<Grain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub propolis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brix: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WiltBody {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nectar: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nectar: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TendrilReachBody {
    #[serde(rename = "callId")]
    pub call_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TendrilResultBody {
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TendrilInfoBody {
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TendrilMetaBody {
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseBody {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headroom: Option<Headroom>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EchoBody {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroneBody {
    pub health: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rhythm_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_echoes: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load: Option<DroneLoad>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroneLoad {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_sigils: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inflight_tools: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_burned: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroneRetrofitBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigil: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAddBody {
    pub humd_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRemoveBody {
    pub humd_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachBody {
    #[serde(rename = "hearOnly")]
    pub hear_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetachBody {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaneSyncBody {
    pub snapshot: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipPublishBody {
    pub topic: String,
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(rename = "msg_id")]
    pub msg_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KadFindNodeBody {
    #[serde(rename = "query_id")]
    pub query_id: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KadFindNodeRespBody {
    #[serde(rename = "query_id")]
    pub query_id: String,
    pub closest: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackfillBody {
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfMarkBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelBody {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupBody {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurateBody {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoutResultBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoutBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PetalCellBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn echo_shapes_canonically() {
        let b = EchoBody {
            ok: true,
            error: None,
        };
        assert_eq!(serde_json::to_value(&b).unwrap(), json!({ "ok": true }));
    }

    #[test]
    fn tendril_result_wire_names() {
        let b = TendrilResultBody {
            call_id: "c1".into(),
            output: Some("done".into()),
            result: None,
            is_error: Some(false),
            title: None,
            metadata: None,
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["callId"], "c1");
        assert_eq!(v["output"], "done");
        assert_eq!(v["isError"], false);
        assert!(!v.as_object().unwrap().contains_key("result"));
    }

    #[test]
    fn a_petal_unfurls_by_kind() {
        let b = UnfurlBody {
            petal: 0,
            kind: "text".into(),
            text: Some("hi".into()),
            sap: None,
            call_id: None,
            name: None,
            grain: None,
            propolis: None,
            source: None,
            brix: None,
            closed: None,
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v, json!({"petal": 0, "kind": "text", "text": "hi"}));
    }

    #[test]
    fn a_wilt_carries_its_nectar() {
        let b = WiltBody {
            reason: "stop".into(),
            nectar: Some(json!({"input_tokens": 3})),
            exit_code: None,
            subtype: None,
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["reason"], "stop");
        assert_eq!(v["nectar"]["input_tokens"], 3);
    }
}
