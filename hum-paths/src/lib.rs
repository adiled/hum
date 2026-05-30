//! Single source of truth for every on-disk path hum reads or writes.
//!
//! Call `init()` once at process startup before any other call here.
//! It sets any unset XDG env vars to HOME-relative defaults, so every
//! subsequent call in the process resolves without fallback logic.
//!
//! Layout follows the XDG Base Directory spec.

use std::path::PathBuf;

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

/// Thrum socket: `$XDG_STATE_HOME/hum/thrum.sock`.
/// Respects `HUM_THRUM_SOCK` and the legacy `HUM_SOCKET` override.
pub fn thrum_sock() -> PathBuf {
    if let Some(p) = std::env::var_os("HUM_THRUM_SOCK") { return PathBuf::from(p); }
    if let Some(p) = std::env::var_os("HUM_SOCKET")     { return PathBuf::from(p); }
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

/// `hum.json` (daemon policy).
pub fn hum_json() -> PathBuf { config_dir().join("hum.json") }

/// `peers.json` (ensemble peer list).
pub fn peers_json() -> PathBuf { config_dir().join("peers.json") }

/// Drift rings directory (`drift/YYYY-MM-DD.ndjson`).
pub fn drift_dir() -> PathBuf { state_dir().join("drift") }

/// Cloned hum source tree (recipes + svc.sh helpers).
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

/// macOS log paths for a launchd unit short id (e.g. `"hum"`, `"hum-claude-cli-worker"`).
/// Returns `(stdout, stderr)`.
pub fn macos_log(unit: &str) -> (PathBuf, PathBuf) {
    let base = home().join("Library/Logs");
    (
        base.join(format!("sh.hum.{unit}.out.log")),
        base.join(format!("sh.hum.{unit}.err.log")),
    )
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn xdg(var: &str) -> PathBuf {
    PathBuf::from(
        std::env::var_os(var)
            .unwrap_or_else(|| panic!("{var} not set — call hum_paths::init() at process startup")),
    )
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must be set")
}
