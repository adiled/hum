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
//! | `$XDG_STATE_HOME/hum` | keys, snapshots, the thrum socket, the rendezvous file |
//! | `$XDG_CONFIG_HOME/hum` | user-editable config: `hum.json`, `peers.json` |
//! | `$XDG_DATA_HOME/hum` | installed source clone |
//! | `$XDG_CACHE_HOME/hum` | derived caches, e.g. foreign hive clones |
//! | `$XDG_RUNTIME_DIR/hum` | non-essential per-boot runtime files |
//!
//! [`layout`] declares every entry hum keeps under those roots, and
//! nothing else belongs there: while humd runs it moves a stray into its
//! root's `tmp/`. Writes go through [`write_atomic`], which stages in that
//! `tmp/` and renames into place.
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

pub mod layout;

pub use layout::{Root, write_atomic, write_atomic_private};

pub fn init() {
    let home = home();
    xdg_default("XDG_STATE_HOME", home.join(".local/state"));
    xdg_default("XDG_CONFIG_HOME", home.join(".config"));
    xdg_default("XDG_DATA_HOME", home.join(".local/share"));
    xdg_default("XDG_CACHE_HOME", home.join(".cache"));
    xdg_default("XDG_RUNTIME_DIR", home.join(".local/state/run"));
}

fn set(var: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(var).filter(|v| !v.is_empty())
}

fn xdg_default(var: &str, default: PathBuf) {
    if set(var).is_none() {
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
pub const ADVISORIES_BASENAME: &str = "advisories.json";
pub const HUM_JSON_BASENAME: &str = "hum.json";
pub const PEERS_JSON_BASENAME: &str = "peers.json";
pub const ORCHFILE_BASENAME: &str = "Orchfile";
pub const ROUTING_JSON_BASENAME: &str = "routing.json";
pub const HIVES_SUBDIR: &str = "hives";
pub const HIVE_INSTALL_UNIT: &str = "hive-install";

pub fn thrum_sock() -> PathBuf {
    if let Some(p) = set("HUM_THRUM_SOCK") {
        return PathBuf::from(p);
    }
    if let Some(p) = set("HUM_SOCKET") {
        return PathBuf::from(p);
    }
    state_dir().join(THRUM_SOCK_BASENAME)
}

pub fn thrum_sock_resolved() -> PathBuf {
    if let Some(p) = set("HUM_THRUM_SOCK") {
        return PathBuf::from(p);
    }
    if let Some(p) = set("HUM_SOCKET") {
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
    state_dir().join(layout::BEES_SUBDIR)
}

pub fn bee_key(id: &str) -> PathBuf {
    bees_dir().join(format!("{id}.key"))
}

pub fn entered_dir() -> PathBuf {
    state_dir().join(layout::ENTERED_SUBDIR)
}

pub fn bee_entry(id: &str) -> PathBuf {
    entered_dir().join(format!("{id}.json"))
}

pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
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

pub fn advisories() -> PathBuf {
    state_dir().join(ADVISORIES_BASENAME)
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
        let mut body = self.clone();
        body.version = 1;
        let body = serde_json::to_string_pretty(&body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_atomic(path, body.as_bytes())
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
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_atomic(&runtime_info(), body.as_bytes())
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

pub fn thehum_dir() -> PathBuf {
    state_dir().join(layout::THEHUM_SUBDIR)
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
    data_dir().join(layout::SRC_SUBDIR)
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

pub fn hum_bin(name: &str) -> PathBuf {
    local_bin_dir().join(name)
}

pub fn foreign_hive_cache(org: &str, repo: &str, branch: &str) -> PathBuf {
    cache_dir()
        .join(layout::HIVES_SUBDIR)
        .join(format!("{org}-{repo}-{branch}"))
}

pub fn hives_d_dir() -> PathBuf {
    config_dir().join(layout::HIVES_D_SUBDIR)
}

pub fn hive_installation(hive: &str) -> PathBuf {
    hives_d_dir().join(format!("{hive}.orch"))
}

pub fn hive_installed(hive: &str) -> bool {
    is_plain_name(hive) && hive_installation(hive).is_file()
}

pub fn hive_config(hive: &str) -> PathBuf {
    config_dir().join(layout::HIVES_SUBDIR).join(format!("{hive}.json"))
}

pub fn hive_state_dir(hive: &str) -> PathBuf {
    state_dir().join(layout::HIVES_SUBDIR).join(hive)
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
    if let Some(v) = set(var) {
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
        assert!(!list.contains("bee_abc"));
    }

    #[test]
    fn a_listed_hid_is_contained() {
        let mut list = Revocations::default();
        assert!(list.revoke("bee_dead"));
        assert!(list.contains("bee_dead"));
        assert!(!list.contains("bee_other"));
    }

    #[test]
    fn revoking_twice_is_not_a_duplicate() {
        let mut list = Revocations::default();
        assert!(list.revoke("bee_dead"));
        assert!(!list.revoke("bee_dead"), "already listed");
        assert_eq!(list.revoked.len(), 1);
    }

    #[test]
    fn unrevoking_removes_only_that_hid() {
        let mut list = Revocations::default();
        list.revoke("bee_dead");
        list.revoke("bee_gone");
        assert!(list.unrevoke("bee_dead"));
        assert!(!list.contains("bee_dead"));
        assert!(list.contains("bee_gone"));
        assert!(!list.unrevoke("bee_dead"), "nothing left to remove");
    }

    #[test]
    fn a_hive_installation_lives_under_hives_d() {
        assert!(hive_installation("bash").ends_with("hives.d/bash.orch"));
        assert!(is_plain_name("openai-server"));
        assert!(is_plain_name("claude_cli.2"));
        assert!(!is_plain_name(""));
        assert!(!is_plain_name("../x"));
        assert!(!is_plain_name(".hidden"));
        assert!(!is_plain_name("a b"));
        assert!(!hive_installed("../../etc/passwd"));
    }

    #[test]
    fn a_list_survives_a_round_trip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        let mut list = Revocations::default();
        list.revoke("bee_dead");
        list.write_to(&path).unwrap();
        assert!(Revocations::read_from(&path).contains("bee_dead"));
    }

    #[test]
    fn a_corrupt_file_revokes_nothing_rather_than_everything() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("revoked.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(!Revocations::read_from(&path).contains("bee_abc"));
    }
}
