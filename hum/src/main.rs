//! `hum` — main user-facing CLI.
//!
//! Subcommands:
//!   hum                    health summary
//!   hum status             daemon + config + service state
//!   hum logs               tail journalctl (Linux) / launchd logs (macOS)
//!   hum doctor             one-shot full diagnostic dump (run this first)
//!   hum hive --list        list hive kinds (catalogue / configured / running)
//!   hum hive <ref> install build a hive + register its bee
//!   hum bee --list         list bees + state
//!   hum bee <id> VERB      enter | exit | reenter a bee (start/stop/restart)
//!   hum nest               list orchd-managed bees (delegates to `orchd status`)
//!   hum penny              show lifetime counters
//!   hum recipes [name]     list recipes / point at one
//!   hum thehum <verb>      inspect the persistent chi log
//!                          (status | tail | range | verify | replay)
//!   hum update             self-update from latest GitHub release
//!   hum uninstall          remove service + binary (state preserved)
//!   hum version            print version
//!   hum help               print this surface

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "hum", version, about = "hum — the AI stack on a biodiverse agentic kernel")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Daemon + config + service-manager state
    Status,
    /// Tail recent daemon logs (cross-platform)
    Logs {
        /// Number of lines to show (default: 200)
        #[arg(short = 'n', long, default_value_t = 200)]
        lines: u32,
    },
    /// One-shot full diagnostic dump: versions, config, env sanity,
    /// the claude binary, every bee + service state, and recent
    /// daemon + worker logs with warnings highlighted. Run this first
    /// when something is wrong; paste the output into a bug report.
    Doctor,
    /// Hive kinds — the source a bee is commissioned from.
    ///   hum hive --list           catalogue + configured + running
    ///   hum hive <ref> install    build the hive + register its bee
    /// <ref> is a bundled name, a local path, or the source URL a bee
    /// advertises (github tree URL of a hives/<kind> dir).
    Hive {
        /// Hive ref (name | path | source URL). Omit with --list.
        target: Option<String>,
        /// Action on the hive: `install`.
        action: Option<String>,
        /// List the hive catalogue.
        #[arg(long)]
        list: bool,
    },
    /// Bees — the running instances of a hive.
    ///   hum bee --list                  list bees + state
    ///   hum bee <name|id> enter         start a stopped bee
    ///   hum bee <name|id> exit          stop (state preserved)
    ///   hum bee <name|id> reenter       restart (graceful, same id)
    Bee {
        /// Bee name or id (hive name accepted, e.g. "claude-cli").
        target: Option<String>,
        /// Lifecycle verb: enter | exit | reenter.
        verb: Option<String>,
        /// List bees.
        #[arg(long)]
        list: bool,
    },
    /// List orchd-managed bees (delegates to `orchd status`).
    Nest,
    /// Show lifetime counters from penny.json
    Penny,
    /// List available recipes (recipes/*) or run one
    Recipes {
        /// Recipe name (e.g. "opencode"). Omit to list.
        name: Option<String>,
    },
    /// Inspect the persistent chi log (thehum).
    ///   hum thehum status                       dir, file count, seq, snapshot
    ///   hum thehum tail [-n N]                  most recent daily file (default 20)
    ///   hum thehum range --author <hid>         filter by author + seq range
    ///                    --from <seq> [--to <seq>]
    ///   hum thehum verify                       check hash chain + signatures
    ///   hum thehum replay                       count events by chi kind
    Thehum {
        #[command(subcommand)]
        verb: ThehumVerb,
    },
    /// Inspect or edit the peer-mesh state.
    ///   hum ensemble                          show our identity + reach + configured peers
    ///   hum ensemble peer add <humd_id>       append an entry to peers.json
    ///       --hint <tcp:host:port|iroh:hex>   (repeatable)
    ///       --alias <name>                    optional human-friendly name
    ///   hum ensemble peer rm <humd_id|alias>  drop matching entries
    Ensemble {
        #[command(subcommand)]
        verb: Option<EnsembleVerb>,
    },
    /// Stop the service and remove the humd binary. State preserved.
    Uninstall,
    /// Check for a newer release and self-update. Compares the local
    /// version against GitHub's latest release; if newer, re-runs the
    /// canonical install (which bounces the service atomically).
    Update {
        /// Update even when versions match (force reinstall).
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum EnsembleVerb {
    /// Manage entries in peers.json — bootstrap peers humd dials on boot.
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
}

#[derive(Subcommand)]
enum PeerAction {
    /// Append a peer to peers.json. Repeated `--hint` flags accumulate.
    /// Existing entries with the same humd_id are replaced (idempotent).
    Add {
        /// Peer humd_id (hex, with or without `humd_` prefix).
        humd_id: String,
        /// Dial hint, e.g. `tcp:host:port` or `iroh:<64-hex>`. Repeat for multiple.
        #[arg(long)]
        hint: Vec<String>,
        /// Optional alias for `hum://<alias>/path` URI resolution.
        #[arg(long)]
        alias: Option<String>,
    },
    /// Remove all entries matching `target` (by humd_id or alias).
    Rm {
        /// humd_id (full hex, with/without prefix) or alias.
        target: String,
    },
}

#[derive(Subcommand)]
enum ThehumVerb {
    /// Print dir, file count, total seq, latest snapshot height + ts.
    Status,
    /// Tail the most recent daily file as compact JSON, one event per line.
    Tail {
        /// Number of trailing events to show.
        #[arg(short = 'n', long, default_value_t = 20)]
        n: usize,
    },
    /// Filter events by author hid and seq range.
    Range {
        /// Author humd hid (hex).
        #[arg(long)]
        author: String,
        /// Inclusive lower bound on seq.
        #[arg(long)]
        from: u64,
        /// Inclusive upper bound on seq.
        #[arg(long)]
        to: Option<u64>,
    },
    /// Verify hash chain + signatures across the whole log.
    Verify,
    /// Replay the log, counting events by chi kind.
    Replay,
}

fn main() -> Result<()> {
    hum_paths::init();
    let cli = Cli::parse();
    match cli.cmd {
        None => summary(),
        Some(Cmd::Status) => status(),
        Some(Cmd::Logs { lines }) => logs(lines),
        Some(Cmd::Doctor) => doctor(),
        Some(Cmd::Hive { target, action, list }) => hive(target, action, list),
        Some(Cmd::Bee { target, verb, list }) => bee(target, verb, list),
        Some(Cmd::Nest) => nest(),
        Some(Cmd::Penny) => penny(),
        Some(Cmd::Recipes { name }) => recipes(name),
        Some(Cmd::Thehum { verb }) => thehum_cmd(verb),
        Some(Cmd::Ensemble { verb }) => ensemble(verb),
        Some(Cmd::Uninstall) => uninstall(),
        Some(Cmd::Update { force }) => update(force),
    }
}

fn update(force: bool) -> Result<()> {
    // Resolve "what version is upstream" via the GitHub API. Compare
    // to ours (Cargo.toml version, bumped by scripts/version.sh).
    // Re-running the canonical installer takes care of building +
    // service-bouncing in one move — no need to special-case binary
    // swaps here.
    let local = env!("CARGO_PKG_VERSION").to_string();
    let upstream = match latest_release_tag() {
        Some(t) => t,
        None => {
            eprintln!("could not reach github.com; skipping update");
            return Ok(());
        }
    };
    let upstream_trim = upstream.trim_start_matches('v').to_string();
    println!("local: {local}  upstream: {upstream_trim}");
    if !force && upstream_trim == local {
        println!("up to date.");
        return Ok(());
    }
    println!("updating to {upstream_trim} …");
    // Canonical installer URL is the single source of truth. It pulls
    // source, builds, bounces the service via the installer.
    let url = "https://raw.githubusercontent.com/adiled/hum/main/install";
    let status = Command::new("bash")
        .arg("-c")
        .arg(format!("curl -fsSL {url} | bash"))
        .status()?;
    if !status.success() {
        anyhow::bail!("installer exited with {status}");
    }
    println!("update complete.");
    Ok(())
}

/// Best-effort latest-release fetch. Returns None on any network /
/// parse failure so callers can degrade gracefully (e.g. a cron-fired
/// update that runs while offline shouldn't error-spam the journal).
fn latest_release_tag() -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-H", "Accept: application/vnd.github+json",
            "https://api.github.com/repos/adiled/hum/releases/latest",
        ])
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let body = String::from_utf8(out.stdout).ok()?;
    // Tiny grep for `"tag_name":"vX.Y.Z"` — avoids pulling a full
    // JSON parser into the CLI for one field.
    let needle = "\"tag_name\":";
    let start = body.find(needle)? + needle.len();
    let rest = &body[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')?;
    Some(rest[q1..q1+q2].to_string())
}

// ─── helpers ─────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn probe_thrum(sock: &Path) -> Result<()> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    if !sock.exists() {
        anyhow::bail!("socket file missing (humd not running)");
    }
    let mut s = UnixStream::connect(sock)
        .map_err(|e| anyhow::anyhow!("connect refused ({e}) — stale socket, humd crashed"))?;
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    s.set_write_timeout(Some(Duration::from_secs(1)))?;
    s.write_all(b"{\"chi\":\"hello\",\"sid\":\"hum-doctor-probe\",\"bee\":[\"worker\"]}\n")?;
    let mut buf = [0u8; 256];
    match s.read(&mut buf) {
        Ok(0) => anyhow::bail!("socket closed without breath"),
        Ok(_) => Ok(()),
        Err(e) => anyhow::bail!("no breath within 1s ({e})"),
    }
}

fn humd_bin() -> Result<PathBuf> {
    let candidates = [
        std::env::var_os("HUM_BIN").map(PathBuf::from),
        Some(hum_paths::humd_bin()),
    ];
    for c in candidates.into_iter().flatten() {
        if c.exists() { return Ok(c); }
    }
    anyhow::bail!("humd binary not found (set HUM_BIN or run ./install)")
}

// ─── subcommands ─────────────────────────────────────────────────────────

fn summary() -> Result<()> {
    println!("hum {} — `hum help` for the surface", env!("CARGO_PKG_VERSION"));
    status()
}

fn status() -> Result<()> {
    let thrum_sock = hum_paths::thrum_sock_resolved();

    let bin = humd_bin().ok();
    let bin_display = bin.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(missing)".into());

    println!("humd binary:  {bin_display}");
    if let Some(b) = &bin {
        let v = Command::new(b).arg("--version").output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "?".into());
        println!("  version:    {v}");
    }
    let humd_key = hum_paths::humd_key();
    let peers_json = hum_paths::peers_json();
    let hum_json = hum_paths::hum_json();
    println!("identity:     {} {}", humd_key.display(), yn(humd_key.exists()));
    println!("peers.json:   {} {}", peers_json.display(), yn(peers_json.exists()));
    println!("hum.json:     {} {}", hum_json.display(), yn(hum_json.exists()));
    println!("thrum socket: {} {}", thrum_sock.display(),
             yn(std::fs::metadata(&thrum_sock).is_ok()));

    Ok(())
}

fn yn(b: bool) -> &'static str { if b { "✓" } else { "missing" } }

/// `hum ensemble` — inspect or edit on-disk peer-mesh state.
fn ensemble(verb: Option<EnsembleVerb>) -> Result<()> {
    match verb {
        None => ensemble_show(),
        Some(EnsembleVerb::Peer { action }) => match action {
            PeerAction::Add { humd_id, hint, alias } => ensemble_peer_add(&humd_id, hint, alias),
            PeerAction::Rm { target } => ensemble_peer_rm(&target),
        },
    }
}

/// Default display — our identity + reach hints + configured peers.
/// Live peer state lives in the running daemon and needs an admin-tone
/// RPC that isn't wired yet.
fn ensemble_show() -> Result<()> {
    match humd::read_key()? {
        Some(key) => println!("me:      {}", key.hid()),
        None => println!("me:      (no identity — humd has never booted)"),
    }

    let reach = hum_paths::RuntimeInfo::read()
        .map(|info| info.ensemble_addrs)
        .unwrap_or_default();
    if reach.is_empty() {
        println!("reach:   (humd not running, or no transport bound)");
    } else {
        println!("reach:   (paste into a peer's peers.json hints)");
        for h in &reach {
            println!("           {h}");
        }
    }

    let peers_json = hum_paths::peers_json();
    if !peers_json.exists() {
        println!("peers:   (peers.json not present at {})", peers_json.display());
        return Ok(());
    }
    let raw = std::fs::read_to_string(&peers_json)?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", peers_json.display()))?;
    let entries = parsed.get("peers").and_then(|v| v.as_array());
    match entries {
        Some(es) if !es.is_empty() => {
            println!("peers:   ({} configured in {})", es.len(), peers_json.display());
            for (i, e) in es.iter().enumerate() {
                let id = e.get("humd_id").and_then(|v| v.as_str()).unwrap_or("(no id)");
                let alias = e.get("alias").and_then(|v| v.as_str()).unwrap_or("");
                let hints: Vec<&str> = e.get("hints")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|h| h.as_str()).collect())
                    .unwrap_or_default();
                let label = if alias.is_empty() { id.to_string() } else { format!("{alias}  {id}") };
                println!("  [{i}] {label}");
                for h in &hints {
                    println!("        {h}");
                }
            }
        }
        _ => println!("peers:   (none configured in {})", peers_json.display()),
    }
    Ok(())
}

/// Read peers.json into `(file, peers_array)`. Returns an empty file
/// shape if peers.json is missing — the caller will create it on write.
fn peers_load() -> Result<(serde_json::Map<String, serde_json::Value>, Vec<serde_json::Value>)> {
    let path = hum_paths::peers_json();
    if !path.exists() {
        return Ok((serde_json::Map::new(), Vec::new()));
    }
    let raw = std::fs::read_to_string(&path)?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    let mut obj = parsed.as_object().cloned().unwrap_or_default();
    let peers = obj
        .remove("peers")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    Ok((obj, peers))
}

/// Atomic write — tmp + rename, mkdir -p, preserves unknown top-level
/// keys the caller carried over from peers_load.
fn peers_save(
    mut other_fields: serde_json::Map<String, serde_json::Value>,
    peers: Vec<serde_json::Value>,
) -> Result<()> {
    let path = hum_paths::peers_json();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    other_fields.insert("peers".into(), serde_json::Value::Array(peers));
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(other_fields))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn normalize_humd_id(s: &str) -> String {
    // Strip a leading `humd_` if present; humd's loader accepts both
    // shapes, but storing one canonical form keeps the file readable.
    s.strip_prefix("humd_").unwrap_or(s).to_string()
}

fn ensemble_peer_add(humd_id: &str, hints: Vec<String>, alias: Option<String>) -> Result<()> {
    if hints.is_empty() {
        anyhow::bail!("at least one --hint required (e.g. --hint tcp:host:port or --hint iroh:<64-hex>)");
    }
    let canonical_id = normalize_humd_id(humd_id);
    let hex_only = canonical_id.chars().all(|c| c.is_ascii_hexdigit());
    if !hex_only || canonical_id.len() != 64 {
        anyhow::bail!(
            "humd_id `{humd_id}` doesn't look like 64-hex (optionally prefixed `humd_`)"
        );
    }

    let (other_fields, mut peers) = peers_load()?;
    let before = peers.len();
    peers.retain(|p| p.get("humd_id").and_then(|v| v.as_str()) != Some(&canonical_id));
    let replaced = before != peers.len();

    let mut entry = serde_json::Map::new();
    entry.insert("humd_id".into(), serde_json::Value::String(canonical_id.clone()));
    entry.insert(
        "hints".into(),
        serde_json::Value::Array(hints.into_iter().map(serde_json::Value::String).collect()),
    );
    if let Some(a) = alias {
        entry.insert("alias".into(), serde_json::Value::String(a));
    }
    peers.push(serde_json::Value::Object(entry));
    peers_save(other_fields, peers)?;

    let path = hum_paths::peers_json();
    if replaced {
        println!("peers.json: replaced entry for humd_{} in {}", &canonical_id[..16], path.display());
    } else {
        println!("peers.json: added humd_{} → {}", &canonical_id[..16], path.display());
    }
    println!("(restart humd for the new entry to take effect)");
    Ok(())
}

fn ensemble_peer_rm(target: &str) -> Result<()> {
    let canonical = normalize_humd_id(target);
    let (other_fields, peers) = peers_load()?;
    let before = peers.len();
    let kept: Vec<_> = peers
        .into_iter()
        .filter(|p| {
            let id = p.get("humd_id").and_then(|v| v.as_str()).unwrap_or("");
            let alias = p.get("alias").and_then(|v| v.as_str()).unwrap_or("");
            id != canonical && alias != target
        })
        .collect();
    let removed = before - kept.len();
    peers_save(other_fields, kept)?;
    let path = hum_paths::peers_json();
    if removed == 0 {
        println!("peers.json: no entry matching `{target}` in {}", path.display());
    } else {
        println!("peers.json: removed {removed} entr{} from {}", if removed == 1 { "y" } else { "ies" }, path.display());
        println!("(restart humd for the removal to take effect)");
    }
    Ok(())
}

fn logs(lines: u32) -> Result<()> {
    match hum_paths::daemon_logs("humd") {
        hum_paths::DaemonLogs::Journald { unit } => {
            Command::new("journalctl")
                .args(["--user", "-u", &unit, "--no-pager", "-n", &lines.to_string()])
                .status()?;
        }
        hum_paths::DaemonLogs::Files { stdout, stderr } => {
            Command::new("tail").args(["-n", &lines.to_string()])
                .arg(stdout).arg(stderr).status()?;
        }
    }
    Ok(())
}

fn doctor() -> Result<()> {
    let bar = "────────────────────────────────────────────────────────";
    println!("{bar}\nhum doctor\n{bar}");

    // 1. Versions + platform.
    println!("\n[versions]");
    println!("  hum CLI:    {}", env!("CARGO_PKG_VERSION"));
    if let Ok(b) = humd_bin() {
        let v = Command::new(&b).arg("--version").output().ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string()).unwrap_or_else(|| "?".into());
        println!("  humd:       {v}  ({})", b.display());
    } else {
        println!("  humd:       NOT FOUND (set HUM_BIN or run ./install)");
    }
    println!("  os:         {} {}", std::env::consts::OS, std::env::consts::ARCH);

    // 2. Config + state files.
    let hum_json = hum_paths::hum_json();
    let peers_json = hum_paths::peers_json();
    let humd_key = hum_paths::humd_key();
    println!("\n[config + state]");
    println!("  hum.json:   {} {}", hum_json.display(), yn(hum_json.exists()));
    println!("  peers.json: {} {}", peers_json.display(), yn(peers_json.exists()));
    println!("  identity:   {} {}", humd_key.display(), yn(humd_key.exists()));

    // 3. hum.json lint — catches the config drift that silently breaks
    //    routing (the keys humd ignores, stale section names, a default
    //    pointing nowhere). These parse fine but do nothing.
    println!("\n[hum.json schema validation]");
    match std::fs::read_to_string(&hum_json) {
        Err(_) => println!("  (no hum.json — humd runs on defaults)"),
        Ok(raw) => match config::validate(&raw) {
            Ok(()) => println!("  ✓ valid against hum.schema.json"),
            Err(violations) => {
                println!("  ✗ INVALID — humd will refuse to start:");
                for v in &violations { println!("      - {v}"); }
            }
        },
    }

    // 4. Bee identities — the persisted keys that back hid dedup. A
    //    missing or wrong-size key means a bee can't keep a stable hid
    //    across reconnects (ghost-manifest accumulation).
    let bees_dir = hum_paths::bees_dir();
    println!("\n[bee identities]  ({})", bees_dir.display());
    match std::fs::read_dir(&bees_dir) {
        Err(_) => println!("  (none yet — minted on first bee boot)"),
        Ok(entries) => {
            let mut any = false;
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("key") {
                    any = true;
                    let kind = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    println!("  {kind}: {}", if sz == 32 { "✓ 32-byte ed25519 seed".to_string() } else { format!("✗ {sz} bytes (expected 32 — corrupt key)") });
                }
            }
            if !any { println!("  (none yet)"); }
        }
    }

    // 5. Env sanity — the macOS traps live here.
    println!("\n[env sanity]");
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let runtime_exists = std::path::Path::new(&runtime).is_dir();
    println!("  XDG_RUNTIME_DIR: {runtime} {}", if runtime_exists { "✓" } else { "✗ DOES NOT EXIST (penny writes will fail — common macOS trap when set to a Linux /run/user path)" });

    match hum_paths::RuntimeInfo::read() {
        Some(rt) => {
            let age_s = (now_ms() as i64 - rt.bound_at_ms as i64).max(0) / 1000;
            println!("  runtime.json: ✓ pid={} version={} bound {}s ago", rt.pid, rt.version, age_s);
        }
        None => println!("  runtime.json: ✗ MISSING (humd has not published a rendezvous; either not running, or pre-0.31.19)"),
    }

    let sock = hum_paths::thrum_sock_resolved();
    match probe_thrum(&sock) {
        Ok(()) => println!("  thrum sock: {} ✓ live", sock.display()),
        Err(e) => println!("  thrum sock: {} ✗ {e}", sock.display()),
    }

    match Command::new("orchd").arg("--version").output() {
        Ok(o) if o.status.success() => println!("  orchd:      ✓ {}", String::from_utf8_lossy(&o.stdout).trim()),
        _ => println!("  orchd:      ✗ NOT FOUND in PATH (run ./install to build it)"),
    }

    // 4. The claude binary (worker's compute).
    println!("\n[claude binary]");
    let claude = std::env::var("CLAUDE_CLI_PATH").unwrap_or_else(|_| "claude".into());
    match Command::new(&claude).arg("--version").output() {
        Ok(o) if o.status.success() => println!("  {claude}: {}", String::from_utf8_lossy(&o.stdout).trim()),
        Ok(_) | Err(_) => println!("  {claude}: ✗ NOT RUNNABLE — set CLAUDE_CLI_PATH to the real binary"),
    }

    println!("\n[bees]");
    let _ = bee_list_full(&orch_catalog());

    // 6. Recent logs with warnings/errors surfaced. This is where the
    //    real failures show (worker.result.error, bee.hid.*, spawn fails).
    println!("\n[recent humd + worker logs — warnings/errors]");
    print_recent_logs("hum", 60);
    print_recent_logs("hum-claude-cli-worker", 60);

    println!("\n{bar}");
    println!("If a bee shows 0 tokens / silent finish, look for `worker.result.error`");
    println!("above — claude reports auth/model/credit failures there, not on stderr.");
    Ok(())
}

fn print_recent_logs(unit: &str, lines: u32) {
    let raw_cmd = match hum_paths::daemon_logs(unit) {
        hum_paths::DaemonLogs::Journald { unit: u } =>
            format!("journalctl --user -u {u} --no-pager -n {lines} 2>/dev/null"),
        hum_paths::DaemonLogs::Files { stdout, stderr } =>
            format!("tail -n {lines} {} {} 2>/dev/null", stdout.display(), stderr.display()),
    };
    let script = format!(
        "{raw_cmd} | grep -iE 'WARN|ERROR|result.error|bee\\.hid|spawn|panic|fail' | tail -15"
    );
    println!("  ── {unit} ──");
    let out = Command::new("bash").arg("-c").arg(&script).output();
    match out {
        Ok(o) => {
            let txt = String::from_utf8_lossy(&o.stdout);
            if txt.trim().is_empty() { println!("    (no warnings/errors in last {lines} lines)"); }
            else { for l in txt.lines() { println!("    {l}"); } }
        }
        Err(_) => println!("    (logs unavailable)"),
    }
}

// ── hive / bee plumbing ─────────────────────────────────────────────────

fn hive(target: Option<String>, action: Option<String>, list: bool) -> Result<()> {
    // hum hive --list  (or bare `hum hive`)
    if list || target.is_none() {
        return hive_list();
    }
    let ref_ = target.unwrap();
    match action.as_deref() {
        Some("install") => hive_install(&ref_),
        Some(act) => anyhow::bail!("unknown hive action '{act}' for '{ref_}' (try: install)"),
        None => anyhow::bail!("hum hive {ref_} <action> — try: hum hive {ref_} install"),
    }
}

fn hive_list() -> Result<()> {
    use std::collections::BTreeMap;
    // kind -> (has installer, configured model, running)
    let root = repo_root_or_install_dir();
    let hives_dir = root.join(hum_paths::HIVES_SUBDIR);
    let mut kinds: BTreeMap<String, (bool, Option<String>, bool)> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(&hives_dir) {
        for e in entries.flatten() {
            if e.path().is_dir() && e.path().join(hum_paths::HIVE_INSTALL_SCRIPT).exists() {
                kinds.entry(e.file_name().to_string_lossy().to_string()).or_default().0 = true;
            }
        }
    }
    let hum_json = hum_paths::hum_json();
    let mut default_kind = String::new();
    if let Ok(raw) = std::fs::read_to_string(&hum_json) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            default_kind = v.get("nest").and_then(|n| n.get("default"))
                .and_then(|d| d.as_str()).unwrap_or("").to_string();
            if let Some(obj) = v.get("hives").and_then(|h| h.as_object()) {
                for (k, cfg) in obj {
                    kinds.entry(k.clone()).or_default().1 =
                        cfg.get("defaultModel").and_then(|m| m.as_str()).map(str::to_string);
                }
            }
        }
    }
    for kind in orch_catalog() {
        kinds.entry(kind).or_default().2 = true;
    }
    if kinds.is_empty() {
        println!("no hives found (looked in {})", hives_dir.display());
        return Ok(());
    }
    println!("Hive kinds (catalogue: {}):\n", hives_dir.display());
    println!("  {:<18} {:<10} {:<20} {}", "KIND", "INSTALLER", "CONFIGURED", "RUNNING");
    for (kind, (installer, model, running)) in &kinds {
        let configured = match (model, kind == &default_kind) {
            (Some(m), true)  => format!("{m} (default)"),
            (Some(m), false) => m.clone(),
            (None, true)     => "(default)".to_string(),
            (None, false)    => "—".to_string(),
        };
        println!("  {:<18} {:<10} {:<20} {}", kind,
            if *installer { "✓" } else { "—" }, configured,
            if *running { "✓" } else { "—" });
    }
    println!("\nbuild one: hum hive <name|path|source-url> install   |   bees: hum bee --list");
    Ok(())
}

/// Resolve a hive ref to its `install` script, then run it. <ref> is the
/// same dialect a bee advertises as its `source`:
///   - bundled name   → <repo>/hives/<name>/install
///   - local path     → <dir>/install  (or a direct install file)
///   - github tree URL → https://github.com/<org>/<repo>/tree/<branch>/<sub>
///                       our own repo maps to the local checkout; a
///                       foreign repo is shallow-cloned to a cache.
fn hive_install(reference: &str) -> Result<()> {
    let dir = resolve_hive_dir(reference)?;
    let orchfile = dir.join(hum_paths::ORCHFILE_BASENAME);
    if !orchfile.exists() {
        anyhow::bail!("no Orchfile at {}", orchfile.display());
    }
    let kind = read_orchfile_service(&orchfile)?
        .ok_or_else(|| anyhow::anyhow!("no SERVICE directive in {}", orchfile.display()))?;

    build_hive(&dir, &kind)?;

    let orch_d = hum_paths::orch_d_dir();
    std::fs::create_dir_all(&orch_d)?;
    let dest = orch_d.join(format!("{kind}.orch"));
    std::fs::copy(&orchfile, &dest)?;
    println!("registered {kind} ({})", dest.display());

    rewrite_hum_orchfile(&orch_d)?;

    let s = orchd_cmd().arg("up").arg(&kind).status()
        .map_err(|e| anyhow::anyhow!("orchd not found: {e}"))?;
    if !s.success() { anyhow::bail!("orchd up {kind} failed"); }
    println!("✓ {kind} entered; see `hum bee --list`");
    Ok(())
}

fn build_hive(dir: &Path, kind: &str) -> Result<()> {
    let custom = dir.join("build");
    if custom.is_file() {
        println!("building {kind} (./build in {}) ...", dir.display());
        let s = Command::new("bash").arg(&custom).current_dir(dir).status()?;
        if !s.success() { anyhow::bail!("custom build script failed: {}", custom.display()); }
        return Ok(());
    }
    if dir.join("Cargo.toml").exists() { return build_cargo(dir, kind); }
    if dir.join("package.json").exists() { return build_node(dir, kind); }
    if dir.join("go.mod").exists() { return build_go(dir, kind); }
    anyhow::bail!("no Cargo.toml / package.json / go.mod / build in {}", dir.display());
}

fn build_cargo(dir: &Path, kind: &str) -> Result<()> {
    println!("building {kind} (cargo install --path {}) ...", dir.display());
    let s = Command::new("cargo")
        .args(["install", "--quiet", "--locked", "--path"]).arg(dir)
        .args(["--root"]).arg(hum_paths::local_dir())
        .arg("--force")
        .status()?;
    if !s.success() { anyhow::bail!("cargo install failed for {}", dir.display()); }
    Ok(())
}

fn build_node(dir: &Path, kind: &str) -> Result<()> {
    let dist = dir.join("dist").join("index.js");
    let pkg_mgr = ["pnpm", "npm"].iter().find(|m| which(m)).copied();
    if let Some(mgr) = pkg_mgr {
        println!("building {kind} ({mgr} install + build) in {}", dir.display());
        let s = Command::new(mgr).arg("install").current_dir(dir).status()?;
        if !s.success() { anyhow::bail!("{mgr} install failed"); }
        let s = Command::new(mgr).args(["run", "build"]).current_dir(dir).status()?;
        if !s.success() { anyhow::bail!("{mgr} run build failed"); }
    } else if dist.exists() {
        println!("using pre-built {} (no pnpm/npm in PATH)", dist.display());
    } else {
        anyhow::bail!("no pnpm/npm in PATH and no prebuilt {}", dist.display());
    }
    if !dist.exists() {
        anyhow::bail!("build did not produce {}", dist.display());
    }
    let node = which_first(&["node", "/usr/local/bin/node"])
        .or_else(|| Some(hum_paths::fnm_node_bin()).filter(|p| p.exists()))
        .ok_or_else(|| anyhow::anyhow!("node not in PATH; install Node 22+"))?;
    let bin = hum_paths::hum_bin(kind);
    std::fs::create_dir_all(bin.parent().unwrap())?;
    let wrapper = format!(
        "#!/usr/bin/env bash\nexec {} {} \"$@\"\n",
        node.display(), dist.display(),
    );
    std::fs::write(&bin, wrapper)?;
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(&bin)?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&bin, perm)?;
    println!("wrote node wrapper at {}", bin.display());
    Ok(())
}

fn build_go(dir: &Path, kind: &str) -> Result<()> {
    if !which("go") { anyhow::bail!("go not in PATH; install Go"); }
    let bin = hum_paths::hum_bin(kind);
    std::fs::create_dir_all(bin.parent().unwrap())?;
    println!("building {kind} (go build) in {}", dir.display());
    let s = Command::new("go").args(["build", "-o"]).arg(&bin).arg(".").current_dir(dir).status()?;
    if !s.success() { anyhow::bail!("go build failed"); }
    Ok(())
}

fn which(name: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status().map(|s| s.success()).unwrap_or(false)
}

fn which_first(candidates: &[&str]) -> Option<PathBuf> {
    for c in candidates {
        if c.starts_with('/') {
            let p = PathBuf::from(c);
            if p.exists() { return Some(p); }
        } else if which(c) {
            return Some(PathBuf::from(c));
        }
    }
    None
}

fn read_orchfile_service(path: &Path) -> Result<Option<String>> {
    let raw = std::fs::read_to_string(path)?;
    Ok(raw.lines()
        .filter_map(|l| l.trim().strip_prefix("SERVICE ").map(|s| s.trim().to_string()))
        .next())
}

fn rewrite_hum_orchfile(orch_d: &Path) -> Result<()> {
    let mut combined = String::new();
    let mut entries: Vec<_> = std::fs::read_dir(orch_d)?.flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("orch"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let body = std::fs::read_to_string(e.path())?;
        combined.push_str(&body);
        if !combined.ends_with('\n') { combined.push('\n'); }
        combined.push('\n');
    }
    std::fs::write(hum_paths::orchfile(), combined)?;
    Ok(())
}

fn resolve_hive_dir(reference: &str) -> Result<PathBuf> {
    if let Some(rest) = reference.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(5, '/').collect();
        if parts.len() == 5 && parts[2] == "tree" {
            let (org, repo, branch, sub) = (parts[0], parts[1], parts[3], parts[4]);
            if org == "adiled" && repo == "hum" {
                return Ok(repo_root_or_install_dir().join(sub));
            }
            let cache = hum_paths::foreign_hive_cache(org, repo, branch);
            if !cache.exists() {
                std::fs::create_dir_all(cache.parent().unwrap()).ok();
                let url = format!("https://github.com/{org}/{repo}");
                println!("cloning {url} @ {branch} ...");
                let ok = Command::new("git")
                    .args(["clone", "--depth", "1", "--branch", branch, &url])
                    .arg(&cache).status().map(|s| s.success()).unwrap_or(false);
                if !ok { anyhow::bail!("git clone failed: {url}"); }
            }
            return Ok(cache.join(sub));
        }
        anyhow::bail!("unrecognized github source URL (want .../tree/<branch>/<path>): {reference}");
    }
    let p = PathBuf::from(reference);
    if p.is_dir() { return Ok(p); }
    let bundled = repo_root_or_install_dir().join(hum_paths::HIVES_SUBDIR).join(reference);
    if bundled.exists() { return Ok(bundled); }
    anyhow::bail!("can't resolve hive '{reference}' (not a bundled name, path, or github source URL)");
}

fn bee_list_full(installed: &[String]) -> Result<()> {
    let snap_path = hum_paths::bees_snapshot();
    let live: Vec<serde_json::Value> = std::fs::read_to_string(&snap_path).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_object().map(|o| o.values().cloned().collect()))
        .unwrap_or_default();

    if live.is_empty() && installed.is_empty() {
        println!("no bees connected and no bee services installed.");
        println!("build one: hum hive <name|path|source-url> install");
        return Ok(());
    }

    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let arr = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();

    let mut matched_kinds: Vec<String> = Vec::new();
    for m in &live {
        let hive = s(m, "name");
        let managed = installed.iter().any(|k| k == &hive || hive.starts_with(&format!("{k}-")));
        if managed { matched_kinds.push(hive.clone()); }

        let role = arr(m, "bee").iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>().join("+");
        let models = arr(m, "models").iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>();
        let tools: Vec<String> = arr(m, "tools").iter().map(|t| s(t, "name")).filter(|x| !x.is_empty()).collect();
        let provides = arr(m, "provides").iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>();
        let wire = m.get("propensity").map(|p| s(p, "wire")).unwrap_or_default();
        let state = if managed { "in nest (orchd-managed)" } else { "in nest (unmanaged)" };

        println!("● {hive}  —  {state}");
        let hid = s(m, "hid");
        if !hid.is_empty() { println!("    hid:      {}", hid); }
        if !role.is_empty()     { println!("    role:     {role}"); }
        if !models.is_empty()   { println!("    models:   {}", models.join(", ")); }
        if !tools.is_empty()    { println!("    tools:    {} ({})", tools.len(), tools.join(", ")); }
        if !provides.is_empty() { println!("    provides: {}", provides.join(", ")); }
        if !wire.is_empty()     { println!("    wire:     {wire}"); }
        let version = s(m, "version");
        if !version.is_empty()  { println!("    version:  {version}"); }
        let source = s(m, "source");
        if !source.is_empty()   { println!("    source:   {source}"); }
        println!();
    }

    for kind in installed {
        if matched_kinds.contains(kind) { continue; }
        println!("● {kind}  —  installed, not handshaked");
        println!();
    }

    println!("verbs: hum bee <id> enter | exit | reenter   (id `all` for every bee)");
    Ok(())
}

fn bee(target: Option<String>, verb: Option<String>, list: bool) -> Result<()> {
    let installed = orch_catalog();

    if list || (target.is_none() && verb.is_none()) {
        return bee_list_full(&installed);
    }

    let (target, verb) = match (target, verb) {
        (Some(t), Some(v)) => (t, v),
        (Some(t), None) => anyhow::bail!("hum bee {t} <verb> — enter | exit | reenter"),
        _ => anyhow::bail!("hum bee <id> <verb>, or hum bee --list"),
    };
    if !orch_route_verb(&target, &verb)? {
        anyhow::bail!("no bee matching '{target}'. bees: {}",
            if installed.is_empty() { "(none)".into() } else { installed.join(", ") });
    }
    Ok(())
}

fn penny() -> Result<()> {
    let path = hum_paths::penny();
    if !path.exists() {
        println!("no penny.json yet ({})", path.display());
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    println!("{raw}");
    Ok(())
}

fn recipes(name: Option<String>) -> Result<()> {
    let root = repo_root_or_install_dir();
    let recipes_dir = root.join(hum_paths::RECIPES_SUBDIR);
    if !recipes_dir.exists() {
        println!("no recipes/ dir at {}", recipes_dir.display());
        return Ok(());
    }
    match name {
        None => {
            println!("Available recipes (in {}):", recipes_dir.display());
            for entry in std::fs::read_dir(&recipes_dir)? {
                let entry = entry?;
                if entry.path().is_dir() {
                    println!("  {}", entry.file_name().to_string_lossy());
                }
            }
            println!();
            println!("Run one with: hum recipes <name>");
        }
        Some(n) => {
            let install = recipes_dir.join(&n).join(hum_paths::HIVE_INSTALL_SCRIPT);
            if !install.exists() {
                anyhow::bail!("recipes/{n}/install not found");
            }
            Command::new(install).status()?;
        }
    }
    Ok(())
}

/// Find the repo root (running from a clone) or the rsynced source dir
/// (paradigm 2 installation). Returns whatever exists.
fn repo_root_or_install_dir() -> PathBuf {
    // Same exe dir → likely a clone; check for Cargo.toml.
    if let Ok(exe) = std::env::current_exe() {
        let mut p = exe.clone();
        while let Some(parent) = p.parent() {
            if parent.join("Cargo.toml").exists() {
                return parent.to_path_buf();
            }
            p = parent.to_path_buf();
        }
    }
    {
        let candidate = hum_paths::src_dir();
        if candidate.exists() { return candidate; }
    }
    PathBuf::from(".")
}

// ── orchd shell-outs (bee lifecycle) ─────────────────────────────────────


fn orchd_cmd() -> Command {
    // Prefer the absolute path the installer wrote, so a missing
    // ~/.local/bin on $PATH doesn't break `hum hive install` /
    // `hum bee` / `hum nest`. Fall back to bare "orchd" so users who
    // installed orchd elsewhere (or via a package manager) still work.
    let abs = hum_paths::hum_bin("orchd");
    let mut c = if abs.exists() { Command::new(&abs) } else { Command::new("orchd") };
    c.arg("--orchfile").arg(hum_paths::orchfile())
     .arg("--user")
     .arg("--namespace").arg("hum");
    c
}

/// Service names declared in hum's Orchfile.
fn orch_catalog() -> Vec<String> {
    let path = hum_paths::orchfile();
    let Ok(raw) = std::fs::read_to_string(&path) else { return Vec::new(); };
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix("SERVICE ").map(|s| s.trim().to_string()))
        .collect()
}

fn nest() -> Result<()> {
    let status = orchd_cmd().arg("status").status()
        .map_err(|e| anyhow::anyhow!("orchd not found: {e}"))?;
    if !status.success() {
        anyhow::bail!("orchd status failed");
    }
    Ok(())
}

/// Route enter/exit/reenter through orchd. Returns Ok(true) if orchd
/// handled the verb, Ok(false) if the kind is not in orchd's catalog.
fn orch_route_verb(kind: &str, verb: &str) -> Result<bool> {
    if !orch_catalog().iter().any(|k| k == kind) {
        return Ok(false);
    }
    let verb_arg = match verb {
        "enter"   => "up",
        "exit"    => "down",
        "reenter" => "restart",
        other => anyhow::bail!("unknown verb '{other}' (enter | exit | reenter)"),
    };
    let past = match verb { "enter" => "entered", "exit" => "exited", _ => "re-entered" };
    let status = orchd_cmd().arg(verb_arg).arg(kind).status()
        .map_err(|e| anyhow::anyhow!("orchd not found: {e}"))?;
    if !status.success() {
        anyhow::bail!("orchd {verb_arg} {kind} failed");
    }
    println!("  ✓ {past} {kind} (orchd)");
    Ok(true)
}

fn uninstall() -> Result<()> {
    let _ = Command::new("humctl").arg("stop").status();
    if let Ok(bin) = humd_bin() {
        let _ = std::fs::remove_file(&bin);
        println!("removed {}", bin.display());
    }
    println!("state preserved. `./install purge` to wipe.");
    Ok(())
}

// ── thehum: persistent chi-log inspector ─────────────────────────────────

fn thehum_cmd(verb: ThehumVerb) -> Result<()> {
    match verb {
        ThehumVerb::Status => thehum_status(),
        ThehumVerb::Tail { n } => thehum_tail(n),
        ThehumVerb::Range { author, from, to } => thehum_range(&author, from, to),
        ThehumVerb::Verify => thehum_verify(),
        ThehumVerb::Replay => thehum_replay(),
    }
}

/// Ndjson files in the thehum dir, sorted lexicographically (YYYY-MM-DD).
fn thehum_ndjson_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("readdir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ndjson"))
        .collect();
    files.sort();
    Ok(files)
}

/// Deserialize every well-formed ndjson line into a thehum::Event.
fn thehum_load_all(dir: &Path) -> Result<Vec<thehum::Event>> {
    let mut out = Vec::new();
    for path in thehum_ndjson_files(dir)? {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        for (i, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let ev: thehum::Event = serde_json::from_str(line)
                .with_context(|| format!("{}:{}: malformed event", path.display(), i + 1))?;
            out.push(ev);
        }
    }
    Ok(out)
}

fn thehum_status() -> Result<()> {
    let dir = hum_paths::thehum_dir();
    if !dir.exists() {
        println!("thehum dir:    {} (does not exist yet)", dir.display());
        return Ok(());
    }
    let files = thehum_ndjson_files(&dir)?;
    let seq = std::fs::read(hum_paths::thehum_seq_file(&dir)).ok().and_then(|b| {
        if b.len() == 8 {
            let mut a = [0u8; 8];
            a.copy_from_slice(&b);
            Some(u64::from_le_bytes(a))
        } else { None }
    }).unwrap_or(0);

    // Last chi=="snapshot" wins.
    let mut latest_height: Option<u64> = None;
    let mut latest_ts_ms: Option<i64> = None;
    let events = thehum_load_all(&dir).unwrap_or_default();
    for e in &events {
        if e.chi == "snapshot" {
            latest_height = e.body.get("height").and_then(|v| v.as_u64()).or(latest_height);
            latest_ts_ms = Some(e.ts_ms);
        }
    }

    println!("thehum dir:        {}", dir.display());
    println!("daily files:       {}", files.len());
    println!("total seq:         {seq}");
    match (latest_height, latest_ts_ms) {
        (Some(h), Some(ts)) => println!("latest snapshot:   height={h} ts_ms={ts} ({})", fmt_ts_ms(ts)),
        _ => println!("latest snapshot:   (none)"),
    }
    Ok(())
}

fn thehum_tail(n: usize) -> Result<()> {
    let dir = hum_paths::thehum_dir();
    let files = thehum_ndjson_files(&dir)?;
    let Some(last) = files.last() else {
        println!("(no ndjson files in {})", dir.display());
        return Ok(());
    };
    let content = std::fs::read_to_string(last)
        .with_context(|| format!("read {}", last.display()))?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    for line in &lines[start..] {
        // Re-emit compactly via Event round-trip.
        match serde_json::from_str::<thehum::Event>(line) {
            Ok(ev) => println!("{}", serde_json::to_string(&ev).unwrap_or_else(|_| (*line).to_string())),
            Err(_) => println!("{line}"),
        }
    }
    Ok(())
}

fn thehum_range(author: &str, from: u64, to: Option<u64>) -> Result<()> {
    let dir = hum_paths::thehum_dir();
    let events = thehum_load_all(&dir)?;
    let mut matched: Vec<&thehum::Event> = events.iter()
        .filter(|e| e.author == author && e.seq >= from && to.map(|hi| e.seq <= hi).unwrap_or(true))
        .collect();
    matched.sort_by_key(|e| e.seq);
    if matched.is_empty() {
        println!("(no events matching author={author} from={from}{})",
            to.map(|t| format!(" to={t}")).unwrap_or_default());
        return Ok(());
    }
    println!("  {:>8}  {:<10}  {:<13}  {}", "SEQ", "CHI", "TS_MS", "RID");
    for e in &matched {
        println!("  {:>8}  {:<10}  {:<13}  {}", e.seq, e.chi, e.ts_ms, e.rid);
    }
    println!("\n{} event(s).", matched.len());
    Ok(())
}

fn thehum_verify() -> Result<()> {
    let dir = hum_paths::thehum_dir();
    let mut events = thehum_load_all(&dir)?;
    events.sort_by_key(|e| (e.author.clone(), e.seq));
    if events.is_empty() {
        println!("OK (empty log)");
        return Ok(());
    }

    let key_path = hum_paths::humd_key();
    let bytes = std::fs::read(&key_path)
        .with_context(|| format!("read {}", key_path.display()))?;
    if bytes.len() != 32 {
        anyhow::bail!("humd.key is {} bytes, expected 32", bytes.len());
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pubkey = signing.verifying_key();

    match thehum::read::verify_chain(&events, &pubkey) {
        Ok(()) => {
            println!("OK ({} events verified)", events.len());
            Ok(())
        }
        Err(e) => {
            println!("VIOLATION: {e}");
            Err(e)
        }
    }
}

fn thehum_replay() -> Result<()> {
    use std::collections::BTreeMap;
    let dir = hum_paths::thehum_dir();
    let mut events = thehum_load_all(&dir)?;
    events.sort_by_key(|e| (e.author.clone(), e.seq));

    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for e in &events {
        *counts.entry(e.chi.clone()).or_default() += 1;
    }

    println!("  {:<16}  {}", "CHI", "COUNT");
    for (chi, n) in &counts {
        println!("  {:<16}  {}", chi, n);
    }
    println!("\n{} event(s) across {} kind(s).", events.len(), counts.len());
    Ok(())
}

fn fmt_ts_ms(ts_ms: i64) -> String {
    use chrono::DateTime;
    DateTime::from_timestamp_millis(ts_ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| ts_ms.to_string())
}
