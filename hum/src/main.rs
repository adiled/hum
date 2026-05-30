//! `hum` — main user-facing CLI.
//!
//! Inspection-only for 0.3: every subcommand reads cross-platform
//! state (filesystem + service manager via scripts/svc.sh). Daemon-
//! internal queries (peers, drift, drone, sessions) will land when
//! humd exposes an RPC control socket; until then, those live as
//! `humd <subcommand>` arguments inside the daemon binary's own CLI.
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
//!   hum penny              show lifetime counters
//!   hum recipes [name]     list recipes / point at one
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
    // source, builds, bounces the service via scripts/svc.sh.
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

fn svc_helper() -> Option<PathBuf> {
    // Look next to this binary's repo root or in the rsynced source.
    let candidates = [
        std::env::current_exe().ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("../../scripts/svc.sh")),
        Some(hum_paths::src_dir().join("scripts/svc.sh")),
        Some(PathBuf::from("./scripts/svc.sh")),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
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

    if let Some(svc) = svc_helper() {
        println!();
        let _ = Command::new("bash")
            .arg("-c")
            .arg(format!(". {} && svc_status hum", svc.display()))
            .status();
    }
    Ok(())
}

fn yn(b: bool) -> &'static str { if b { "✓" } else { "missing" } }

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

    // 5. Bees + service state (full manifest info).
    println!("\n[bees]");
    if let Some(svc) = svc_helper() {
        let installed = bee_list(&svc).unwrap_or_default();
        let _ = bee_list_full(&svc, &installed);
    } else {
        println!("  (svc.sh not found — can't enumerate services)");
    }

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

// ── hive / bee shared service plumbing ─────────────────────────────────────

/// `svc_list` short-ids of installed bee services (the `hum-*` units).
fn bee_list(svc: &std::path::Path) -> Result<Vec<String>> {
    let out = Command::new("bash")
        .arg("-c")
        .arg(format!(". {} && svc_list", svc.display()))
        .output()
        .context("run svc_list")?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// True if a unit is currently running (svc_is_active exit 0).
fn svc_active(svc: &std::path::Path, unit: &str) -> bool {
    Command::new("bash")
        .arg("-c")
        .arg(format!(". {} && svc_is_active {}", svc.display(), unit))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Last exit code reported by the service manager. None if unknown.
/// Non-zero with `!svc_active` means crash-loop.
fn svc_last_exit(svc: &std::path::Path, unit: &str) -> Option<i32> {
    let out = Command::new("bash")
        .arg("-c")
        .arg(format!(". {} && svc_last_exit {}", svc.display(), unit))
        .output().ok()?;
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    raw.parse().ok()
}

/// Resolve a user-given name to installed service unit(s), tolerantly:
///   exact unit ("hum-claude-cli-worker")
///   → "hum-<name>" ("paid-oracle" → "hum-paid-oracle")
///   → hive-kind prefix ("claude-cli" → "hum-claude-cli-worker"), so the
///     kind shown by `hum hive --list` addresses its bee.
fn resolve_units(installed: &[String], name: &str) -> Vec<String> {
    if name == "all" { return installed.to_vec(); }
    let prefixed = format!("hum-{name}");
    installed.iter().filter(|b| {
        **b == name || **b == prefixed
            || b.strip_prefix("hum-").map(|s| s == name || s.starts_with(&format!("{name}-"))).unwrap_or(false)
    }).cloned().collect()
}

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
    let hives_dir = root.join("hives");
    let mut kinds: BTreeMap<String, (bool, Option<String>, bool)> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(&hives_dir) {
        for e in entries.flatten() {
            if e.path().is_dir() && e.path().join("install").exists() {
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
    if let Some(svc) = svc_helper() {
        let catalogue: Vec<String> = kinds.keys().cloned().collect();
        for unit in bee_list(&svc).unwrap_or_default() {
            let sid = unit.strip_prefix("hum-").unwrap_or(&unit).to_string();
            let kind = catalogue.iter()
                .filter(|k| sid == **k || sid.starts_with(&format!("{k}-")))
                .max_by_key(|k| k.len()).cloned().unwrap_or(sid);
            kinds.entry(kind).or_default().2 = true;
        }
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
    let orchfile = dir.join("Orchfile");
    if !orchfile.exists() {
        anyhow::bail!("no Orchfile at {}", orchfile.display());
    }
    let kind = read_orchfile_service(&orchfile)?
        .ok_or_else(|| anyhow::anyhow!("no SERVICE directive in {}", orchfile.display()))?;

    if dir.join("Cargo.toml").exists() {
        println!("building {kind} (cargo install --path {} ...)", dir.display());
        let s = Command::new("cargo")
            .args(["install", "--quiet", "--locked", "--path"]).arg(&dir)
            .args(["--root"]).arg(home_local())
            .arg("--force")
            .status()?;
        if !s.success() { anyhow::bail!("cargo install failed for {}", dir.display()); }
    } else if dir.join("package.json").exists() {
        anyhow::bail!("TS hive install not yet automated; run `pnpm install && pnpm build` in {}", dir.display());
    } else {
        anyhow::bail!("no Cargo.toml or package.json in {} — don't know how to build", dir.display());
    }

    let orch_d = hum_paths::config_dir().join("orch.d");
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
    std::fs::write(hum_orchfile(), combined)?;
    Ok(())
}

fn home_local() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
}

fn resolve_hive_dir(reference: &str) -> Result<PathBuf> {
    if let Some(rest) = reference.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(5, '/').collect();
        if parts.len() == 5 && parts[2] == "tree" {
            let (org, repo, branch, sub) = (parts[0], parts[1], parts[3], parts[4]);
            if org == "adiled" && repo == "hum" {
                return Ok(repo_root_or_install_dir().join(sub));
            }
            let cache = hum_paths::cache_dir().join("hives").join(format!("{org}-{repo}-{branch}"));
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
    let bundled = repo_root_or_install_dir().join("hives").join(reference);
    if bundled.exists() { return Ok(bundled); }
    anyhow::bail!("can't resolve hive '{reference}' (not a bundled name, path, or github source URL)");
}

/// Render `hum bee --list` with maximum info: humd's live manifest
/// (hid, role, models, tools, provides, wire, version, source) joined
/// with each bee's service unit + running state.
fn bee_list_full(svc: &std::path::Path, installed: &[String]) -> Result<()> {
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

    // Live bees (full info), each matched to a service unit if any.
    let mut matched_units: Vec<String> = Vec::new();
    for m in &live {
        let hive = s(m, "name");
        let unit = installed.iter().find(|u| {
            let sid = u.strip_prefix("hum-").unwrap_or(u);
            sid == hive || sid.starts_with(&format!("{hive}-"))
        }).cloned();
        if let Some(u) = &unit { matched_units.push(u.clone()); }

        let role = arr(m, "bee").iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>().join("+");
        let models = arr(m, "models").iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>();
        let tools: Vec<String> = arr(m, "tools").iter().map(|t| s(t, "name")).filter(|x| !x.is_empty()).collect();
        let provides = arr(m, "provides").iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>();
        let wire = m.get("propensity").map(|p| s(p, "wire")).unwrap_or_default();
        let state = match &unit {
            Some(u) if svc_active(svc, u) => "in nest (service running)".to_string(),
            Some(u) => match svc_last_exit(svc, u) {
                Some(code) if code != 0 => format!("⚠ crash-looping (exit {code})"),
                _ => "in nest (service stopped?)".to_string(),
            },
            None => "in nest (unmanaged)".to_string(),
        };

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
        if let Some(u) = &unit  { println!("    service:  {u}"); }
        println!();
    }

    for u in installed {
        if matched_units.contains(u) { continue; }
        let state = if svc_active(svc, u) {
            "service running, not handshaked".to_string()
        } else {
            match svc_last_exit(svc, u) {
                Some(code) if code != 0 => format!("⚠ crash-looping (exit {code})"),
                _ => "exited".to_string(),
            }
        };
        println!("● {}  —  {state}", u.strip_prefix("hum-").unwrap_or(u));
        println!("    service:  {u}");
        println!();
    }

    println!("verbs: hum bee <id> enter | exit | reenter   (id `all` for every bee)");
    Ok(())
}

fn bee(target: Option<String>, verb: Option<String>, list: bool) -> Result<()> {
    let svc = svc_helper().context("scripts/svc.sh not found — install hum first")?;
    let installed = bee_list(&svc)?;

    // List: `hum bee --list`, or bare `hum bee`. Full info comes from
    // humd's live manifest snapshot ($XDG_STATE_HOME/hum/bees.json);
    // service state comes from the service manager.
    if list || (target.is_none() && verb.is_none()) {
        bee_list_full(&svc, &installed)?;
        return Ok(());
    }

    // Operate: `hum bee <target> <verb>`.
    let (target, verb) = match (target, verb) {
        (Some(t), Some(v)) => (t, v),
        (Some(t), None) => anyhow::bail!("hum bee {t} <verb> — enter | exit | reenter"),
        _ => anyhow::bail!("hum bee <id> <verb>, or hum bee --list"),
    };
    // Prefer humnest for any kind it knows about; fall back to svc.sh for
    // legacy units or unknown targets (svc_active/svc_last_exit helpers
    // stay live so `hum bee --list` keeps working).
    if target != "all" && orch_route_verb(&target, &verb)? {
        return Ok(());
    }
    let op = match verb.as_str() {
        "enter"   => "svc_start",
        "exit"    => "svc_stop",
        "reenter" => "svc_restart",
        other => anyhow::bail!("unknown verb '{other}' (enter | exit | reenter)"),
    };
    let units = resolve_units(&installed, &target);
    if units.is_empty() {
        anyhow::bail!("no bee matching '{target}'. bees: {}",
            if installed.is_empty() { "(none)".into() } else { installed.join(", ") });
    }
    let past = match verb.as_str() { "enter" => "entered", "exit" => "exited", _ => "re-entered" };
    let mut all_ok = true;
    for unit in &units {
        let ok = Command::new("bash").arg("-c")
            .arg(format!(". {} && {} {}", svc.display(), op, unit))
            .status().map(|s| s.success()).unwrap_or(false);
        all_ok &= ok;
        println!("  {} {unit}", if ok { format!("✓ {past}") } else { "✗ failed".into() });
    }
    if all_ok { Ok(()) } else { anyhow::bail!("one or more {verb} ops failed") }
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
    let recipes_dir = root.join("recipes");
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
            let install = recipes_dir.join(&n).join("install");
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

fn hum_orchfile() -> PathBuf { hum_paths::config_dir().join("Orchfile") }

fn orchd_cmd() -> Command {
    let mut c = Command::new("orchd");
    c.arg("--orchfile").arg(hum_orchfile())
     .arg("--user")
     .arg("--namespace").arg("hum");
    c
}

/// Service names declared in hum's Orchfile.
fn orch_catalog() -> Vec<String> {
    let path = hum_orchfile();
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
    let svc = svc_helper().context("scripts/svc.sh not found")?;
    let script = format!(r#"
        . {}
        svc_uninstall hum || true
    "#, svc.display());
    Command::new("bash").arg("-c").arg(script).status()?;
    if let Ok(bin) = humd_bin() {
        let _ = std::fs::remove_file(&bin);
        println!("removed {}", bin.display());
    }
    println!("state preserved. `./install purge` to wipe.");
    Ok(())
}
