//! The ensemble around the forager: the humd in the test's own process is
//! the far one, with the nurse; the near one, where the forager lives and
//! no nurse does, runs as its own process under roots of its own and
//! reaches the far one over TCP.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::json;

use super::{world, Forager, WAIT};

const ROOTS: [(&str, &str); 5] = [
    ("XDG_STATE_HOME", "s"),
    ("XDG_CONFIG_HOME", "c"),
    ("XDG_DATA_HOME", "d"),
    ("XDG_CACHE_HOME", "k"),
    ("XDG_RUNTIME_DIR", "r"),
];

pub struct Near {
    _child: tokio::process::Child,
    root: PathBuf,
    log: PathBuf,
}

impl Near {
    pub fn env(&self) -> Vec<(String, String)> {
        ROOTS.iter().map(|(k, v)| (k.to_string(), self.root.join(v).display().to_string())).collect()
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn under(root: &Path, p: PathBuf) -> PathBuf {
    root.join(p.strip_prefix(&world().root).expect("a hum path is under the world's root"))
}

fn humd_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let path = exe.parent().unwrap().parent().unwrap().join("examples").join("humd");
    assert!(path.exists(), "build it first: cargo build --example humd ({})", path.display());
    path
}

pub fn far_reach() -> (String, String) {
    let since = Instant::now();
    loop {
        if let Some(info) = hum_paths::RuntimeInfo::read()
            && let Some(tcp) = info.ensemble_addrs.iter().find(|a| a.starts_with("tcp:"))
        {
            return (humd::load_or_mint_key().unwrap().hid().to_hex(), tcp.clone());
        }
        assert!(since.elapsed() < WAIT, "the far humd published no tcp address");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub async fn spawn_near(far_hid: &str, far_tcp: &str) -> Near {
    let root = world().root.join("near");
    let at = |p: PathBuf| under(&root, p);
    std::fs::create_dir_all(at(hum_paths::hives_d_dir())).unwrap();
    let humd_cfg = json!({"humd": {"metricsAddr": "none", "irohPort": 0, "trustRemoteNurses": true}});
    std::fs::write(at(hum_paths::hum_json()), humd_cfg.to_string()).unwrap();
    let peers = json!({"peers": [{"humd_id": far_hid, "hints": [far_tcp]}]});
    std::fs::write(at(hum_paths::peers_json()), peers.to_string()).unwrap();
    for hive in ["bp7", "nurse-echo"] {
        std::fs::write(at(hum_paths::hive_installation(hive)), "# the near humd recognises it\n").unwrap();
    }
    let log = world().root.join("near-humd.log");
    let mut cmd = tokio::process::Command::new(humd_bin());
    for (k, v) in ROOTS {
        cmd.env(k, root.join(v));
    }
    cmd.env("HUM_LOG_LEVEL", "info")
        .env_remove("HUM_THRUM_SOCK")
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let near = Near { _child: cmd.spawn().unwrap(), root, log };
    let sock = under(&near.root, hum_paths::thrum_sock_resolved());
    let since = Instant::now();
    while std::os::unix::net::UnixStream::connect(&sock).is_err() {
        assert!(since.elapsed() < WAIT, "the near humd did not bind {}:\n{}", sock.display(), near.log_text());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    near
}

pub async fn forager_on(near: &Near, name: &str) -> Forager {
    let env = near.env();
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    Forager::start(name, &env).await
}
