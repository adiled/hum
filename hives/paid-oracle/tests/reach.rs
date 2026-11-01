//! The oracle as orchd runs it, on a humd in a root of its own, reading a
//! chain and a price source on loopback: a nurse and a forager reach
//! `quote`, and every reach is answered under its `callId`.

mod chain;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use chain::*;
use hum_thrum::{Bee, Event, Options};
use serde_json::{json, Value};
use thrum_core::Role;

const WAIT: Duration = Duration::from_secs(30);
const PAY_TO: &str = "0x1111111111111111111111111111111111111111";
const PRICE: u128 = 50_000_000_000_000_000;

struct World {
    root: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl World {
    fn new() -> World {
        let root = tempfile::Builder::new().prefix("paid-oracle").tempdir().unwrap().keep();
        unsafe {
            std::env::set_var("XDG_STATE_HOME", root.join("s"));
            std::env::set_var("XDG_CONFIG_HOME", root.join("c"));
            std::env::set_var("XDG_DATA_HOME", root.join("d"));
            std::env::set_var("XDG_CACHE_HOME", root.join("k"));
            std::env::set_var("XDG_RUNTIME_DIR", root.join("r"));
            std::env::set_var("HUM_LOG_LEVEL", "warn");
            std::env::remove_var("HUM_THRUM_SOCK");
            std::env::remove_var("HUM_SOCKET");
        }
        std::fs::create_dir_all(hum_paths::hives_d_dir()).unwrap();
        std::fs::write(hum_paths::hum_json(), json!({"humd": {"metricsAddr": "none", "irohPort": 0}}).to_string()).unwrap();
        for hive in ["paid-oracle", "nurse-test"] {
            std::fs::write(hum_paths::hive_installation(hive), "# paid-oracle's test\n").unwrap();
        }
        let sock = hum_paths::thrum_sock_resolved();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
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
        World { root, stop: Some(tx), thread: Some(thread) }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Oracle {
    child: tokio::process::Child,
    log: PathBuf,
}

impl Oracle {
    /// Started as its Orchfile starts it: every setting passed, the unset ones empty.
    async fn start(w: &World, chain: &Chain, life: u32) -> Oracle {
        let log = w.root.join(format!("oracle-{life}.log"));
        let out = std::fs::File::create(&log).unwrap();
        let orchfile = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Orchfile")).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_paid-oracle"));
        for name in orchfile.lines().filter_map(|l| l.strip_prefix("ENV ")).filter_map(|l| l.split_once('=')).map(|(k, _)| k) {
            cmd.env(name, "");
        }
        cmd.env("PAID_ORACLE_PAY_TO", PAY_TO)
            .env("PAID_ORACLE_RPC", chain.rpc_url())
            .env("PAID_ORACLE_PRICE_URL", chain.price_url())
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .kill_on_drop(true);
        let mut oracle = Oracle { child: cmd.spawn().unwrap(), log };
        let since = Instant::now();
        while !oracle.log_text().contains("thrum.nestled") {
            if let Ok(Some(status)) = oracle.child.try_wait() {
                panic!("the oracle exited {status}:\n{}", oracle.log_text());
            }
            assert!(since.elapsed() < WAIT, "the oracle did not nestle:\n{}", oracle.log_text());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        oracle
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

async fn bee(hive: &str, label: &str, role: Role) -> Bee {
    let key = hum_identity::enter_as(hive, label, role.as_str()).unwrap();
    let hello = match role {
        Role::Nurse => json!({"version": "1.0.0", "models": ["test"], "headroom": {"freeSlots": 1, "totalSlots": 1}}),
        _ => json!({"version": "1.0.0"}),
    };
    Bee::link_as(Options::new(hive, label, role).hello(hello), key).await.unwrap()
}

/// What answers the reach: the first tone carrying its `callId`.
async fn reach(bee: &mut Bee, sid: &str, call_id: &str, args: Value) -> Value {
    bee.send(json!({"chi": "tendril-reach", "sid": sid, "callId": call_id, "name": "quote", "args": args}));
    let mut heard = Vec::new();
    let answered = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = bee.next().await {
            if let Event::Tone(tone) = event {
                if tone["callId"] == call_id {
                    return tone;
                }
                heard.push(tone);
            }
        }
        panic!("the bee stopped");
    })
    .await;
    answered.unwrap_or_else(|_| panic!("no answer under {call_id}; heard {heard:?}"))
}

fn paid(chain: &Chain, hash: &str, nonce: &str) {
    chain.hold(hash, native(PAY_TO, PRICE, &input_with(nonce)), Some(receipt("0x1", 100, vec![])));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_reach_is_answered_under_its_call_id_and_a_payment_buys_one_quote() {
    let w = World::new();
    let chain = Chain::start().await;
    let oracle = Oracle::start(&w, &chain, 0).await;

    let mut nurse = bee("nurse-test", "main", Role::Nurse).await;
    let terms = reach(&mut nurse, "s-nurse", "c-nurse", json!({})).await;
    assert_eq!(terms["chi"], "tendril-result", "a nurse hears a tendril-result: {terms}");
    assert_eq!(terms["isError"], true);
    assert_eq!(terms["result"]["code"], "402");
    assert!(terms["output"].as_str().unwrap().contains("payment required"), "{terms}");

    let mut buyer = bee("paid-oracle", "buyer", Role::Forager).await;
    let terms = reach(&mut buyer, "s-buy", "c-1", json!({})).await;
    let x402 = &terms["result"]["x402"];
    assert_eq!(x402["pay_to"], PAY_TO);
    assert_eq!(x402["price_atomic"], PRICE.to_string());
    let nonce = x402["nonce"].as_str().unwrap().to_string();

    let tx = format!("0x{}", "ab".repeat(32));
    paid(&chain, &tx, &nonce);
    let proof = json!({"paymentProof": {"txHash": tx, "nonce": nonce}});
    let quote = reach(&mut buyer, "s-buy", "c-2", proof.clone()).await;
    assert_eq!(quote["chi"], "tendril-result");
    assert_eq!(quote["isError"], false, "{quote}");
    assert_eq!(quote["result"]["px"], "3456.78");

    let again = reach(&mut buyer, "s-buy", "c-3", proof.clone()).await;
    assert_eq!((again["chi"].as_str(), again["isError"].as_bool()), (Some("tendril-result"), Some(true)));
    assert_eq!(again["result"]["code"], "409", "{again}");

    let before = reach(&mut buyer, "s-buy", "c-4", json!({})).await["result"]["x402"]["nonce"]
        .as_str()
        .unwrap()
        .to_string();
    oracle.stop().await;
    let oracle = Oracle::start(&w, &chain, 1).await;

    let replay = reach(&mut buyer, "s-buy", "c-5", proof).await;
    assert_eq!(replay["result"]["code"], "409", "a restart forgets no redemption: {replay}");
    let tx2 = format!("0x{}", "cd".repeat(32));
    paid(&chain, &tx2, &before);
    let later = reach(&mut buyer, "s-buy", "c-6", json!({"paymentProof": {"txHash": tx2, "nonce": before}})).await;
    assert_eq!(later["isError"], false, "a nonce issued before a restart still buys a quote: {later}");
    oracle.stop().await;
}
