//! A prompt's graft, stitched into a Claude session transcript.
//!
//! Each bloom becomes a user turn of its words, then the cell's words and
//! tendrils as assistant turns and each grasp as the tool result that
//! answered them, the tendril named as the cell knew it over MCP. A
//! transcript that already holds the graft's blooms is left as it is.

use std::sync::{Mutex, MutexGuard, OnceLock};

use claude_cli::graft::{create_claude_session, graft, prior_of, session_path, tell};
use serde_json::{Value, json};
use thrum_core::{Bloom, Petal};

fn home() -> (tempfile::TempDir, MutexGuard<'static, ()>) {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    unsafe { std::env::set_var("HOME", dir.path()) };
    (dir, guard)
}

fn petal(v: Value) -> Petal {
    serde_json::from_value(v).unwrap()
}

fn blooms() -> Vec<Bloom> {
    vec![
        Bloom {
            content: Some(vec![petal(json!({"kind": "text", "text": "what is here?"}))]),
            grew: Some(vec![
                petal(json!({"kind": "tendril", "callId": "c-1", "name": "bash", "args": {"command": "ls"}})),
                petal(json!({"kind": "grasp", "callId": "c-1", "result": "a.txt"})),
                petal(json!({"kind": "text", "text": "There is a.txt."})),
            ]),
        },
        Bloom {
            content: Some(vec![petal(json!({"kind": "text", "text": "and else?"}))]),
            grew: Some(vec![petal(json!({"kind": "text", "text": "Nothing else."}))]),
        },
    ]
}

#[test]
fn a_graft_reads_as_the_turns_the_cell_would_have_taken() {
    let prior = prior_of(&blooms());
    let roles: Vec<&str> = prior.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "tool", "assistant", "user", "assistant"]);
    assert_eq!(prior[1]["content"][0]["toolName"], "mcp__hum__bash");
    assert_eq!(prior[1]["content"][0]["input"]["command"], "ls");
    assert_eq!(prior[2]["content"][0]["toolCallId"], "c-1");
    assert_eq!(prior[2]["content"][0]["result"], "a.txt");
}

#[test]
fn a_system_prompt_is_told_in_the_first_user_turn_of_a_graft() {
    let mut prior = prior_of(&blooms());
    assert!(tell(&mut prior, "be brief"));
    assert_eq!(prior[0]["content"][0]["text"], "be brief");
    assert_eq!(prior[0]["content"][1]["text"], "what is here?");
    assert_eq!(prior[4]["content"][0]["text"], "and else?", "only the first turn is told");
}

#[test]
fn a_graft_is_stitched_once() {
    let (dir, _guard) = home();
    let cwd = dir.path().join("nest");
    std::fs::create_dir_all(&cwd).unwrap();
    let session = "00000000-0000-4000-8000-000000000001";
    create_claude_session(&cwd, session).unwrap();
    let path = session_path(&cwd, session);

    let first = graft(&prior_of(&blooms()), &path, session, &cwd, None).unwrap();
    assert_eq!(first.grafted, 3, "every assistant turn is written");
    let lines: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let tool_use = lines.iter().find(|l| l["message"]["content"][0]["type"] == "tool_use").expect("the tendril is a tool use");
    assert_eq!(tool_use["message"]["content"][0]["name"], "mcp__hum__bash");
    let answer = lines.iter().find(|l| l["message"]["content"][0]["type"] == "tool_result").expect("the grasp answers it");
    assert_eq!(answer["message"]["content"][0]["tool_use_id"], "c-1");

    let again = graft(&prior_of(&blooms()), &path, session, &cwd, None).unwrap();
    assert_eq!(again.grafted, 0, "a transcript that holds the blooms is left as it is");
}
