//! Single source of truth for every on-disk path hum reads or writes.
//!
//! Call `init()` once at process startup before any other call here.
//! It sets any unset XDG env vars to HOME-relative defaults, so every
//! subsequent call in the process resolves without fallback logic.
//!
//! Layout follows the XDG Base Directory spec:
//!
//! | root | holds |
//! |---|---|
//! | `$XDG_STATE_HOME/hum` | keys, snapshots, drift rings, the thrum socket, the rendezvous file |
//! | `$XDG_CONFIG_HOME/hum` | user-editable config: `hum.json`, `peers.json` |
//! | `$XDG_DATA_HOME/hum` | installed source clone, recipes |
//! | `$XDG_CACHE_HOME/hum` | derived caches, e.g. foreign hive clones |
//! | `$XDG_RUNTIME_DIR/hum` | non-essential per-boot runtime files |
//!
//! The `*_BASENAME` constants are the single source of truth for every
//! filename hum reads or writes. Tests and any code building paths under
//! a non-default root compose these against their own directory, never
//! hardcoding the strings.
//!
//! [`Revocations`] is the withdrawn-bee list. It lives here rather than
//! in humd so `humctl` can edit the list without the daemon's dependency
//! tree. humd reads it once at boot, so a change takes effect on the
//! next start.
//!
//! An absent or unparseable revocation file yields an empty list rather
//! than an error. Failing closed would lock every bee out of the socket,
//! and a daemon that admits nobody is a worse failure than one that
//! admits a revoked bee — the operator can act on the parse warning.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub fn init() {
    let home = home();
    xdg_default("XDG_STATE_HOME", home.join(".local/state"));
    xdg_default("XDG_CONFIG_HOME", home.join(".config"));
    xdg_default("XDG_DATA_HOME", home.join(".local/share"));
    xdg_default("XDG_CACHE_HOME", home.join(".cache"));
    xdg_default("XDG_RUNTIME_DIR", home.join(".local/state/run"));
}

fn xdg_default(var: &str, default: PathBuf) {
    if std::env::var_os(var).is_none() {
        unsafe {
            std::env::set_var(var, default);
        }
    }
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME").join("hum")
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME").join("hum")
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME").join("hum")
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME").join("hum")
}

pub fn runtime_dir() -> PathBuf {
    xdg("XDG_RUNTIME_DIR").join("hum")
}

pub const THRUM_SOCK_BASENAME: &str = "thrum.sock";
pub const HTTP_SOCK_BASENAME: &str = "hum.sock.http";
pub const COMB_BASENAME: &str = "comb.json";
pub const HUMD_KEY_BASENAME: &str = "humd.key";
pub const BEES_SNAPSHOT_BASENAME: &str = "bees.json";
pub const RUNTIME_INFO_BASENAME: &str = "runtime.json";
pub const REVOKED_BEES_BASENAME: &str = "revoked.json";
pub const HUM_JSON_BASENAME: &str = "hum.json";
pub const PEERS_JSON_BASENAME: &str = "peers.json";
pub const ORCHFILE_BASENAME: &str = "Orchfile";
pub const ROUTING_JSON_BASENAME: &str = "routing.json";
pub const HIVES_SUBDIR: &str = "hives";
pub const RECIPES_SUBDIR: &str = "recipes";
pub const HIVE_INSTALL_SCRIPT: &str = "install";

pub fn claude_session_dir(cwd_hash: &str) -> PathBuf {
    claude_data_dir().join("projects").join(cwd_hash)
}

pub fn thrum_sock() -> PathBuf {
    if let Some(p) = std::env::var_os("HUM_THRUM_SOCK") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    state_dir().join(THRUM_SOCK_BASENAME)
}

pub fn thrum_sock_resolved() -> PathBuf {
    if let Some(p) = std::env::var_os("HUM_THRUM_SOCK") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    if let Some(rt) = RuntimeInfo::read() {
        return rt.socket;
    }
    state_dir().join(THRUM_SOCK_BASENAME)
}

pub fn http_sock() -> PathBuf {
    runtime_dir().join(HTTP_SOCK_BASENAME)
}

pub fn routing_json() -> PathBuf {
    state_dir().join(ROUTING_JSON_BASENAME)
}

pub fn comb() -> PathBuf {
    runtime_dir().join(COMB_BASENAME)
}

pub fn humd_key() -> PathBuf {
    state_dir().join(HUMD_KEY_BASENAME)
}

pub fn bees_dir() -> PathBuf {
    state_dir().join("bees")
}

pub fn bee_key(kind: &str) -> PathBuf {
    bees_dir().join(format!("{kind}.key"))
}

pub fn bees_snapshot() -> PathBuf {
    state_dir().join(BEES_SNAPSHOT_BASENAME)
}

pub fn runtime_info() -> PathBuf {
    state_dir().join(RUNTIME_INFO_BASENAME)
}

pub fn revoked_bees() -> PathBuf {
    state_dir().join(REVOKED_BEES_BASENAME)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Revocations {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub revoked: Vec<String>,
}

impl Revocations {
    pub fn read_from(path: &std::path::Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) if bytes.is_empty() => Self::default(),
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(list) => list,
                Err(e) => {
                    tracing::warn!(
                        target: "revoke",
                        path = %path.display(),
                        err = %e,
                        "revoke.load.parse_failed — treating as empty"
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn read() -> Self {
        Self::read_from(&revoked_bees())
    }

    pub fn write_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut body = self.clone();
        body.version = 1;
        let body = serde_json::to_string_pretty(&body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, path)
    }

    pub fn write(&self) -> std::io::Result<()> {
        self.write_to(&revoked_bees())
    }

    pub fn contains(&self, hid_hex: &str) -> bool {
        self.revoked.iter().any(|h| h == hid_hex)
    }

    pub fn revoke(&mut self, hid: &str) -> bool {
        if self.contains(hid) {
            return false;
        }
        self.revoked.push(hid.to_string());
        true
    }

    pub fn unrevoke(&mut self, hid: &str) -> bool {
        let before = self.revoked.len();
        self.revoked.retain(|h| h != hid);
        self.revoked.len() != before
    }
}

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

pub fn hum_json() -> PathBuf {
    config_dir().join(HUM_JSON_BASENAME)
}

pub fn peers_json() -> PathBuf {
    config_dir().join(PEERS_JSON_BASENAME)
}

pub fn drift_dir() -> PathBuf {
    state_dir().join("drift")
}

pub fn thehum_dir() -> PathBuf {
    state_dir().join("thehum")
}

pub const THEHUM_SEQ_BASENAME: &str = "seq.bin";
pub const THEHUM_SNAPSHOTS_SUBDIR: &str = "snapshots";
pub const THEHUM_ROOT_BASENAME: &str = "root.txt";
pub const THEHUM_NDJSON_EXT: &str = "ndjson";

pub fn thehum_seq_file(dir: &std::path::Path) -> PathBuf {
    dir.join(THEHUM_SEQ_BASENAME)
}
pub fn thehum_snapshots_dir(dir: &std::path::Path) -> PathBuf {
    dir.join(THEHUM_SNAPSHOTS_SUBDIR)
}
pub fn thehum_root_file(dir: &std::path::Path) -> PathBuf {
    dir.join(THEHUM_ROOT_BASENAME)
}

pub fn src_dir() -> PathBuf {
    data_dir().join("src")
}

pub fn local_dir() -> PathBuf {
    home().join(".local")
}

pub fn local_bin_dir() -> PathBuf {
    local_dir().join("bin")
}

pub fn humd_bin() -> PathBuf {
    local_bin_dir().join("humd")
}

pub fn fnm_node_bin() -> PathBuf {
    local_dir().join("share/fnm/aliases/default/bin/node")
}

pub fn claude_data_dir() -> PathBuf {
    home().join(".claude")
}

pub fn hum_bin(name: &str) -> PathBuf {
    local_bin_dir().join(name)
}

pub fn foreign_hive_cache(org: &str, repo: &str, branch: &str) -> PathBuf {
    cache_dir()
        .join("hives")
        .join(format!("{org}-{repo}-{branch}"))
}

pub fn orch_d_dir() -> PathBuf {
    config_dir().join("orch.d")
}

pub fn orchfile() -> PathBuf {
    config_dir().join(ORCHFILE_BASENAME)
}

pub fn bee_config(kind: &str) -> PathBuf {
    config_dir().join("bees").join(format!("{kind}.json"))
}

pub enum DaemonLogs {
    Journald { unit: String },
    Files { stdout: PathBuf, stderr: PathBuf },
}

pub fn daemon_logs(name: &str) -> DaemonLogs {
    #[cfg(target_os = "macos")]
    {
        let base = home().join("Library/Logs");
        DaemonLogs::Files {
            stdout: base.join(format!("sh.hum.{name}.out.log")),
            stderr: base.join(format!("sh.hum.{name}.err.log")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    DaemonLogs::Journald {
        unit: name.to_string(),
    }
}

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

pub fn expand_tilde(p: &std::path::Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        return home().join(rest);
    }
    if s == "~" {
        return home();
    }
    p.to_path_buf()
}

pub mod config;

#[cfg(test)]
mod revocation_tests {
    use super::*;

    #[test]
    fn an_absent_file_revokes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let list = Revocations::read_from(&tmp.path().join("nope.json"));
        assert!(!list.contains("wbee_abc"));
    }

    #[test]
    fn a_listed_hid_is_contained() {
        let mut list = Revocations::default();
        assert!(list.revoke("wbee_dead"));
        assert!(list.contains("wbee_dead"));
        assert!(!list.contains("wbee_other"));
    }

    #[test]
    fn revoking_twice_is_not_a_duplicate() {
        let mut list = Revocations::default();
        assert!(list.revoke("wbee_dead"));
        assert!(!list.revoke("wbee_dead"), "already listed");
        assert_eq!(list.revoked.len(), 1);
    }

    #[test]
    fn unrevoking_removes_only_that_hid() {
        let mut list = Revocations::default();
        list.revoke("wbee_dead");
        list.revoke("wbee_gone");
        assert!(list.unrevoke("wbee_dead"));
        assert!(!list.contains("wbee_dead"));
        assert!(list.contains("wbee_gone"));
        assert!(!list.unrevoke("wbee_dead"), "nothing left to remove");
    }

    #[test]
    fn a_list_survives_a_round_trip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        let mut list = Revocations::default();
        list.revoke("wbee_dead");
        list.write_to(&path).unwrap();
        assert!(Revocations::read_from(&path).contains("wbee_dead"));
    }

    #[test]
    fn a_corrupt_file_revokes_nothing_rather_than_everything() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(!Revocations::read_from(&path).contains("wbee_abc"));
    }
}
