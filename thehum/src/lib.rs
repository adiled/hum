//! `thehum` — per-humd authored chi log.
//!
//! Every humd maintains a signed append-only log of every chi it observes.
//! The log is the only authoritative store of activity; everything else
//! (bees.json, route tables, sid state) is a derived view.
//!
//! Event shape: chi envelope + author hid + monotonic seq + ts_ms +
//! prev_hash (chain) + ed25519 signature.
//!
//! Three flavors of participation:
//! - Archive — keep all logs forever
//! - Rolling — drop daily files older than N days
//! - Light — keep snapshots + own-sids only

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub mod anchor;
pub mod append;
pub mod canon;
pub mod layout;
pub mod read;
pub mod retention;
pub mod sign;
pub mod snapshot;

pub use ed25519_dalek::{SigningKey as Key, VerifyingKey as PubKey};
pub use ids::HumId;

pub type Seq = u64;
pub type Hash32 = [u8; 32];
pub type StateRoot = Hash32;

/// One canonical chi-log line: envelope + author + chain + sig.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// The chi kind (e.g. "hello", "prompt", "chunk", "tool-call",
    /// "finish", "snapshot", "backfill").
    pub chi: String,
    /// Session id this event belongs to. None for events that don't
    /// scope to a sid (some "hello", network-wide "snapshot").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<HumId>,
    /// Correlation id (the rid in chi envelopes).
    pub rid: HumId,
    /// The rest of the wire body, verbatim.
    pub body: serde_json::Value,
    /// Author humd's hid hex form.
    pub author: String,
    /// Strictly monotonic per author. Gap = missing event.
    pub seq: Seq,
    /// Wall-clock at append time, ms since epoch. Reading code uses
    /// this, NEVER `now()`.
    pub ts_ms: i64,
    /// Hex of sha256 over the prior event's canonical bytes. Chain
    /// integrity: any tampering invalidates downstream hashes.
    pub prev_hash: String,
    /// Hex of 64-byte ed25519 signature over the canonical pre-sig
    /// bytes of THIS event.
    pub sig: String,
}

/// Retention mode for this humd's chi log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionMode {
    Archive,
    Rolling,
    Light,
}

impl Default for RetentionMode {
    fn default() -> Self { Self::Archive }
}

/// Persistence configuration. Read from `hum.json` `thehum` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub mode: RetentionMode,
    /// For Rolling mode: drop daily files older than this.
    #[serde(default = "default_days")]
    pub days: u32,
    /// Snapshot every N events (whichever fires first).
    #[serde(default = "default_snapshot_events")]
    pub snapshot_every_events: u64,
    /// Snapshot every N seconds (whichever fires first).
    #[serde(default = "default_snapshot_seconds")]
    pub snapshot_every_seconds: u64,
    /// Encrypt-at-rest (v0: not wired; field reserved).
    #[serde(default)]
    pub encrypt_at_rest: bool,
    /// fsync per event (true) or batched (false). Per-event by default.
    #[serde(default = "default_fsync")]
    pub fsync_per_event: bool,
}

fn default_days() -> u32 { 30 }
fn default_snapshot_events() -> u64 { 1000 }
fn default_snapshot_seconds() -> u64 { 600 }
fn default_fsync() -> bool { true }

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: RetentionMode::default(),
            days: default_days(),
            snapshot_every_events: default_snapshot_events(),
            snapshot_every_seconds: default_snapshot_seconds(),
            encrypt_at_rest: false,
            fsync_per_event: default_fsync(),
        }
    }
}

/// The per-humd chi-log handle. Single appender, many readers.
pub struct TheHum {
    pub(crate) dir: PathBuf,
    pub(crate) signing_key: Arc<SigningKey>,
    pub(crate) author_hid: String,
    pub(crate) cfg: Config,
    pub(crate) state: Arc<Mutex<AppendState>>,
    pub(crate) live_tx: broadcast::Sender<Event>,
}

pub(crate) struct AppendState {
    pub seq: Seq,
    pub prev_hash: Hash32,
    pub last_snapshot_seq: Seq,
    pub last_snapshot_ts_ms: i64,
}

impl TheHum {
    /// Open (or initialize) thehum at `dir`. Loads seq.bin, last-line
    /// hash, snapshot pointer. Mints the dir on first run.
    pub fn open(dir: &Path, signing_key: SigningKey, cfg: Config) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create thehum dir {}", dir.display()))?;
        std::fs::create_dir_all(layout::snapshots_dir(dir))
            .with_context(|| format!("create snapshots dir {}", layout::snapshots_dir(dir).display()))?;

        let pubkey = signing_key.verifying_key();
        let author_hid = ensemble::Hid::from_pubkey(
            ensemble::HidPrefix::Humd,
            &pubkey.to_bytes(),
        ).to_hex();

        let (seq, prev_hash) = append::recover_state(dir)
            .context("recover append state")?;

        let (live_tx, _rx) = broadcast::channel::<Event>(1024);

        Ok(Self {
            dir: dir.to_path_buf(),
            signing_key: Arc::new(signing_key),
            author_hid,
            cfg,
            state: Arc::new(Mutex::new(AppendState {
                seq,
                prev_hash,
                last_snapshot_seq: 0,
                last_snapshot_ts_ms: 0,
            })),
            live_tx,
        })
    }

    pub fn author_hid(&self) -> &str { &self.author_hid }
    pub fn dir(&self) -> &Path { &self.dir }
    pub fn cfg(&self) -> &Config { &self.cfg }
}
