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

fn lines_of(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn delivered(mut b: Vec<Bloom>) -> Vec<Bloom> {
    let last = b.last_mut().unwrap().grew.get_or_insert_with(Vec::new);
    last.push(petal(json!({"kind": "tendril", "callId": "check_17", "name": "zsh_check", "args": {"id": 17}})));
    last.push(petal(json!({"kind": "grasp", "callId": "check_17", "result": "job 17 finished (exit 0)"})));
    b
}

#[test]
fn only_a_graft_ending_in_tool_results_is_answered() {
    use claude_cli::graft::answered;
    assert!(!answered(&blooms()), "it ends in words");
    assert!(answered(&delivered(blooms())));
    let mut thought = delivered(blooms());
    thought.last_mut().unwrap().grew.as_mut().unwrap().push(petal(json!({"kind": "sepal", "text": "hm"})));
    assert!(answered(&thought), "thinking aside");
    assert!(!answered(&[]));
}

#[test]
fn a_graft_ending_in_tool_results_keeps_them_last() {
    let (dir, _guard) = home();
    let cwd = dir.path().join("nest");
    std::fs::create_dir_all(&cwd).unwrap();
    let session = "00000000-0000-4000-8000-000000000002";
    create_claude_session(&cwd, session).unwrap();
    let path = session_path(&cwd, session);

    graft(&prior_of(&delivered(blooms())), &path, session, &cwd, None).unwrap();
    let lines = lines_of(&path);
    let last = lines.last().unwrap();
    assert_eq!(last["type"], "user");
    assert_eq!(last["message"]["content"][0]["type"], "tool_result");
    assert_eq!(last["message"]["content"][0]["tool_use_id"], "check_17");
}

#[test]
fn tool_results_a_held_transcript_lacks_are_stitched_on() {
    let (dir, _guard) = home();
    let cwd = dir.path().join("nest");
    std::fs::create_dir_all(&cwd).unwrap();
    let session = "00000000-0000-4000-8000-000000000003";
    create_claude_session(&cwd, session).unwrap();
    let path = session_path(&cwd, session);

    graft(&prior_of(&blooms()), &path, session, &cwd, None).unwrap();
    let before = lines_of(&path).len();
    let late = graft(&prior_of(&delivered(blooms())), &path, session, &cwd, None).unwrap();
    assert_eq!(late.grafted, 1, "the late call is written");
    let lines = lines_of(&path);
    assert_eq!(lines.len(), before + 2, "only the late call and its result");
    assert_eq!(lines[before]["message"]["content"][0]["type"], "tool_use");
    assert_eq!(lines[before]["message"]["content"][0]["id"], "check_17");
    assert_eq!(lines[before + 1]["message"]["content"][0]["tool_use_id"], "check_17");

    let again = graft(&prior_of(&delivered(blooms())), &path, session, &cwd, None).unwrap();
    assert_eq!(again.grafted, 0, "held results are not stitched twice");
    assert_eq!(lines_of(&path).len(), before + 2);
}

#[test]
fn the_continue_line_opens_no_turn() {
    let (dir, _guard) = home();
    let cwd = dir.path().join("nest");
    std::fs::create_dir_all(&cwd).unwrap();
    let session = "00000000-0000-4000-8000-000000000004";
    create_claude_session(&cwd, session).unwrap();
    let path = session_path(&cwd, session);

    graft(&prior_of(&delivered(blooms())), &path, session, &cwd, None).unwrap();
    let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    use std::io::Write;
    for entry in [
        json!({"type": "assistant", "uuid": "s-1", "message": {"role": "assistant", "model": "<synthetic>", "content": [{"type": "text", "text": "No response requested."}]}}),
        json!({"type": "user", "uuid": "s-2", "message": {"role": "user", "content": claude_cli::stream::CONTINUE}}),
        json!({"type": "assistant", "uuid": "s-3", "message": {"role": "assistant", "content": [{"type": "text", "text": "Job 17 is done."}]}}),
    ] {
        writeln!(file, "{entry}").unwrap();
    }
    let before = lines_of(&path).len();
    let mut grown = delivered(blooms());
    grown.last_mut().unwrap().grew.as_mut().unwrap().push(petal(json!({"kind": "text", "text": "Job 17 is done."})));
    let held = graft(&prior_of(&grown), &path, session, &cwd, None).unwrap();
    assert_eq!(held.grafted, 0, "the transcript holds every turn");
    assert_eq!(lines_of(&path).len(), before);

    grown.push(Bloom {
        content: Some(vec![petal(json!({"kind": "text", "text": "and now?"}))]),
        grew: Some(vec![petal(json!({"kind": "text", "text": "Now this."}))]),
    });
    let next = graft(&prior_of(&grown), &path, session, &cwd, None).unwrap();
    assert_eq!(next.grafted, 1, "a turn grown elsewhere is still stitched");
    assert_eq!(lines_of(&path).last().unwrap()["message"]["content"][0]["text"], "Now this.");
}
