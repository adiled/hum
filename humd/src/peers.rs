//! Bootstrap peer list — peers humd should dial on boot.
//!
//! Optional file at `$XDG_CONFIG_HOME/hum/peers.json` (default
//! `$HOME/.config/hum/peers.json`). Missing or unreadable file = no
//! peers, daemon boots cleanly. Bad entries are skipped, not fatal —
//! one malformed peer should not break startup for the rest.
//!
//! File shape:
//! ```json
//! {
//!   "peers": [
//!     { "humd_id": "<64-hex>", "hints": ["tcp:host:port", "iroh:<node-id>"] }
//!   ]
//! }
//! ```

use std::path::PathBuf;

use ensemble::Hid;
use serde::{Deserialize, Serialize};
use tracing::{trace, warn};

/// One bootstrap entry, post-parse: id is typed, hints are passed through
/// for the transport layer to decide which to dial.
#[derive(Debug, Clone)]
pub struct PeerConfig {
    pub humd_id: Hid,
    pub hints: Vec<String>,
}

/// Wire shape — fields stay loose strings so we can tolerate malformed
/// rows and still parse the rest. The typed [`PeerConfig`] is what the
/// daemon consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawFile {
    #[serde(default)]
    peers: Vec<RawPeer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawPeer {
    #[serde(default)]
    humd_id: String,
    #[serde(default)]
    hints: Vec<String>,
}

/// Resolve `${XDG_CONFIG_HOME or $HOME/.config}/hum/peers.json`.
pub fn peers_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("hum").join("peers.json");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".config")
                .join("hum")
                .join("peers.json");
        }
    }
    PathBuf::from(".config/hum/peers.json")
}

/// Best-effort load of the peers file.
///
/// Missing file → empty vec. Parse errors on the outer object → empty vec
/// (warn). Malformed rows inside `peers[]` → skipped (warn), good rows
/// kept.
pub fn load() -> Vec<PeerConfig> {
    let path = peers_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            trace!(path = %path.display(), "peers.missing");
            return Vec::new();
        }
        Err(e) => {
            warn!(path = %path.display(), err = %e, "peers.read.failed");
            return Vec::new();
        }
    };
    let parsed: RawFile = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            warn!(path = %path.display(), err = %e, "peers.parse.failed");
            return Vec::new();
        }
    };
    let mut out = Vec::with_capacity(parsed.peers.len());
    for row in parsed.peers {
        match parse_humd_id(&row.humd_id) {
            Some(humd_id) => out.push(PeerConfig { humd_id, hints: row.hints }),
            None => {
                warn!(humd_id = %row.humd_id, "peers.skip.bad-id");
            }
        }
    }
    trace!(path = %path.display(), count = out.len(), "peers.loaded");
    out
}

/// Parse a Hid string (either `<prefix>_<hex>` or bare 64-hex
/// legacy) back into the typed id. None on malformed input.
fn parse_humd_id(s: &str) -> Option<Hid> {
    Hid::from_hex(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Fixture file with two good entries + one malformed → 2 loaded.
    #[test]
    fn load_parses_fixture_and_skips_bad_rows() {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        let dir = tmp.path().join("hum");
        std::fs::create_dir_all(&dir).unwrap();

        let good_a = "a".repeat(64);
        let good_b = "b".repeat(64);
        let bad = "nope";
        let body = format!(
            r#"{{
              "peers": [
                {{ "humd_id": "{good_a}", "hints": ["tcp:host-a:9000"] }},
                {{ "humd_id": "{bad}", "hints": ["tcp:bad:1"] }},
                {{ "humd_id": "{good_b}", "hints": ["tcp:host-b:9001", "iroh:abc"] }}
              ]
            }}"#
        );
        std::fs::write(dir.join("peers.json"), body).unwrap();

        let loaded = load();
        assert_eq!(loaded.len(), 2, "bad row dropped");
        assert_eq!(loaded[0].hints, vec!["tcp:host-a:9000".to_string()]);
        assert_eq!(loaded[1].hints.len(), 2);

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    /// Missing file returns empty without error.
    #[test]
    fn load_missing_file_is_empty() {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        let loaded = load();
        assert!(loaded.is_empty());
        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
