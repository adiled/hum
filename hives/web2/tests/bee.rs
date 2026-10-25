//! The bee end to end: a humd here, `web2 serve` as its attendant, a nurse
//! that reaches whatever its bloom asks through `wrepl`, and a forager that
//! asks. What the forager reads back is what wrepl answered, so the line
//! went forager → humd → nurse → humd → web2 → an MCP server, and back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hum_thrum::{Bee, Event, Options};
use parking_lot::Mutex;
use serde_json::json;
use thrum_core::Role;

const WAIT: Duration = Duration::from_secs(60);

fn mock_mcp() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let path = exe.parent().unwrap().parent().unwrap().join("examples").join("mock_mcp");
    assert!(path.exists(), "build the mock first: cargo build --example mock_mcp ({})", path.display());
    path
}

fn start_humd() -> tokio::sync::oneshot::Sender<()> {
    let sock = hum_paths::thrum_sock_resolved();
    let _ = std::fs::remove_file(&sock);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let mut cfg = humd::DaemonConfig::from_env();
            cfg.humd_key = None;
            let _ = humd::run(cfg, async {
                let _ = rx.await;
            })
            .await;
        });
        rt.shutdown_background();
    });
    let since = Instant::now();
    while std::os::unix::net::UnixStream::connect(&sock).is_err() {
        assert!(since.elapsed() < WAIT, "humd did not bind {}", sock.display());
        std::thread::sleep(Duration::from_millis(50));
    }
    tx
}

fn nurse(mut bee: Bee, seen_tools: Arc<Mutex<Vec<String>>>) {
    tokio::spawn(async move {
        let mut pending: HashMap<String, String> = HashMap::new();
        let mut n = 0u32;
        while let Some(event) = bee.next().await {
            let Event::Tone(tone) = event else { continue };
            let sid = tone["sid"].as_str().unwrap_or("").to_string();
            match tone["chi"].as_str() {
                Some("prompt") => {
                    let names: Vec<String> = tone["tools"]
                        .as_array()
                        .map(|t| t.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    *seen_tools.lock() = names;
                    n += 1;
                    let call_id = format!("call-{n}");
                    pending.insert(call_id.clone(), sid.clone());
                    let line = tone["text"].as_str().unwrap_or("").to_string();
                    bee.send(json!({ "chi": "tendril-reach", "sid": sid, "callId": call_id, "name": "wrepl", "args": { "line": line } }));
                }
                Some("tendril-result") => {
                    let Some(sid) = tone["callId"].as_str().and_then(|c| pending.remove(c)) else { continue };
                    let mut text = tone["output"].as_str().unwrap_or("").to_string();
                    if tone["isError"] == true {
                        text = format!("error: {text}");
                    }
                    bee.send(json!({ "chi": "unfurl", "sid": sid, "petal": 0, "kind": "text", "text": text }));
                    bee.send(json!({ "chi": "wilt", "sid": sid, "reason": "stop" }));
                }
                _ => {}
            }
        }
    });
}

async fn ask(forager: &mut Bee, n: &mut u32, line: &str) -> String {
    *n += 1;
    let sid = format!("web2-bee-{n}");
    forager.send(json!({ "chi": "prompt", "sid": sid, "modelId": "mock-model", "text": line }));
    let mut bloom = String::new();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let event = tokio::time::timeout_at(deadline, forager.next())
            .await
            .unwrap_or_else(|_| panic!("no answer to {line:?} within {WAIT:?}"))
            .expect("the forager's client stopped");
        let Event::Tone(tone) = event else { continue };
        if tone["sid"] != sid {
            continue;
        }
        match tone["chi"].as_str() {
            Some("unfurl") => bloom.push_str(tone["text"].as_str().unwrap_or("")),
            Some("wilt") | Some("error") => return bloom,
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bloom_reaches_wrepl_through_humd() {
    let tmp = tempfile::Builder::new().prefix("w2").tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("XDG_STATE_HOME", root.join("s"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("c"));
        std::env::set_var("XDG_DATA_HOME", root.join("d"));
        std::env::set_var("XDG_CACHE_HOME", root.join("k"));
        std::env::set_var("XDG_RUNTIME_DIR", root.join("r"));
        std::env::set_var("HUM_LOG_LEVEL", "warn");
        std::env::set_var("WEB2_CONFIG", root.join("web2.json"));
        std::env::set_var("WEB2_STATE", root.join("web2-state"));
        std::env::remove_var("HUM_THRUM_SOCK");
        std::env::remove_var("HUM_SOCKET");
    }
    std::fs::create_dir_all(hum_paths::hives_d_dir()).unwrap();
    std::fs::write(hum_paths::hum_json(), r#"{"humd":{"metricsAddr":"none","irohPort":0}}"#).unwrap();
    for hive in ["web2", "forager", "nurse-mock"] {
        std::fs::write(hum_paths::hive_installation(hive), "# web2's bee test\n").unwrap();
    }
    std::fs::write(root.join("web2.json"), json!({ "mcpServers": { "mock": { "command": mock_mcp() } } }).to_string()).unwrap();

    let _humd = start_humd();
    let log = std::fs::File::create(root.join("web2-serve.log")).unwrap();
    let _web2 = tokio::process::Command::new(env!("CARGO_BIN_EXE_web2"))
        .arg("serve")
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let seen_tools = Arc::new(Mutex::new(Vec::new()));
    let nurse_bee = Bee::link(Options::new("nurse-mock", "main", Role::Nurse).hello(json!({
        "version": "1.0.0",
        "models": ["mock-model"],
        "headroom": { "freeSlots": 4, "totalSlots": 4 },
    })))
    .await
    .expect("the nurse nestles");
    nurse(nurse_bee, seen_tools.clone());
    let mut forager = Bee::link(Options::new("forager", "main", Role::Forager)).await.expect("the forager nestles");
    let mut n = 0;

    let since = Instant::now();
    loop {
        let answer = ask(&mut forager, &mut n, "mock echo 'through the hum'").await;
        if answer == "through the hum" {
            break;
        }
        let log = std::fs::read_to_string(root.join("web2-serve.log")).unwrap_or_default();
        assert!(since.elapsed() < WAIT, "wrepl never answered; last answer {answer:?}\nweb2 serve:\n{log}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(seen_tools.lock().contains(&"wrepl".to_string()), "the bloom was handed {:?}", seen_tools.lock());

    assert_eq!(ask(&mut forager, &mut n, "mock add 2 3 --jq .sum").await, "5");
    let refused = ask(&mut forager, &mut n, "mock create-note --title x").await;
    assert!(refused.starts_with("error: ") && refused.contains("only reads here"), "{refused}");
    assert!(ask(&mut forager, &mut n, "help").await.contains("mock"));

    std::fs::write(
        root.join("web2.json"),
        json!({ "mcpServers": { "mock": { "command": mock_mcp() } }, "services": { "mock": { "writes": true } } }).to_string(),
    )
    .unwrap();
    assert_eq!(ask(&mut forager, &mut n, "mock create-note --title x").await, "created x");
}
