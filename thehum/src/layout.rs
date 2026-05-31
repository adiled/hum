//! Single source of truth for thehum's on-disk filenames.

use std::path::{Path, PathBuf};

pub fn seq_file(dir: &Path) -> PathBuf { dir.join("seq.bin") }
pub fn snapshots_dir(dir: &Path) -> PathBuf { dir.join("snapshots") }
pub fn root_file(dir: &Path) -> PathBuf { dir.join("root.txt") }
pub fn ndjson_ext() -> &'static str { "ndjson" }
