//! The tendrils the thrum declares.
//!
//! Most tendrils are named by the bee that serves them, an attendant
//! listing them in its hello's `tools`, or a forager on its prompt. Two
//! are the thrum's own, their shapes declared here so humd routes them by
//! what they need rather than by name: `bash`, which a builder serves
//! where the bloom builds, and `scent`, which an attendant serves for the
//! models its hello names.
//!
//! A `bash` reach carries a `command` and an optional `timeout` in ms. The
//! builder runs it in the sigil's shell, which starts where the bloom
//! builds and keeps its directory and environment from one reach to the
//! next. What it grasps is the command's output, stdout and stderr
//! together, its last line naming the exit code when it is not 0, with
//! the code in `metadata.exitCode` and `isError` set.
//!
//! A `scent` reach carries the `model` to distil by, the texts as `input`,
//! and optionally the `dimensions` the scents should have. What it grasps,
//! as its `result`, is one scent per text, in order, each a vector of
//! numbers that sits near the scents of texts that mean alike, with the
//! nectar it took. A forager reaches it with no bloom between, and no
//! bloom is handed it: a cell makes words and has no use for one.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const BASH: &str = "bash";

pub const SCENT: &str = "scent";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScentArgs {
    pub model: String,
    pub input: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scents {
    pub scents: Vec<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nectar: Option<Value>,
}

pub const BASH_TIMEOUT_MS: u64 = 120_000;

pub const BASH_TIMEOUT_MOST_MS: u64 = 600_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashArgs {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl BashArgs {
    pub fn timeout_ms(&self) -> u64 {
        self.timeout.unwrap_or(BASH_TIMEOUT_MS).clamp(1, BASH_TIMEOUT_MOST_MS)
    }
}

pub fn bash_tool() -> Value {
    json!({
        "name": BASH,
        "description": format!(
            "Run a command in a bash shell on the host this conversation works on. \
             The shell keeps its working directory and environment from one call to the next. \
             Returns stdout and stderr together, and the exit code when it is not 0. \
             A command stops after `timeout` ms, {BASH_TIMEOUT_MS} by default and {BASH_TIMEOUT_MOST_MS} at most."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to run." },
                "timeout": { "type": "integer", "description": "How long it may run, in ms." }
            },
            "required": ["command"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bash_tool_is_named_and_shaped() {
        let tool = bash_tool();
        assert_eq!(tool["name"], BASH);
        assert_eq!(tool["inputSchema"]["required"][0], "command");
        let args: BashArgs = serde_json::from_value(json!({"command": "ls"})).unwrap();
        assert_eq!(args.timeout_ms(), BASH_TIMEOUT_MS);
        let long: BashArgs = serde_json::from_value(json!({"command": "ls", "timeout": 9_999_999})).unwrap();
        assert_eq!(long.timeout_ms(), BASH_TIMEOUT_MOST_MS);
    }

    #[test]
    fn a_scent_names_its_model_and_comes_back_in_order() {
        let args: ScentArgs = serde_json::from_value(json!({"model": "m", "input": ["a", "b"]})).unwrap();
        assert_eq!(args.input.len(), 2);
        assert!(args.dimensions.is_none());
        let grasped: Scents = serde_json::from_value(json!({"scents": [[0.5, 1.0], [0.0, 0.25]]})).unwrap();
        assert_eq!(grasped.scents[1], vec![0.0, 0.25]);
    }
}
