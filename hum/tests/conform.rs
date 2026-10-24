//! One client, five languages: the conformance run.
//!
//! The thrum client is hum-thrum's, and every language reaches it the
//! same way, so every language must behave the same way. This run holds
//! them to it against a real humd and the real link: an echo bee in Rust
//! (in this process), TS, Python, Go and C, each on its own client, and
//! for every one of them:
//!
//! - it nestles;
//! - a prompt from another bee reaches it through humd, and its answer
//!   comes back;
//! - a session it follows by sid gets its next tone (a cancel) before
//!   any chi handler does;
//! - a tone it sends with a key the thrum does not declare still goes,
//!   the bee hears an advisory, and humd records one against it;
//! - humd restarting under it is a `lost` and then a `nestled`, and
//!   prompts reach it again;
//! - closing it ends the link cleanly;
//! - a bee of a hive this humd does not have is refused, and its client
//!   says so and exits 3.
//!
//! A language whose toolchain is missing is skipped with a note, unless
//! `THRUM_CONFORM_REQUIRE` names it (CI names them all). The echo bees
//! are `thrum-clients/<lang>/examples`. The run starts `hum` once before
//! it times anything: macOS assesses a freshly linked binary on its first
//! launch, which can take longer than a bee is given to nestle.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hum_thrum::{Bee, Event, Options, Refused};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

struct Humd {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Humd {
    fn start() -> Self {
        let sock = hum_paths::thrum_sock_resolved();
        let _ = std::fs::remove_file(&sock);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut cfg = humd::DaemonConfig::from_env();
                cfg.bind_mcp = false;
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
            if since.elapsed() > WAIT {
                let log = std::fs::read_to_string(sock.parent().unwrap().join("../../logs/humd.log")).unwrap_or_default();
                panic!("humd did not bind {}:\n{}", sock.display(), log.lines().rev().take(15).collect::<Vec<_>>().join("\n"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Self { stop: Some(tx), thread: Some(thread) }
    }

    fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Humd {
    fn drop(&mut self) {
        self.stop();
    }
}

enum Runner {
    Process { child: Child, stdin: Option<ChildStdin> },
    Inline { sender: hum_thrum::Sender },
}

struct EchoBee {
    lang: &'static str,
    model: String,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    runner: Runner,
}

impl EchoBee {
    fn expect(&mut self, want: &str) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if line.starts_with(want) {
                        return line;
                    }
                }
                Err(_) => panic!("{}: no `{want}` within {WAIT:?}; saw {:?}", self.lang, self.seen),
            }
        }
    }

    fn close(mut self) -> Option<i32> {
        match &mut self.runner {
            Runner::Process { stdin, .. } => drop(stdin.take()),
            Runner::Inline { sender } => sender.close(),
        }
        self.expect("closed");
        self.exit_code()
    }

    fn exit_code(&mut self) -> Option<i32> {
        match &mut self.runner {
            Runner::Process { child, .. } => {
                let since = Instant::now();
                loop {
                    if let Ok(Some(status)) = child.try_wait() {
                        return status.code();
                    }
                    if since.elapsed() > WAIT {
                        let _ = child.kill();
                        panic!("{}: the bee did not exit", self.lang);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            Runner::Inline { .. } => Some(0),
        }
    }
}

struct Lang {
    name: &'static str,
    command: Vec<String>,
}

fn have(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn languages(root: &Path) -> Vec<Lang> {
    let clients = repo().join("thrum-clients");
    let required: Vec<String> = std::env::var("THRUM_CONFORM_REQUIRE")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = Vec::new();
    let mut found = |name: &'static str, command: Option<Vec<String>>, why: &str| match command {
        Some(command) => out.push(Lang { name, command }),
        None if required.iter().any(|r| r == name) => panic!("{name} is required but {why}"),
        None => eprintln!("conform: skipping {name}: {why}"),
    };

    let node = have("node", &["--version"]).and_then(|v| {
        let major: u32 = v.trim_start_matches('v').split('.').next()?.parse().ok()?;
        (major >= 22).then_some(major)
    });
    found(
        "ts",
        node.map(|major| {
            let mut c = vec!["node".to_string()];
            if major < 23 {
                c.push("--experimental-strip-types".into());
            }
            c.push(clients.join("ts/examples/echo_bee.ts").display().to_string());
            c
        }),
        "node 22 or later is not on PATH",
    );

    found(
        "py",
        have("python3", &["-c", "import sys; assert sys.version_info >= (3, 9)"]).map(|_| {
            vec!["python3".into(), clients.join("python/examples/echo_bee.py").display().to_string()]
        }),
        "python 3.9 or later is not on PATH",
    );

    let go_bee = root.join("echo-go");
    let go_built = have("go", &["version"]).is_some()
        && Command::new("go")
            .args(["build", "-o"])
            .arg(&go_bee)
            .arg("./examples/echo_bee")
            .current_dir(clients.join("go"))
            .status()
            .is_ok_and(|s| s.success());
    found("go", go_built.then(|| vec![go_bee.display().to_string()]), "go could not build the echo bee");

    let c_built = Command::new("make")
        .args(["-s", "out/echo_bee"])
        .current_dir(clients.join("c"))
        .status()
        .is_ok_and(|s| s.success());
    found(
        "c",
        c_built.then(|| vec![clients.join("c/out/echo_bee").display().to_string()]),
        "make could not build the echo bee (cc, pkg-config and cJSON)",
    );
    out
}

fn spawn_process(lang: &'static str, command: &[String], hive: &str, logs: &Path) -> EchoBee {
    let model = format!("echo-{lang}");
    let err = std::fs::File::create(logs.join(format!("{lang}-{hive}.err"))).unwrap();
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .env("ECHO_HIVE", hive)
        .env("ECHO_MODEL", &model)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(err)
        .spawn()
        .unwrap_or_else(|e| panic!("{lang}: {e}"));
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    EchoBee { lang, model, lines: rx, seen: Vec::new(), runner: Runner::Process { child, stdin } }
}

async fn spawn_inline(hive: &str) -> EchoBee {
    let model = "echo-rs".to_string();
    let (tx, rx) = mpsc::channel();
    let opts = Options::new(hive, "main").hello(json!({
        "roles": ["worker"],
        "version": "1.0.0",
        "models": [model],
        "headroom": {"freeSlots": 1, "totalSlots": 1},
    }));
    let key = hum_identity::enter_as(hive, "main").expect("the rust echo bee enters");
    let mut bee = Bee::spawn(opts, key);
    let sender = bee.sender();
    tokio::spawn(async move {
        let mut followed: Option<Value> = None;
        while let Some(event) = bee.next().await {
            let line = match event {
                Event::Nestled => "nestled".to_string(),
                Event::Lost => "lost".to_string(),
                Event::Advisory(a) => format!("advisory {}", a.code),
                Event::Refused(why) => format!("refused {why}"),
                Event::Tone(tone) if followed.is_some() && tone["sid"] == *followed.as_ref().unwrap() => {
                    followed = None;
                    format!("sid {}", tone["chi"].as_str().unwrap_or(""))
                }
                Event::Tone(tone) => {
                    if tone["chi"] == "prompt" {
                        let text = tone["text"].as_str().unwrap_or("").to_string();
                        let sid = tone["sid"].clone();
                        if text == "follow" {
                            followed = Some(sid.clone());
                        }
                        let mut chunk = json!({"chi": "chunk", "sid": sid, "chunkType": "text_delta", "delta": format!("echo: {text}")});
                        if text == "stray" {
                            chunk["stray"] = json!(true);
                        }
                        bee.send(chunk);
                        bee.send(json!({"chi": "finish", "sid": sid, "finishReason": "stop"}));
                    }
                    continue;
                }
            };
            if tx.send(line).is_err() {
                return;
            }
        }
        let _ = tx.send("closed".to_string());
    });
    EchoBee { lang: "rs", model, lines: rx, seen: Vec::new(), runner: Runner::Inline { sender } }
}

struct Asker {
    bee: Bee,
    asked: u32,
}

impl Asker {
    async fn ask(&mut self, model: &str, text: &str) -> String {
        self.asked += 1;
        let sid = format!("conform-{model}-{}", self.asked);
        self.bee.send(json!({"chi": "prompt", "sid": sid, "modelId": model, "text": text}));
        let mut delta = String::new();
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let event = tokio::time::timeout_at(deadline, self.bee.next())
                .await
                .unwrap_or_else(|_| panic!("{model}: no answer to {text:?} within {WAIT:?}"))
                .expect("the asker's client stopped");
            let Event::Tone(tone) = event else { continue };
            if tone["sid"] != sid {
                continue;
            }
            match tone["chi"].as_str() {
                Some("chunk") => delta.push_str(tone["delta"].as_str().unwrap_or("")),
                Some("finish") => return delta,
                _ => {}
            }
        }
    }

    fn cancel_last(&self, model: &str) {
        let sid = format!("conform-{model}-{}", self.asked);
        self.bee.send(json!({"chi": "cancel", "sid": sid}));
    }

    async fn nestled_again(&mut self) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout_at(deadline, self.bee.next()).await {
                Ok(Some(Event::Nestled)) => return,
                Ok(Some(_)) => continue,
                _ => panic!("the asker did not nestle again"),
            }
        }
    }
}

fn advised(bee: &str) -> Vec<String> {
    let book: Value = std::fs::read(hum_paths::advisories())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    book[bee]["advisories"]
        .as_array()
        .map(|a| a.iter().filter_map(|e| e["code"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_language_is_the_same_bee() {
    let tmp = tempfile::Builder::new().prefix("hc").tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let logs = root.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let humd_log = std::fs::File::create(logs.join("humd.log")).unwrap();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_writer(std::sync::Mutex::new(humd_log))
        .with_ansi(false)
        .try_init();
    unsafe {
        std::env::set_var("XDG_STATE_HOME", root.join("s"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("c"));
        std::env::set_var("XDG_DATA_HOME", root.join("d"));
        std::env::set_var("XDG_CACHE_HOME", root.join("k"));
        std::env::set_var("XDG_RUNTIME_DIR", root.join("r"));
        std::env::set_var("HUM_CLI", env!("CARGO_BIN_EXE_hum"));
        std::env::set_var("HUM_MCP_PORT", "0");
        std::env::set_var("HUM_LOG_LEVEL", "warn,hum_thrum=info");
        std::env::remove_var("HUM_THRUM_SOCK");
        std::env::remove_var("HUM_SOCKET");
    }
    std::fs::create_dir_all(hum_paths::hives_d_dir()).unwrap();
    std::fs::write(hum_paths::hum_json(), r#"{"humd":{"metricsAddr":"none","irohPort":0}}"#).unwrap();
    for hive in ["asker", "echo-rs", "echo-ts", "echo-py", "echo-go", "echo-c"] {
        std::fs::write(hum_paths::hive_installation(hive), "# conformance\n").unwrap();
    }

    let langs = languages(&root);
    let warm = Command::new(env!("CARGO_BIN_EXE_hum")).arg("--version").output();
    assert!(warm.is_ok_and(|o| o.status.success()), "hum does not run");
    let mut humd = Humd::start();
    let asker_bee = Bee::link(Options::new("asker", "main").hello(json!({"roles": ["asker"]})))
        .await
        .expect("the asker nestles");
    let mut asker = Asker { bee: asker_bee, asked: 0 };

    let mut bees = vec![spawn_inline("echo-rs").await];
    for lang in &langs {
        bees.push(spawn_process(lang.name, &lang.command, &format!("echo-{}", lang.name), &logs));
    }
    let names: Vec<_> = bees.iter().map(|b| b.lang).collect();
    eprintln!("conform: {}", names.join(", "));

    for bee in &mut bees {
        bee.expect("nestled");
        assert_eq!(asker.ask(&bee.model, "hi").await, "echo: hi", "{}", bee.lang);
        assert_eq!(asker.ask(&bee.model, "stray").await, "echo: stray", "{}", bee.lang);
        bee.expect("advisory undeclared-key");
        assert_eq!(asker.ask(&bee.model, "follow").await, "echo: follow", "{}", bee.lang);
        asker.cancel_last(&bee.model);
        bee.expect("sid cancel");
    }
    for bee in &bees {
        let id = hum_identity::enter_as(&format!("echo-{}", bee.lang), "main").unwrap().id();
        let deadline = Instant::now() + WAIT;
        while !advised(&id).contains(&"undeclared-key".to_string()) {
            assert!(Instant::now() < deadline, "{}: humd recorded no advisory", bee.lang);
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    humd.stop();
    let mut humd = Humd::start();
    asker.nestled_again().await;
    for bee in &mut bees {
        bee.expect("lost");
        bee.expect("nestled");
        assert_eq!(asker.ask(&bee.model, "again").await, "echo: again", "{} after humd restarted", bee.lang);
    }

    for bee in bees {
        let lang = bee.lang;
        assert_eq!(bee.close(), Some(0), "{lang}: a closed bee exits 0");
    }

    let mut refused = vec![];
    let err = Bee::link(Options::new("echo-nope", "main")).await.err().expect("rs is refused");
    assert!(err.downcast_ref::<Refused>().is_some(), "rs: {err}");
    refused.push("rs");
    for lang in &langs {
        let mut bee = spawn_process(lang.name, &lang.command, "echo-nope", &logs);
        let line = bee.expect("refused");
        assert!(line.contains("not installed"), "{}: {line}", lang.name);
        assert_eq!(bee.exit_code(), Some(3), "{}: a refused bee exits 3", lang.name);
        refused.push(lang.name);
    }
    eprintln!("conform: refused cleanly: {}", refused.join(", "));

    asker.bee.close();
    humd.stop();
}
