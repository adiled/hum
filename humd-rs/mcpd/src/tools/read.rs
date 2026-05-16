//! Read — text + image. v0 keeps the surface deliberately narrow: read
//! the bytes, line-number them, return up to ~7500 chars. Images come
//! back as base64 with mime so Claude CLI can render them.

use crate::protocol::{ToolDef, ToolResult};
use crate::session::SessionState;
use crate::tools::fs_util::{assert_path, is_image};
use base64::Engine;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_LINES: usize = 2000;

#[derive(Deserialize)]
struct Args {
    file_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

pub fn def() -> ToolDef {
    ToolDef {
        name: "Read".to_string(),
        description: "Read a file from the filesystem. Returns line-numbered text for text files, base64 + mime for images. Refuses paths outside the session cwd (with /tmp also allowed). `offset` (1-based line) and `limit` (line count) crop the slice; both optional.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Absolute path to read." },
                "offset": { "type": "number", "description": "1-based line offset. Optional." },
                "limit": { "type": "number", "description": "Maximum number of lines. Optional." },
            },
            "required": ["file_path"],
        }),
    }
}

pub fn run(args: Value, session: &Arc<Mutex<SessionState>>) -> ToolResult {
    let args: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("invalid args: {e}")),
    };
    let snap = session.lock().clone();
    let abs = match assert_path(&args.file_path, &snap) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e),
    };
    if let Err(e) = snap.check_permission("Read", abs.to_str()) {
        return ToolResult::error(e);
    }
    if !abs.exists() {
        return ToolResult::error(format!("File does not exist: {}", abs.display()));
    }
    let meta = match std::fs::metadata(&abs) {
        Ok(m) => m,
        Err(e) => return ToolResult::error(format!("stat failed: {e}")),
    };
    if meta.is_dir() {
        return ToolResult::error(format!(
            "Path is a directory, not a file: {}",
            abs.display()
        ));
    }
    if is_image(&abs) {
        return read_image(&abs);
    }
    read_text(&abs, args.offset, args.limit)
}

fn read_text(path: &Path, offset: Option<usize>, limit: Option<usize>) -> ToolResult {
    let raw = match std::fs::read(path) {
        Ok(v) => v,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };
    let trunc = raw.len() > MAX_READ_BYTES;
    let slice = if trunc { &raw[..MAX_READ_BYTES] } else { &raw[..] };
    let s = String::from_utf8_lossy(slice).into_owned();

    let start = offset.unwrap_or(1).saturating_sub(1);
    let take = limit.unwrap_or(MAX_LINES).min(MAX_LINES);
    let mut out = String::new();
    for (idx, line) in s.lines().skip(start).take(take).enumerate() {
        let n = start + idx + 1;
        out.push_str(&format!("{n:>6}\t{line}\n"));
    }
    if trunc {
        out.push_str(&format!(
            "[mcpd: truncated at {} KB — file is larger]\n",
            MAX_READ_BYTES / 1024
        ));
    }
    ToolResult::text(out)
}

fn read_image(path: &Path) -> ToolResult {
    let bytes = match std::fs::read(path) {
        Ok(v) => v,
        Err(e) => return ToolResult::error(format!("read failed: {e}")),
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream().to_string();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    // Native MCP image content type — passed straight back when nestler
    // serializes. v0 wraps the descriptor in the text output so the
    // client at least sees something useful.
    ToolResult {
        output: format!("[image:{mime}, {} bytes]\n{b64}", bytes.len()),
        title: Some(format!("image: {}", path.display())),
        metadata: Some(json!({ "mime": mime, "bytes": bytes.len() })),
        is_error: false,
    }
}
