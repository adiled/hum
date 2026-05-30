//! Single source of truth for every on-disk path hum reads or writes.
//!
//! Call `init()` once at process startup before any other call here.
//! It sets any unset XDG env vars to HOME-relative defaults, so every
//! subsequent call in the process resolves without fallback logic.
//!
//! Layout follows the XDG Base Directory spec.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Set unset XDG env vars to HOME-relative defaults.
///
/// Must be called once at startup in humd, hum CLI, and every hive worker.
/// Panics if `HOME` is unset, which is always a configuration error.
pub fn init() {
    let home = home();
    xdg_default("XDG_STATE_HOME",  home.join(".local/state"));
    xdg_default("XDG_CONFIG_HOME", home.join(".config"));
    xdg_default("XDG_DATA_HOME",   home.join(".local/share"));
    xdg_default("XDG_CACHE_HOME",  home.join(".cache"));
    xdg_default("XDG_RUNTIME_DIR", home.join(".local/state/run"));
}

fn xdg_default(var: &str, default: PathBuf) {
    if std::env::var_os(var).is_none() {
        // Safety: single-threaded startup; no other threads reading env yet.
        unsafe { std::env::set_var(var, default); }
    }
}

// ── Directory roots ──────────────────────────────────────────────────────────

/// `$XDG_STATE_HOME/hum` — persistent state: keys, snapshots, drift rings,
/// the thrum socket, and the rendezvous file.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME").join("hum")
}

/// `$XDG_CONFIG_HOME/hum` — user-editable config: hum.json, peers.json.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME").join("hum")
}

/// `$XDG_DATA_HOME/hum` — installed source clone, recipes.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME").join("hum")
}

/// `$XDG_CACHE_HOME/hum` — derived caches (e.g. foreign hive clones).
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME").join("hum")
}

/// `$XDG_RUNTIME_DIR/hum` — non-essential per-boot runtime files.
pub fn runtime_dir() -> PathBuf {
    xdg("XDG_RUNTIME_DIR").join("hum")
}

// ── Named files ──────────────────────────────────────────────────────────────

/// Default thrum socket path. The path humd would BIND if nothing
/// overrides it. Use this from the daemon; clients should call
/// [`thrum_sock_resolved`] instead so they honor whatever path humd
/// actually published in `runtime.json`.
pub fn thrum_sock() -> PathBuf {
    if let Some(p) = std::env::var_os("HUM_THRUM_SOCK") { return PathBuf::from(p); }
    if let Some(p) = std::env::var_os("HUM_SOCKET")     { return PathBuf::from(p); }
    state_dir().join("thrum.sock")
}

/// What clients (bees, CLI) should connect to. Honors humd's
/// rendezvous file first, then env overrides, then the default.
pub fn thrum_sock_resolved() -> PathBuf {
    if let Some(p) = std::env::var_os("HUM_THRUM_SOCK") { return PathBuf::from(p); }
    if let Some(p) = std::env::var_os("HUM_SOCKET")     { return PathBuf::from(p); }
    if let Some(rt) = RuntimeInfo::read() { return rt.socket; }
    state_dir().join("thrum.sock")
}

/// humd HTTP control socket.
pub fn http_sock() -> PathBuf { runtime_dir().join("hum.sock.http") }

/// Penny lifetime counters.
pub fn penny() -> PathBuf { runtime_dir().join("penny.json") }

/// humd ed25519 identity seed.
pub fn humd_key() -> PathBuf { state_dir().join("humd.key") }

/// Directory holding per-bee ed25519 identity seeds.
pub fn bees_dir() -> PathBuf { state_dir().join("bees") }

/// Per-bee ed25519 identity seed; one file per hive kind.
pub fn bee_key(kind: &str) -> PathBuf {
    bees_dir().join(format!("{kind}.key"))
}

/// Live bee manifest snapshot (written by daemon on every register/disconnect).
pub fn bees_snapshot() -> PathBuf { state_dir().join("bees.json") }

/// Rendezvous file: running daemon publishes its socket path, pid, and version here.
pub fn runtime_info() -> PathBuf { state_dir().join("runtime.json") }

pub fn humnest_sock() -> PathBuf { state_dir().join("humnest.sock") }
pub fn humnest_runtime() -> PathBuf { state_dir().join("humnest_runtime.json") }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub socket: PathBuf,
    pub pid: u32,
    pub version: String,
    pub thrum_version: String,
    pub bound_at_ms: u64,
    #[serde(default)]
    pub ensemble_addrs: Vec<String>,
}

impl RuntimeInfo {
    pub fn read() -> Option<Self> {
        let raw = std::fs::read_to_string(runtime_info()).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn write(&self) -> std::io::Result<()> {
        let path = runtime_info();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, path)
    }

    pub fn remove() {
        let _ = std::fs::remove_file(runtime_info());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumnestRuntimeInfo {
    pub socket: PathBuf,
    pub pid: u32,
    pub version: String,
    pub bound_at_ms: u64,
}

impl HumnestRuntimeInfo {
    pub fn read() -> Option<Self> {
        let raw = std::fs::read_to_string(humnest_runtime()).ok()?;
        serde_json::from_str(&raw).ok()
    }

    pub fn write(&self) -> std::io::Result<()> {
        let path = humnest_runtime();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, path)
    }

    pub fn remove() {
        let _ = std::fs::remove_file(humnest_runtime());
    }
}

/// `hum.json` (daemon policy).
pub fn hum_json() -> PathBuf { config_dir().join("hum.json") }

/// `peers.json` (ensemble peer list).
pub fn peers_json() -> PathBuf { config_dir().join("peers.json") }

/// Drift rings directory (`drift/YYYY-MM-DD.ndjson`).
pub fn drift_dir() -> PathBuf { state_dir().join("drift") }

/// Cloned hum source tree (recipes + hive installers).
pub fn src_dir() -> PathBuf { data_dir().join("src") }

/// Installed humd binary location.
pub fn humd_bin() -> PathBuf {
    home().join(".local/bin/humd")
}

/// hums.json (session registry).
pub fn hums_json() -> PathBuf { state_dir().join("hums.json") }

/// Per-bee config file for a given hive kind (e.g. `ollama-server.json`).
pub fn bee_config(kind: &str) -> PathBuf {
    config_dir().join("bees").join(format!("{kind}.json"))
}

/// Where a hum daemon's logs live, by platform.
pub enum DaemonLogs {
    Journald { unit: String },
    Files { stdout: PathBuf, stderr: PathBuf },
}

pub fn daemon_logs(name: &str) -> DaemonLogs {
    #[cfg(target_os = "macos")]
    {
        let base = home().join("Library/Logs");
        return DaemonLogs::Files {
            stdout: base.join(format!("sh.hum.{name}.out.log")),
            stderr: base.join(format!("sh.hum.{name}.err.log")),
        };
    }
    #[cfg(not(target_os = "macos"))]
    DaemonLogs::Journald { unit: name.to_string() }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn xdg(var: &str) -> PathBuf {
    if let Some(v) = std::env::var_os(var) {
        return PathBuf::from(v);
    }
    init();
    PathBuf::from(std::env::var_os(var).expect("init() set the var"))
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must be set")
}
