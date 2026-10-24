//! The layout: every entry hum keeps under its roots, and nothing else.
//!
//! Each root (state, config, data, cache, runtime) holds exactly the
//! entries [`LAYOUT`] declares. A declared directory is either free, its
//! contents its owner's business (the source clone, thehum, a hive's own
//! state), or shaped, each child a name the shape admits (bee keys by id,
//! hive installations by hive name). Anything else is a stray. While humd
//! runs, its guard moves a stray into its root's `tmp/`, so whoever wrote
//! it finds it beside where it wrote it, and moves what has sat in `tmp/`
//! a week to `~/.hum-archive/<date>/<root>/`.
//!
//! Writes stage in the root's `tmp/` too: [`write_atomic`] writes a hidden
//! `.write-*` file there and renames it into place, so a half-written file
//! never appears under a declared name or as a stray, and the rename never
//! leaves the root's filesystem. A path under no root stages beside
//! itself.
//!
//! The config root is yours to edit, and an editor leaves swap and backup
//! files beside what it edits, so a stray there moves only once it has
//! been untouched for [`Root::settle`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::{
    BEES_SNAPSHOT_BASENAME, COMB_BASENAME, HTTP_SOCK_BASENAME, HUMD_KEY_BASENAME,
    HUM_JSON_BASENAME, PEERS_JSON_BASENAME, REVOKED_BEES_BASENAME, ROUTING_JSON_BASENAME,
    RUNTIME_INFO_BASENAME, THRUM_SOCK_BASENAME,
};

pub const TMP_SUBDIR: &str = "tmp";
pub const BEES_SUBDIR: &str = "bees";
pub const ENTERED_SUBDIR: &str = "bees.d";
pub const THEHUM_SUBDIR: &str = "thehum";
pub const HIVES_SUBDIR: &str = "hives";
pub const HIVES_D_SUBDIR: &str = "hives.d";
pub const SRC_SUBDIR: &str = "src";
pub const ARCHIVE_DIRNAME: &str = ".hum-archive";
pub const STAGING_PREFIX: &str = ".write-";

const CONFIG_SETTLE: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Root {
    State,
    Config,
    Data,
    Cache,
    Runtime,
}

impl Root {
    pub const ALL: [Root; 5] = [Root::State, Root::Config, Root::Data, Root::Cache, Root::Runtime];

    pub fn dir(self) -> PathBuf {
        match self {
            Root::State => crate::state_dir(),
            Root::Config => crate::config_dir(),
            Root::Data => crate::data_dir(),
            Root::Cache => crate::cache_dir(),
            Root::Runtime => crate::runtime_dir(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Root::State => "state",
            Root::Config => "config",
            Root::Data => "data",
            Root::Cache => "cache",
            Root::Runtime => "runtime",
        }
    }

    pub fn tmp(self) -> PathBuf {
        self.dir().join(TMP_SUBDIR)
    }

    pub fn settle(self) -> Duration {
        match self {
            Root::Config => CONFIG_SETTLE,
            _ => Duration::ZERO,
        }
    }

    pub fn of(path: &Path) -> Option<Root> {
        Root::ALL.into_iter().find(|root| path.starts_with(root.dir()))
    }
}

#[derive(Clone, Copy)]
pub enum Inside {
    File,
    Free,
    Shaped(fn(&str) -> bool),
}

#[derive(Clone, Copy)]
pub struct Entry {
    pub root: Root,
    pub name: &'static str,
    pub inside: Inside,
    pub holds: &'static str,
}

const fn file(root: Root, name: &'static str, holds: &'static str) -> Entry {
    Entry { root, name, inside: Inside::File, holds }
}

const fn free(root: Root, name: &'static str, holds: &'static str) -> Entry {
    Entry { root, name, inside: Inside::Free, holds }
}

const fn shaped(root: Root, name: &'static str, shape: fn(&str) -> bool, holds: &'static str) -> Entry {
    Entry { root, name, inside: Inside::Shaped(shape), holds }
}

pub const LAYOUT: &[Entry] = &[
    file(Root::State, HUMD_KEY_BASENAME, "this humd's identity key"),
    file(Root::State, THRUM_SOCK_BASENAME, "the thrum socket"),
    file(Root::State, RUNTIME_INFO_BASENAME, "the rendezvous: where the running humd listens"),
    file(Root::State, ROUTING_JSON_BASENAME, "peer addresses kept across restarts"),
    file(Root::State, BEES_SNAPSHOT_BASENAME, "the bees last seen"),
    file(Root::State, REVOKED_BEES_BASENAME, "the revoked bees"),
    shaped(Root::State, BEES_SUBDIR, is_bee_key_name, "bee keys, <id>.key"),
    shaped(Root::State, ENTERED_SUBDIR, is_bee_entry_name, "entered bees, <id>.json"),
    free(Root::State, THEHUM_SUBDIR, "the persistent chi log"),
    shaped(Root::State, HIVES_SUBDIR, crate::is_plain_name, "each hive's own state, <hive>/"),
    free(Root::State, TMP_SUBDIR, "staged writes, and strays the guard moved"),
    file(Root::Config, HUM_JSON_BASENAME, "humd's config"),
    file(Root::Config, PEERS_JSON_BASENAME, "bootstrap peers"),
    shaped(Root::Config, HIVES_D_SUBDIR, is_hive_installation_name, "installed hives, <hive>.orch"),
    shaped(Root::Config, HIVES_SUBDIR, is_hive_config_name, "each hive's config, <hive>.json"),
    free(Root::Config, TMP_SUBDIR, "staged writes, and strays the guard moved"),
    free(Root::Data, SRC_SUBDIR, "the installed source clone"),
    free(Root::Data, TMP_SUBDIR, "staged writes, and strays the guard moved"),
    free(Root::Cache, HIVES_SUBDIR, "foreign hive clones"),
    free(Root::Cache, TMP_SUBDIR, "staged writes, and strays the guard moved"),
    file(Root::Runtime, COMB_BASENAME, "the comb"),
    file(Root::Runtime, HTTP_SOCK_BASENAME, "the http socket"),
    free(Root::Runtime, TMP_SUBDIR, "staged writes, and strays the guard moved"),
];

pub fn entry(root: Root, name: &str) -> Option<&'static Entry> {
    LAYOUT.iter().find(|e| e.root == root && e.name == name)
}

pub fn is_bee_id(name: &str) -> bool {
    name.strip_prefix("bee_").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn named(name: &str, suffix: &str, stem_ok: fn(&str) -> bool) -> bool {
    name.strip_suffix(suffix).is_some_and(stem_ok)
}

pub fn is_bee_key_name(name: &str) -> bool {
    named(name, ".key", is_bee_id)
}

pub fn is_bee_entry_name(name: &str) -> bool {
    named(name, ".json", is_bee_id)
}

pub fn is_hive_installation_name(name: &str) -> bool {
    named(name, ".orch", crate::is_plain_name)
}

pub fn is_hive_config_name(name: &str) -> bool {
    named(name, ".json", crate::is_plain_name)
}

fn names_in(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

pub fn strays(root: Root) -> Vec<PathBuf> {
    let dir = root.dir();
    let mut out = Vec::new();
    for name in names_in(&dir) {
        match entry(root, &name).map(|e| e.inside) {
            None => out.push(dir.join(&name)),
            Some(Inside::Shaped(admits)) => {
                for child in names_in(&dir.join(&name)) {
                    if !admits(&child) {
                        out.push(dir.join(&name).join(child));
                    }
                }
            }
            Some(Inside::File) | Some(Inside::Free) => {}
        }
    }
    out
}

pub fn archive_dir() -> PathBuf {
    crate::home().join(ARCHIVE_DIRNAME)
}

static STAGED: AtomicU64 = AtomicU64::new(0);

fn staging_path(path: &Path) -> std::io::Result<PathBuf> {
    let dir = match Root::of(path) {
        Some(root) => root.tmp(),
        None => path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    std::fs::create_dir_all(&dir)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = STAGED.fetch_add(1, Ordering::Relaxed);
    Ok(dir.join(format!("{STAGING_PREFIX}{}-{nanos}-{n}", std::process::id())))
}

fn write_staged(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staged = staging_path(path)?;
    let written = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if private {
                opts.mode(0o600);
            }
        }
        let mut f = opts.open(&staged)?;
        f.write_all(bytes)?;
        if private {
            f.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        drop(f);
        std::fs::rename(&staged, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    written
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_staged(path, bytes, false)
}

pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_staged(path, bytes, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bee_names_are_ids() {
        let id = format!("bee_{}", "ab".repeat(32));
        assert!(is_bee_id(&id));
        assert!(is_bee_key_name(&format!("{id}.key")));
        assert!(is_bee_entry_name(&format!("{id}.json")));
        assert!(!is_bee_key_name("dwarfstar.key"));
        assert!(!is_bee_key_name(&format!("{id}.key.tmp")));
        assert!(!is_bee_id(&format!("wbee_{}", "ab".repeat(32))));
        assert!(!is_bee_id(&format!("bee_{}", "AB".repeat(32))));
    }

    #[test]
    fn hive_names_are_plain() {
        assert!(is_hive_installation_name("dwarfstar.orch"));
        assert!(!is_hive_installation_name("../x.orch"));
        assert!(is_hive_config_name("openai-server.json"));
        assert!(!is_hive_config_name("openai-server.json~"));
    }

    #[test]
    fn every_root_has_a_tmp() {
        for root in Root::ALL {
            assert!(matches!(entry(root, TMP_SUBDIR).map(|e| e.inside), Some(Inside::Free)));
        }
    }

    #[test]
    fn a_name_is_declared_once_per_root() {
        for (i, a) in LAYOUT.iter().enumerate() {
            for b in &LAYOUT[i + 1..] {
                assert!(!(a.root == b.root && a.name == b.name), "{}/{} twice", a.root.name(), a.name);
            }
        }
    }
}
