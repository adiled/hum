//! Canonical per-chi tone bodies.
//!
//! One struct per chi, field names renamed to EXACTLY the wire keys the
//! runtime produces. This module is the single source of truth for body
//! shapes; codegen emits the TS/Python/Go mirrors from it, and the
//! golden-message test locks the bytes.
//!
//! Optional fields are also `skip_serializing_if` so absent fields are
//! absent from the wire, mirroring how builders construct tones.
//!
//! Each `*Body` struct is the body of exactly one chi, named after it.
//! `DroneLoad` is the nested payload of a `drone` tone rather than a
//! chi of its own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloBody {
    #[serde(rename = "protoVersion")]
    pub proto_version: String,
    pub bee: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provides: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
    pub content: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "foragerTools")]
    pub forager_tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provided: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "disallowedTools")]
    pub disallowed_tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreathBody {
    pub sessions: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "protoVersion")]
    pub proto_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkBody {
    #[serde(rename = "chunkType")]
    pub chunk_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "blockIdx")]
    pub block_idx: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "partialJson")]
    pub partial_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishBody {
    #[serde(rename = "finishReason")]
    pub finish_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
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
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReadyBody {
    #[serde(rename = "nestId")]
    pub nest_id: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallBody {
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultBody {
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfoBody {
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetaBody {
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "toolName")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PulseBody {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAskBody {
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "toolName")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arg: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePermitBody {
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    pub pending_permissions: Option<u64>,
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
pub struct TendrilResultBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "callId")]
    pub call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TendrilReachBody {
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
    fn tool_result_wire_names() {
        let b = ToolResultBody {
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
    fn chunk_wire_names() {
        let b = ChunkBody {
            chunk_type: "text_delta".into(),
            block_idx: None,
            delta: Some(json!("hi")),
            partial_json: None,
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["chunkType"], "text_delta");
        assert_eq!(v["delta"], "hi");
        assert!(v.as_object().unwrap().contains_key("delta"));
    }
}
