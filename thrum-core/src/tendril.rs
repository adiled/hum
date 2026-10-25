//! The tendril the thrum declares.
//!
//! Most tendrils are named by the bee that serves them, an attendant
//! listing them in its hello's `tools`, or a forager on its prompt. One is
//! the thrum's own: `bash`, which a builder serves. Its shape is declared
//! here, so every nurse hands its cell the same tool and humd routes it by
//! role rather than by name.
//!
//! A `bash` reach carries a `command` and an optional `timeout` in ms. The
//! builder runs it in the sigil's shell, which starts where the bloom
//! builds and keeps its directory and environment from one reach to the
//! next. What it grasps is the command's output, stdout and stderr
//! together, its last line naming the exit code when it is not 0, with
//! the code in `metadata.exitCode` and `isError` set.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const BASH: &str = "bash";

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
}
