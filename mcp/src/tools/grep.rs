//! Grep — content search via ripgrep. Shells out to `rg` for speed; if
//! `rg` is missing, falls back to a regex sweep in Rust.

use crate::protocol::{ToolDef, ToolResult};
use crate::session::SessionState;
use crate::tools::fs_util::assert_path;
use parking_lot::Mutex;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

const MAX_RESULTS: usize = 500;

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".svn", ".hg", "dist", "build", ".next", ".nuxt", ".turbo", "target",
    "__pycache__", ".venv", "venv", ".mypy_cache", ".pytest_cache", ".cache", "coverage", ".idea",
    ".vscode",
];

#[derive(Deserialize)]
struct Args {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default = "yes")]
    line_numbers: bool,
    #[serde(default)]
    case_insensitive: bool,
}

fn yes() -> bool { true }

pub fn def() -> ToolDef {
    ToolDef {
        name: "Grep".to_string(),
        description: "Regex search across file content. Backed by ripgrep when available. `path` restricts to a dir or file; `glob` filters by name (e.g. '*.ts'). Returns up to 500 hits.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern":          { "type": "string", "description": "Regex." },
                "path":             { "type": "string", "description": "File or dir. Defaults to session cwd." },
                "glob":             { "type": "string", "description": "Optional filename glob." },
                "line_numbers":     { "type": "boolean", "default": true },
                "case_insensitive": { "type": "boolean", "default": false },
            },
            "required": ["pattern"],
        }),
    }
}

pub fn run(args: Value, session: &Arc<Mutex<SessionState>>) -> ToolResult {
    let args: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return ToolResult::error(format!("invalid args: {e}")),
    };
    let snap = session.lock().clone();
    let base = match args.path.as_deref() {
        Some(p) => match assert_path(p, &snap) { Ok(p) => p, Err(e) => return ToolResult::error(e) },
        None => snap.cwd.clone(),
    };
    if let Err(e) = snap.check_permission("Grep", base.to_str()) {
        return ToolResult::error(e);
    }

    if which_rg() {
        return run_rg(&args, &base);
    }
    run_native(&args, &base)
}

fn which_rg() -> bool {
    Command::new("rg").arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

fn run_rg(args: &Args, base: &Path) -> ToolResult {
    let mut cmd = Command::new("rg");
    cmd.arg("--no-heading").arg("--color=never");
    if args.line_numbers { cmd.arg("-n"); }
    if args.case_insensitive { cmd.arg("-i"); }
    if let Some(g) = &args.glob { cmd.arg("--glob").arg(g); }
    cmd.arg("--").arg(&args.pattern).arg(base);

    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => return ToolResult::error(format!("rg spawn failed: {e}")),
    };
    if !out.status.success() && out.stdout.is_empty() && !out.stderr.is_empty() {
        // exit 1 with no stdout = no matches; only surface stderr otherwise.
        if out.status.code() != Some(1) {
            return ToolResult::error(String::from_utf8_lossy(&out.stderr).into_owned());
        }
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let kept: Vec<&str> = lines.into_iter().take(MAX_RESULTS).collect();
    let mut body = kept.join("\n");
    if total > MAX_RESULTS {
        body.push_str(&format!("\n[... {} more matches, truncated]", total - MAX_RESULTS));
    }
    if body.is_empty() {
        body = format!("(no matches for {})", args.pattern);
    }
    ToolResult::text(body)
}

fn run_native(args: &Args, base: &Path) -> ToolResult {
    let mut builder = String::new();
    if args.case_insensitive { builder.push_str("(?i)"); }
    builder.push_str(&args.pattern);
    let re = match Regex::new(&builder) {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("invalid regex: {e}")),
    };
    let glob_re = match args.glob.as_deref().map(translate_glob).transpose() {
        Ok(r) => r,
        Err(e) => return ToolResult::error(e),
    };
    let mut hits: Vec<String> = Vec::new();
    walk_search(base, &re, glob_re.as_ref(), args.line_numbers, &mut hits);
    let total = hits.len();
    hits.truncate(MAX_RESULTS);
    let mut body = hits.join("\n");
    if total > MAX_RESULTS {
        body.push_str(&format!("\n[... {} more matches, truncated]", total - MAX_RESULTS));
    }
    if body.is_empty() {
        body = format!("(no matches for {})", args.pattern);
    }
    ToolResult::text(body)
}

fn walk_search(p: &Path, re: &Regex, glob: Option<&Regex>, ln: bool, out: &mut Vec<String>) {
    if out.len() >= MAX_RESULTS { return; }
    let meta = match std::fs::metadata(p) { Ok(m) => m, Err(_) => return };
    if meta.is_file() {
        if let Some(g) = glob {
            let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if !g.is_match(&name) { return; }
        }
        let content = match std::fs::read_to_string(p) { Ok(s) => s, Err(_) => return };
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                let formatted = if ln {
                    format!("{}:{}:{}", p.display(), i + 1, line)
                } else {
                    format!("{}:{}", p.display(), line)
                };
                out.push(formatted);
                if out.len() >= MAX_RESULTS { return; }
            }
        }
    } else if meta.is_dir() {
        let entries = match std::fs::read_dir(p) { Ok(e) => e, Err(_) => return };
        for ent in entries.flatten() {
            let name = ent.file_name();
            let name_s = name.to_string_lossy();
            if name_s.starts_with('.') { continue; }
            if SKIP_DIRS.contains(&name_s.as_ref()) { continue; }
            walk_search(&ent.path(), re, glob, ln, out);
            if out.len() >= MAX_RESULTS { return; }
        }
    }
}

fn translate_glob(g: &str) -> Result<Regex, String> {
    let mut re = String::from("^");
    for c in g.chars() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '.' | '+' | '(' | ')' | '|' | '[' | ']' | '\\' | '^' | '$' => {
                re.push('\\');
                re.push(c);
            }
            other => re.push(other),
        }
    }
    re.push('$');
    Regex::new(&re).map_err(|e| format!("invalid glob: {e}"))
}
