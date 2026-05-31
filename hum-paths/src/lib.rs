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

// ── Canonical file basenames ────────────────────────────────────────────────
// Single source of truth for every filename hum reads or writes. Tests and
// any code building paths under a non-default root must compose these
// against their own directory, NEVER hardcode the strings.

pub const THRUM_SOCK_BASENAME: &str = "thrum.sock";
pub const HTTP_SOCK_BASENAME: &str = "hum.sock.http";
pub const PENNY_BASENAME: &str = "penny.json";
pub const HUMD_KEY_BASENAME: &str = "humd.key";
pub const BEES_SNAPSHOT_BASENAME: &str = "bees.json";
pub const RUNTIME_INFO_BASENAME: &str = "runtime.json";
pub const HUM_JSON_BASENAME: &str = "hum.json";
pub const PEERS_JSON_BASENAME: &str = "peers.json";
pub const ORCHFILE_BASENAME: &str = "Orchfile";
/// Subdirectory of a hum source tree that holds hive crates.
pub const HIVES_SUBDIR: &str = "hives";
/// Subdirectory of a hum source tree that holds recipes (installable bundles).
pub const RECIPES_SUBDIR: &str = "recipes";
/// Per-hive install script name (found inside each hive crate root).
pub const HIVE_INSTALL_SCRIPT: &str = "install";

/// Claude CLI's per-cwd transcript dir, given a cwd hash.
/// Layout: `~/.claude/projects/<cwd_hash>/`.
pub fn claude_session_dir(cwd_hash: &str) -> PathBuf {
    claude_data_dir().join("projects").join(cwd_hash)
}

// ── Named files ──────────────────────────────────────────────────────────────

/// Default thrum socket path. The path humd would BIND if nothing
/// overrides it. Use this from the daemon; clients should call
/// [`thrum_sock_resolved`] instead so they honor whatever path humd
/// actually published in `runtime.json`.
pub fn thrum_sock() -> PathBuf {
    if let Some(p) = std::env::var_os("HUM_THRUM_SOCK") { return PathBuf::from(p); }
    if let Some(p) = std::env::var_os("HUM_SOCKET")     { return PathBuf::from(p); }
    state_dir().join(THRUM_SOCK_BASENAME)
}

/// What clients (bees, CLI) should connect to. Honors humd's
/// rendezvous file first, then env overrides, then the default.
pub fn thrum_sock_resolved() -> PathBuf {
    if let Some(p) = std::env::var_os("HUM_THRUM_SOCK") { return PathBuf::from(p); }
    if let Some(p) = std::env::var_os("HUM_SOCKET")     { return PathBuf::from(p); }
    if let Some(rt) = RuntimeInfo::read() { return rt.socket; }
    state_dir().join(THRUM_SOCK_BASENAME)
}

/// humd HTTP control socket.
pub fn http_sock() -> PathBuf { runtime_dir().join(HTTP_SOCK_BASENAME) }

/// Penny lifetime counters.
pub fn penny() -> PathBuf { runtime_dir().join(PENNY_BASENAME) }

/// humd ed25519 identity seed.
pub fn humd_key() -> PathBuf { state_dir().join(HUMD_KEY_BASENAME) }

/// Directory holding per-bee ed25519 identity seeds.
pub fn bees_dir() -> PathBuf { state_dir().join("bees") }

/// Per-bee ed25519 identity seed; one file per hive kind.
pub fn bee_key(kind: &str) -> PathBuf {
    bees_dir().join(format!("{kind}.key"))
}

/// Live bee manifest snapshot (written by daemon on every register/disconnect).
pub fn bees_snapshot() -> PathBuf { state_dir().join(BEES_SNAPSHOT_BASENAME) }

/// Rendezvous file: running daemon publishes its socket path, pid, and version here.
pub fn runtime_info() -> PathBuf { state_dir().join(RUNTIME_INFO_BASENAME) }

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

/// `hum.json` (daemon policy).
pub fn hum_json() -> PathBuf { config_dir().join(HUM_JSON_BASENAME) }

/// `peers.json` (ensemble peer list).
pub fn peers_json() -> PathBuf { config_dir().join(PEERS_JSON_BASENAME) }

/// Drift rings directory (`drift/YYYY-MM-DD.ndjson`).
pub fn drift_dir() -> PathBuf { state_dir().join("drift") }

/// thehum chi-log directory (`thehum/YYYY-MM-DD.ndjson` + seq.bin + snapshots/).
pub fn thehum_dir() -> PathBuf { state_dir().join("thehum") }

/// Cloned hum source tree (recipes + hive installers).
pub fn src_dir() -> PathBuf { data_dir().join("src") }

/// Helper script ship with hum's source clone — used by the CLI when
/// running cross-platform service operations (systemctl / launchctl wrap).
pub fn svc_script() -> PathBuf { src_dir().join("scripts/svc.sh") }

/// `$HOME/.local` — the base for HOME-anchored installs that aren't
/// resolved through XDG (cargo install --root, .local/bin, etc).
pub fn local_dir() -> PathBuf { home().join(".local") }

/// `$HOME/.local/bin` — where installed hum binaries live.
pub fn local_bin_dir() -> PathBuf { local_dir().join("bin") }

/// Installed humd binary location.
pub fn humd_bin() -> PathBuf { local_bin_dir().join("humd") }

/// fnm-managed node binary (when fnm is the user's node manager).
/// Probed as a fallback after PATH + /usr/local/bin/node.
pub fn fnm_node_bin() -> PathBuf { local_dir().join("share/fnm/aliases/default/bin/node") }

/// `~/.claude` — Claude CLI's data dir. Read by the claude-cli graft for
/// transcript replay; we don't write here.
pub fn claude_data_dir() -> PathBuf { home().join(".claude") }

/// Installed-binary location for a given hum binary name.
pub fn hum_bin(name: &str) -> PathBuf { local_bin_dir().join(name) }

/// Cache dir for a foreign hive clone (org/repo/branch).
pub fn foreign_hive_cache(org: &str, repo: &str, branch: &str) -> PathBuf {
    cache_dir().join("hives").join(format!("{org}-{repo}-{branch}"))
}

/// orch.d directory — one .orch file per registered hive.
pub fn orch_d_dir() -> PathBuf { config_dir().join("orch.d") }

/// Aggregate Orchfile — orchd reads this; we rebuild it from orch.d.
pub fn orchfile() -> PathBuf { config_dir().join(ORCHFILE_BASENAME) }

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

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME must be set")
}

/// Expand a leading `~/` or bare `~` against `$HOME`. Leaves absolute or
/// non-tilde paths alone. Single source of truth for user-config path
/// expansion across the workspace.
pub fn expand_tilde(p: &std::path::Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") { return home().join(rest); }
    if s == "~" { return home(); }
    p.to_path_buf()
}
