//! Hum state persistence.
//!
//! Each hum is a long-lived session record: which nest (Claude CLI, future
//! backends) drives it, which bees (OpenCode and any hear-only
//! observers) are attached, and the per-turn cached fields the daemon
//! needs to cold-respawn an inference process.
//!
//! Persisted as a single JSON object keyed by session id at
//! `${XDG_STATE_HOME or HOME/.local/state}/hum/hums.json`. Writes are
//! atomic (tmp + rename) so a crash mid-flush can't corrupt the file.
//!
//! On load we backfill legacy shapes so older state files survive an
//! upgrade — flat `claudeSessionId` collapses into `nest[0].id`,
//! `opencodeSessionId` / `plugin[]` collapse into `nestled[]`, and
//! `nest[]` is capped at 1.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── Types ───────────────────────────────────────────────────────────────────

/// One entry of `Hum.nest` — the driver process backing this session.
/// Capped at one in storage; the array shape is preserved for future
/// multi-nest provision (e.g. a side-by-side dry-run backend).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestRef {
    pub nest: String,
    pub id: String,
}

/// One entry of `Hum.nestled` — an attached observer or driver. The first
/// entry without `hear_only = true` is the active bee.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestledRef {
    pub bee: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hear_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A single session's persisted state. Field order mirrors the TS
/// `interface Hum` so the JSON shape stays diff-friendly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hum {
    pub id: String,
    /// Capped at 1 in storage. The array shape is kept for future
    /// multi-nest provision.
    #[serde(default)]
    pub nest: Vec<NestRef>,
    /// One driver, zero or more hear-only observers.
    #[serde(default)]
    pub nestled: Vec<NestledRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_respawn: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accessed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_petal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oc_server_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thorns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_tool_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_permissions: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
}

// ─── Registry ────────────────────────────────────────────────────────────────

/// The on-disk registry: `sid -> Hum`. Wrapped in an RwLock for shared
/// read/write across daemon subsystems.
pub struct Hums {
    inner: RwLock<HashMap<String, Hum>>,
    path: PathBuf,
}

impl Hums {
    /// State directory: `${XDG_STATE_HOME or HOME/.local/state}/hum`.
    pub fn state_dir() -> PathBuf {
        if let Ok(p) = std::env::var("XDG_STATE_HOME") {
            return PathBuf::from(p).join("hum");
        }
        if let Some(p) = directories::ProjectDirs::from("", "", "hum") {
            // ProjectDirs.state_dir() is only populated on Linux; fall back
            // to ~/.local/state/hum elsewhere via home_dir below.
            if let Some(s) = p.state_dir() {
                return s.to_path_buf();
            }
        }
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".local/state/hum")
    }

    /// Default file path: `<state_dir>/hums.json`.
    pub fn default_file() -> PathBuf {
        Self::state_dir().join("hums.json")
    }

    /// Load the registry from the default path, applying legacy backfill.
    /// A missing or unparseable file yields an empty registry.
    pub fn load() -> Self {
        Self::load_from(Self::default_file())
    }

    /// Load from a specific path. Same backfill semantics as [`Hums::load`].
    pub fn load_from(path: PathBuf) -> Self {
        let map = read_and_backfill(&path).unwrap_or_default();
        Hums {
            inner: RwLock::new(map),
            path,
        }
    }

    /// Path of the file this registry will be written to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of hums currently in the registry.
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Run a closure against the inner map under a read lock. Cloning a
    /// snapshot is the caller's call — we hand out the borrow only for the
    /// duration of the closure to keep lock scopes obvious.
    pub fn with_read<R>(&self, f: impl FnOnce(&HashMap<String, Hum>) -> R) -> R {
        f(&*self.inner.read())
    }

    /// Mutate the inner map under a write lock. The caller is responsible
    /// for invoking [`Hums::save`] (or letting the daemon's tick do it).
    pub fn with_write<R>(&self, f: impl FnOnce(&mut HashMap<String, Hum>) -> R) -> R {
        f(&mut *self.inner.write())
    }

    /// Get a clone of a single hum by sid.
    pub fn get(&self, sid: &str) -> Option<Hum> {
        self.inner.read().get(sid).cloned()
    }

    /// Insert or replace a hum.
    pub fn insert(&self, sid: String, hum: Hum) -> Option<Hum> {
        self.inner.write().insert(sid, hum)
    }

    /// Remove a hum by sid.
    pub fn remove(&self, sid: &str) -> Option<Hum> {
        self.inner.write().remove(sid)
    }

    /// Atomically persist the registry to disk. The `sid` argument matches
    /// the TS signature — currently the whole file is rewritten on every
    /// save, but we accept the sid so callers can wire wane/drone hooks
    /// without changing the call site later.
    pub fn save(&self, _sid: Option<&str>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let snapshot: HashMap<String, Hum> = self.inner.read().clone();
        let bytes = serde_json::to_vec(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        // Same-directory tmp + rename keeps the swap on one filesystem,
        // so it stays atomic on POSIX.
        let tmp = tmp_sibling(&self.path);
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Drop hums that have been idle for more than `max_age_ms` and whose
    /// `last_accessed` is set. Returns the number reaped. The TS daemon
    /// also consults `nest.roost(sid)` to avoid reaping live processes —
    /// that check has to happen at the call site since this crate doesn't
    /// know about the nest.
    pub fn reap_stale(&self, max_age_ms: i64) -> usize {
        let now = now_ms();
        let mut guard = self.inner.write();
        let before = guard.len();
        guard.retain(|_sid, h| match h.last_accessed {
            Some(ts) => (now - ts) < max_age_ms,
            None => true,
        });
        before - guard.len()
    }

    /// Like [`Hums::reap_stale`] but skip any sid for which `is_alive(sid)`
    /// returns true. Mirrors the TS `reapSessions` check against
    /// `nest.roost(sid)`.
    pub fn reap_stale_unless<F: Fn(&str) -> bool>(&self, max_age_ms: i64, is_alive: F) -> usize {
        let now = now_ms();
        let mut guard = self.inner.write();
        let before = guard.len();
        guard.retain(|sid, h| {
            let stale = match h.last_accessed {
                Some(ts) => (now - ts) >= max_age_ms,
                None => false,
            };
            if !stale {
                return true;
            }
            is_alive(sid)
        });
        before - guard.len()
    }
}

// ─── Accessors (free functions, matching the TS API) ─────────────────────────

/// First nest entry's id, or `None` if no nest is attached.
pub fn nest_id(h: &Hum) -> Option<&str> {
    h.nest.first().map(|n| n.id.as_str())
}

/// First nest entry's name, or `None`.
pub fn nest_name(h: &Hum) -> Option<&str> {
    h.nest.first().map(|n| n.nest.as_str())
}

/// Resolve the on-disk path for this hum's nest, given a resolver that
/// maps `(cwd, nest_id)` to a path. Mirrors TS `nestPath`, which calls
/// `getSessionPath(h.cwd, id)` — the resolver is owned by the nest crate
/// so we accept it as a closure.
pub fn nest_path<F>(h: &Hum, resolver: F) -> Option<PathBuf>
where
    F: FnOnce(&str, &str) -> PathBuf,
{
    let id = nest_id(h)?;
    let cwd = h.cwd.as_deref()?;
    Some(resolver(cwd, id))
}

/// First nestled entry's id, or `None`.
pub fn nestled_id(h: &Hum) -> Option<&str> {
    h.nestled.first().map(|n| n.id.as_str())
}

/// First nestled entry's name (`"opencode"`, etc.), or `None`.
pub fn nestling_name(h: &Hum) -> Option<&str> {
    h.nestled.first().map(|n| n.bee.as_str())
}

/// Update or create the single nest entry, replacing any existing one.
pub fn set_nest(h: &mut Hum, nest: impl Into<String>, id: impl Into<String>) {
    let entry = NestRef {
        nest: nest.into(),
        id: id.into(),
    };
    if h.nest.is_empty() {
        h.nest.push(entry);
    } else {
        h.nest[0] = entry;
    }
}

// ─── Internals ───────────────────────────────────────────────────────────────

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn tmp_sibling(p: &Path) -> PathBuf {
    let mut name = p
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("hums.json"));
    name.push(".tmp");
    p.with_file_name(name)
}

/// Read `path` as JSON, apply legacy backfill on each entry, then
/// deserialize each value into a typed `Hum`. Returns `None` if the file
/// cannot be read or parsed — matches the TS catch-all.
fn read_and_backfill(path: &Path) -> Option<HashMap<String, Hum>> {
    let raw = fs::read(path).ok()?;
    let mut root: Value = serde_json::from_slice(&raw).ok()?;
    let obj = root.as_object_mut()?;

    let mut id_back = 0usize;
    let mut nest_back = 0usize;
    let mut nestled_back = 0usize;
    let mut out = HashMap::with_capacity(obj.len());

    for (sid, entry) in obj.iter_mut() {
        let Some(o) = entry.as_object_mut() else {
            continue;
        };

        // Mint a fresh id when one is missing or non-string. The TS uses
        // `mintId()`; we leave the field blank to avoid pulling the `ids`
        // crate into the dep graph, and let the daemon mint a replacement
        // on first touch. (Counts the backfill either way for parity with
        // TS logging.)
        let needs_id = !matches!(o.get("id"), Some(Value::String(s)) if !s.is_empty());
        if needs_id {
            o.insert("id".into(), Value::String(String::new()));
            id_back += 1;
        }

        // Pull legacy flat fields; remove so they don't linger on writeback.
        let claude_session_id = o
            .remove("claudeSessionId")
            .and_then(|v| v.as_str().map(str::to_string));
        let opencode_session_id = o
            .remove("opencodeSessionId")
            .and_then(|v| v.as_str().map(str::to_string));
        let _claude_session_path = o.remove("claudeSessionPath");
        let plugin_arr = o.remove("plugin");

        // nest backfill — accept missing/non-array, cap to length 1.
        let nest_ok = matches!(o.get("nest"), Some(Value::Array(_)));
        if !nest_ok {
            let nest_name = resolve_nest_name(o.get("cwd").and_then(|v| v.as_str()));
            let id = claude_session_id.unwrap_or_default();
            o.insert(
                "nest".into(),
                Value::Array(vec![serde_json::json!({ "nest": nest_name, "id": id })]),
            );
            nest_back += 1;
        } else if let Some(Value::Array(arr)) = o.get_mut("nest") {
            if arr.len() > 1 {
                arr.truncate(1);
                nest_back += 1;
            }
        }

        // nestled backfill — prefer plugin[] when present, else fall back
        // to opencodeSessionId, else default to a single OC entry keyed
        // by sid.
        let nestled_ok = matches!(o.get("nestled"), Some(Value::Array(_)));
        if !nestled_ok {
            let new_arr = if let Some(Value::Array(plugins)) = plugin_arr {
                plugins
                    .into_iter()
                    .map(|p| {
                        let bee = p
                            .get("plugin")
                            .and_then(|v| v.as_str())
                            .unwrap_or("opencode")
                            .to_string();
                        let id = p
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .unwrap_or_else(|| sid.clone());
                        serde_json::json!({ "bee": bee, "id": id })
                    })
                    .collect::<Vec<_>>()
            } else {
                let id = opencode_session_id.unwrap_or_else(|| sid.clone());
                vec![serde_json::json!({ "bee": "opencode", "id": id })]
            };
            o.insert("nestled".into(), Value::Array(new_arr));
            nestled_back += 1;
        }

        match serde_json::from_value::<Hum>(entry.clone()) {
            Ok(hum) => {
                out.insert(sid.clone(), hum);
            }
            Err(e) => {
                tracing::warn!(sid = %sid, error = %e, "hums.backfill.skip");
            }
        }
    }

    if id_back + nest_back + nestled_back > 0 {
        tracing::info!(
            id = id_back,
            nest = nest_back,
            nestled = nestled_back,
            "hum.backfilled"
        );
    }

    Some(out)
}

/// Default nest name when we have no other signal. The TS calls
/// `resolveNestName(cwd)` which inspects per-cwd config; from this crate
/// we don't have access to that, so we return the safe default and let
/// the daemon override on next write.
fn resolve_nest_name(_cwd: Option<&str>) -> &'static str {
    "claude-cli"
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "hums-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn blank_hum(id: &str) -> Hum {
        Hum {
            id: id.into(),
            nest: vec![],
            nestled: vec![],
            cwd: None,
            model_id: "m".into(),
            tools: None,
            needs_respawn: None,
            last_accessed: None,
            last_synced_petal: None,
            oc_server_url: None,
            thorns: None,
            external_tool_names: None,
            plan_mode: None,
            last_system_prompt: None,
            last_permissions: None,
            last_allowed_tools: None,
            max_context_tokens: None,
        }
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tmpdir();
        let h = Hums::load_from(dir.join("absent.json"));
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tmpdir();
        let path = dir.join("hums.json");
        let h = Hums::load_from(path.clone());
        let mut hum = blank_hum("abc");
        hum.nestled = vec![NestledRef {
            bee: "opencode".into(),
            id: "o1".into(),
            hear_only: None,
        }];
        hum.cwd = Some("/tmp".into());
        hum.model_id = "claude-opus-4-7".into();
        hum.last_accessed = Some(now_ms());
        hum.last_system_prompt = Some("sys".into());
        hum.last_allowed_tools = Some(vec!["Read".into()]);
        hum.max_context_tokens = Some(123_456);
        set_nest(&mut hum, "claude-cli", "n2");
        h.insert("sid-1".into(), hum);
        h.save(None).unwrap();

        let h2 = Hums::load_from(path);
        let got = h2.get("sid-1").expect("present");
        assert_eq!(nest_id(&got), Some("n2"));
        assert_eq!(nestled_id(&got), Some("o1"));
        assert_eq!(got.model_id, "claude-opus-4-7");
        assert_eq!(got.max_context_tokens, Some(123_456));
    }

    #[test]
    fn backfills_legacy_flat_fields() {
        let dir = tmpdir();
        let path = dir.join("legacy.json");
        let legacy = r#"{
            "sid-old": {
                "id": "X",
                "claudeSessionId": "claude-1",
                "opencodeSessionId": "oc-1",
                "claudeSessionPath": "/p",
                "cwd": "/work",
                "modelId": "old-model"
            },
            "sid-plugin": {
                "id": "Y",
                "modelId": "m",
                "plugin": [
                    { "plugin": "opencode", "id": "oc-2" },
                    { "plugin": "ghost", "id": "g-1" }
                ]
            }
        }"#;
        fs::write(&path, legacy).unwrap();

        let h = Hums::load_from(path);
        let a = h.get("sid-old").unwrap();
        assert_eq!(nest_id(&a), Some("claude-1"));
        assert_eq!(nestled_id(&a), Some("oc-1"));
        assert_eq!(nestling_name(&a), Some("opencode"));

        let b = h.get("sid-plugin").unwrap();
        assert_eq!(b.nestled.len(), 2);
        assert_eq!(b.nestled[0].bee, "opencode");
        assert_eq!(b.nestled[1].bee, "ghost");
        assert_eq!(b.nestled[1].id, "g-1");
    }

    #[test]
    fn nest_array_caps_at_one() {
        let dir = tmpdir();
        let path = dir.join("multi.json");
        let raw = r#"{
            "sid": {
                "id": "Z",
                "modelId": "m",
                "nest": [
                    { "nest": "claude-cli", "id": "a" },
                    { "nest": "future", "id": "b" }
                ],
                "nestled": []
            }
        }"#;
        fs::write(&path, raw).unwrap();
        let h = Hums::load_from(path);
        let got = h.get("sid").unwrap();
        assert_eq!(got.nest.len(), 1);
        assert_eq!(nest_id(&got), Some("a"));
    }

    #[test]
    fn reap_stale_drops_old_entries() {
        let dir = tmpdir();
        let path = dir.join("reap.json");
        let h = Hums::load_from(path);
        let now = now_ms();

        let mut fresh = blank_hum("1");
        fresh.last_accessed = Some(now);
        h.insert("fresh".into(), fresh);

        let mut stale = blank_hum("2");
        stale.last_accessed = Some(now - 10_000);
        h.insert("stale".into(), stale);

        let reaped = h.reap_stale(5_000);
        assert_eq!(reaped, 1);
        assert!(h.get("fresh").is_some());
        assert!(h.get("stale").is_none());
    }

    #[test]
    fn reap_stale_unless_keeps_alive() {
        let dir = tmpdir();
        let path = dir.join("reap2.json");
        let h = Hums::load_from(path);
        let now = now_ms();
        let mut stale = blank_hum("1");
        stale.last_accessed = Some(now - 10_000);
        h.insert("stale-alive".into(), stale);
        let reaped = h.reap_stale_unless(5_000, |sid| sid == "stale-alive");
        assert_eq!(reaped, 0);
        assert!(h.get("stale-alive").is_some());
    }

    #[test]
    fn nest_path_uses_resolver() {
        let mut hum = blank_hum("1");
        hum.nest = vec![NestRef {
            nest: "claude-cli".into(),
            id: "n-1".into(),
        }];
        hum.cwd = Some("/work".into());

        let p = nest_path(&hum, |cwd, id| {
            PathBuf::from(format!("{cwd}/.sessions/{id}.jsonl"))
        });
        assert_eq!(p, Some(PathBuf::from("/work/.sessions/n-1.jsonl")));
        hum.cwd = None;
        assert!(nest_path(&hum, |_, _| PathBuf::new()).is_none());
    }
}
