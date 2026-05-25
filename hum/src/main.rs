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
//!   hum bees               list bee services
//!   hum bees restart NAME  restart one bee (or --all) via service mgr
//!   hum penny              show lifetime counters
//!   hum recipes [name]     list recipes / point at one
//!   hum uninstall          remove service + binary (state preserved)
//!   hum version            print version
//!   hum help               print this surface

use std::path::PathBuf;
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
    /// List bee services, or restart them. `hum bees` lists; `hum bees
    /// restart <name>` or `hum bees restart --all` bounces them through
    /// the service manager (graceful, same identity — unlike pkill).
    Bees {
        #[command(subcommand)]
        action: Option<BeeAction>,
    },
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

#[derive(Subcommand)]
enum BeeAction {
    /// Restart one bee by name, or every bee with --all.
    Restart {
        /// Bee service short id (e.g. "hum-paid-oracle"). Omit with --all.
        name: Option<String>,
        /// Restart every installed bee service.
        #[arg(long)]
        all: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        None => summary(),
        Some(Cmd::Status) => status(),
        Some(Cmd::Logs { lines }) => logs(lines),
        Some(Cmd::Bees { action }) => bees(action),
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

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME unset")
}

fn xdg(var: &str, default_suffix: &str) -> Result<PathBuf> {
    if let Some(v) = std::env::var_os(var) {
        return Ok(PathBuf::from(v));
    }
    Ok(home()?.join(default_suffix))
}

fn xdg_runtime_hum() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|v| PathBuf::from(v).join("hum"))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/hum-{}", libc_getuid())))
}

fn unsafe_libc_getuid_fallback() -> u32 { 0 }
fn libc_getuid() -> u32 {
    // Avoid pulling in the libc crate — shell out to `id -u`. Cheap;
    // only called when XDG_RUNTIME_DIR isn't set.
    Command::new("id").arg("-u").output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(unsafe_libc_getuid_fallback)
}

fn humd_bin() -> Result<PathBuf> {
    // Convention: $HUM_DATA/bin/humd or $HOME/.local/bin/humd.
    let candidates = [
        std::env::var_os("HUM_BIN").map(PathBuf::from),
        home().ok().map(|h| h.join(".local").join("bin").join("humd")),
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
        home().ok().map(|h| h.join(".local/share/hum/src/scripts/svc.sh")),
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
    let cfg = xdg("XDG_CONFIG_HOME", ".config")?.join("hum");
    let state = xdg("XDG_STATE_HOME", ".local/state")?.join("hum");
    let runtime = xdg_runtime_hum();
    let thrum_sock = std::env::var_os("HUM_THRUM_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime.join("thrum.sock"));

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
    println!("identity:     {} {}", state.join("humd.key").display(),
             yn(state.join("humd.key").exists()));
    println!("peers.json:   {} {}", cfg.join("peers.json").display(),
             yn(cfg.join("peers.json").exists()));
    println!("hum.json:     {} {}", cfg.join("hum.json").display(),
             yn(cfg.join("hum.json").exists()));
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
    let svc = svc_helper().context("scripts/svc.sh not found — install hum first")?;
    let script = format!(r#"
        . {svc}
        case "$SVC_OS" in
          Linux)  journalctl --user -u hum --no-pager -n {lines} ;;
          Darwin) tail -n {lines} "$HOME/Library/Logs/sh.hum.hum.out.log" \
                                  "$HOME/Library/Logs/sh.hum.hum.err.log" 2>/dev/null ;;
          *) echo "logs unavailable on $SVC_OS" >&2; exit 1 ;;
        esac
    "#, svc = svc.display());
    Command::new("bash").arg("-c").arg(script).status()?;
    Ok(())
}

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

fn bees(action: Option<BeeAction>) -> Result<()> {
    let svc = svc_helper().context("scripts/svc.sh not found — install hum first")?;
    let installed = bee_list(&svc)?;

    let restart = |name: &str| -> bool {
        let ok = Command::new("bash")
            .arg("-c")
            .arg(format!(". {} && svc_restart {}", svc.display(), name))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        println!("  {} {name}", if ok { "✓ restarted" } else { "✗ failed" });
        ok
    };

    match action {
        // `hum bees` — list.
        None => {
            if installed.is_empty() {
                println!("no bee services installed (foragers/workers register their own under hives/<kind>/install)");
            } else {
                println!("bee services:");
                for b in &installed { println!("  {b}"); }
                println!("\nrestart: hum bees restart <name>   |   all: hum bees restart --all");
            }
            Ok(())
        }
        Some(BeeAction::Restart { all: true, .. }) => {
            if installed.is_empty() {
                println!("no bee services to restart");
                return Ok(());
            }
            println!("restarting {} bee(s):", installed.len());
            for b in &installed { restart(b); }
            Ok(())
        }
        Some(BeeAction::Restart { name: Some(name), all: false }) => {
            // Accept the bare hive name too (e.g. "paid-oracle" → "hum-paid-oracle").
            let target = if installed.iter().any(|b| b == &name) {
                name.clone()
            } else {
                let prefixed = format!("hum-{name}");
                if installed.iter().any(|b| b == &prefixed) { prefixed } else { name.clone() }
            };
            if !installed.iter().any(|b| b == &target) {
                anyhow::bail!(
                    "no bee service '{target}'. installed: {}",
                    if installed.is_empty() { "(none)".into() } else { installed.join(", ") }
                );
            }
            println!("restarting bee:");
            if restart(&target) { Ok(()) } else { anyhow::bail!("restart failed for {target}") }
        }
        Some(BeeAction::Restart { name: None, all: false }) => {
            anyhow::bail!("name a bee (hum bees restart <name>) or use --all. list: hum bees");
        }
    }
}

fn penny() -> Result<()> {
    let path = xdg("XDG_STATE_HOME", ".local/state")?.join("hum").join("penny.json");
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
    if let Ok(h) = home() {
        let candidate = h.join(".local/share/hum/src");
        if candidate.exists() { return candidate; }
    }
    PathBuf::from(".")
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
