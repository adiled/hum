//! `hum` — main user-facing CLI.
//!
//! Subcommands:
//!   hum                    health summary
//!   hum status             daemon + config + service state
//!   hum logs               tail journalctl (Linux) / launchd logs (macOS)
//!   hum doctor             one-shot full diagnostic dump (run this first)
//!   hum hive --list          the hives in the catalogue and installed here
//!   hum hive <ref> install   teach this humd a hive (runs its hive-install)
//!   hum hive <ref> uninstall forget a hive
//!   hum bee --list           the bees entered here, and which have nestled
//!   hum bee enter --hive <h> sign a new bee in under a hive; prints its id
//!   hum bee <id> exit        sign it out (reenter signs it back in)
//!   hum config               show the paths hum keeps
//!   hum comb [hive]          show the comb — lifetime measures
//!   hum recipes [name]     list recipes / point at one
//!   hum thehum <verb>      inspect the persistent chi log
//!                          (status | tail | range | verify | replay)
//!   hum update             self-update from latest GitHub release
//!   hum uninstall          remove service + binary (state preserved)
//!   hum version            print version
//!   hum help               print this surface
//!
//! A bee is a participant and a hive is its kind; `hives` and `bees`
//! hold what that means here. A hive ref is the same dialect a bee
//! advertises as its `source`: a bundled name, a local path, or a github
//! tree URL. Our own repo maps to the local checkout; a foreign repo is
//! shallow-cloned to a cache.
//!
//! Live peer state lives in the running daemon, which has no
//! admin-tone RPC yet — `hum ensemble` reads the on-disk files.

mod bees;
mod hives;
mod orchfile;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "hum",
    version = env!("HUM_VERSION"),
    about = "hum — the AI stack on a biodiverse agentic kernel"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = "Daemon + config + service-manager state")]
    Status,
    #[command(about = "Tail recent daemon logs (cross-platform)")]
    Logs {
        #[arg(help = "Number of lines to show (default: 200)")]
        #[arg(short = 'n', long, default_value_t = 200)]
        lines: u32,
    },
    #[command(about = "One-shot full diagnostic dump: versions, config, env sanity, the claude binary, every bee + service state, and recent daemon + worker logs with warnings highlighted. Run this first when something is wrong; paste the output into a bug report")]
    Doctor,
    #[command(about = "Hives — the kinds this humd understands.")]
    #[command(long_about = "Hives — the kinds this humd understands.

  hum hive --list             the catalogue, and what is installed here
  hum hive <ref> install      teach this humd the hive
  hum hive <ref> uninstall    forget it

A directory with an Orchfile is a hive. Installing one runs its
`hive-install` unit, if it has one, and from then on this humd
understands bees the hive enters. What the hive runs, and how, is the
hive's own business; hum starts nothing.

<ref> is a bundled name, a local path, or the source URL a bee
advertises (github tree URL of a hives/<hive> dir).")]
    #[command(verbatim_doc_comment)]
    Hive {
        #[arg(help = "Hive ref (name | path | source URL). Omit with --list")]
        target: Option<String>,
        #[arg(help = "Action on the hive: `install` | `uninstall`")]
        action: Option<String>,
        #[arg(help = "List the hive catalogue")]
        #[arg(long)]
        list: bool,
    },
    #[command(about = "Bees — the participants this humd hears from.")]
    #[command(long_about = "Bees — the participants this humd hears from.

A bee is a key. A hive enters one under itself, hands its id to whatever
will act as that bee (through HUM_BEE), and that process signs its hello
with the key. humd admits only a signed hello from an entered bee whose
hive is installed. hum starts no process.

  hum bee --list               bees entered here, and which have nestled
  hum bee enter --hive <hive>  mint a bee and sign it in; prints its id
  hum bee <id> exit            sign it out; humd refuses its next hello
  hum bee <id> reenter         sign it back in")]
    #[command(verbatim_doc_comment)]
    Bee {
        #[arg(help = "`enter`, or the id of an entered bee")]
        target: Option<String>,
        #[arg(help = "Verb for a bee id: exit | reenter")]
        verb: Option<String>,
        #[arg(help = "The hive to enter a bee under")]
        #[arg(long)]
        hive: Option<String>,
        #[arg(help = "List bees")]
        #[arg(long)]
        list: bool,
    },
    #[command(about = "Show the comb — lifetime measures of what crossed the kernel")]
    Comb {
        #[arg(help = "Hive to report on (default: every hive)")]
        hive: Option<String>,
    },
    #[command(about = "List available recipes (recipes/*) or run one")]
    Recipes {
        #[arg(help = "Recipe name (e.g. \"opencode\"). Omit to list")]
        name: Option<String>,
    },
    #[command(about = "Inspect the persistent chi log (thehum).")]
    #[command(long_about = "Inspect the persistent chi log (thehum).

  hum thehum status               dir, file count, seq, snapshot
  hum thehum tail [-n N]          most recent daily file (default 20)
  hum thehum range --author <hid> filter by author + seq range
                   --from <seq> [--to <seq>]
  hum thehum verify               check hash chain + signatures
  hum thehum replay               count events by chi kind")]
    #[command(verbatim_doc_comment)]
    Thehum {
        #[command(subcommand)]
        verb: ThehumVerb,
    },
    #[command(about = "Inspect or edit the peer-mesh state.")]
    #[command(long_about = "Inspect or edit the peer-mesh state.

  hum ensemble                         show our identity + reach + peers
  hum ensemble peer add <humd_id>      append an entry to peers.json
      --hint <tcp:host:port|iroh:hex>  (repeatable)
      --alias <name>                   optional human-friendly name
  hum ensemble peer rm <humd_id|alias> drop matching entries")]
    #[command(verbatim_doc_comment)]
    Ensemble {
        #[command(subcommand)]
        verb: Option<EnsembleVerb>,
    },
    #[command(about = "Show the hum paths and the hives installed here")]
    Config,
    #[command(about = "Stop the service and remove the humd binary. State preserved")]
    Uninstall,
    #[command(about = "Check for a newer release and self-update. Compares the local version against GitHub's latest release; if newer, re-runs the canonical install (which bounces the service atomically)")]
    Update {
        #[arg(help = "Update even when versions match (force reinstall)")]
        #[arg(long)]
        force: bool,
        #[arg(help = "Update to the latest main HEAD instead of the latest release tag")]
        #[arg(long)]
        edge: bool,
    },
}

#[derive(Subcommand)]
enum EnsembleVerb {
    #[command(about = "Manage entries in peers.json — bootstrap peers humd dials on boot")]
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
}

#[derive(Subcommand)]
enum PeerAction {
    #[command(about = "Append a peer to peers.json. Repeated `--hint` flags accumulate. Existing entries with the same humd_id are replaced (idempotent)")]
    #[command(long_about = "Append a peer to peers.json. Repeated `--hint` flags accumulate.
Existing entries with the same humd_id are replaced (idempotent).")]
    Add {
        #[arg(help = "Peer humd_id (hex, with or without `humd_` prefix)")]
        humd_id: String,
        #[arg(help = "Dial hint, e.g. `tcp:host:port` or `iroh:<64-hex>`. Repeat for multiple")]
        #[arg(long)]
        hint: Vec<String>,
        #[arg(help = "Optional alias for `hum://<alias>/path` URI resolution")]
        #[arg(long)]
        alias: Option<String>,
    },
    #[command(about = "Remove all entries matching `target` (by humd_id or alias)")]
    Rm {
        #[arg(help = "humd_id (full hex, with/without prefix) or alias")]
        target: String,
    },
}

#[derive(Subcommand)]
enum ThehumVerb {
    #[command(about = "Print dir, file count, total seq, latest snapshot height + ts")]
    Status,
    #[command(about = "Tail the most recent daily file as compact JSON, one event per line")]
    Tail {
        #[arg(help = "Number of trailing events to show")]
        #[arg(short = 'n', long, default_value_t = 20)]
        n: usize,
    },
    #[command(about = "Filter events by author hid and seq range")]
    Range {
        #[arg(help = "Author humd hid (hex)")]
        #[arg(long)]
        author: String,
        #[arg(help = "Inclusive lower bound on seq")]
        #[arg(long)]
        from: u64,
        #[arg(help = "Inclusive upper bound on seq")]
        #[arg(long)]
        to: Option<u64>,
    },
    #[command(about = "Verify hash chain + signatures across the whole log")]
    Verify,
    #[command(about = "Replay the log, counting events by chi kind")]
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
        Some(Cmd::Bee { target, verb, hive, list }) => bee(target, verb, hive, list),
        Some(Cmd::Comb { hive }) => comb(hive),
        Some(Cmd::Recipes { name }) => recipes(name),
        Some(Cmd::Thehum { verb }) => thehum_cmd(verb),
        Some(Cmd::Ensemble { verb }) => ensemble(verb),
        Some(Cmd::Uninstall) => uninstall(),
        Some(Cmd::Config) => config(),
        Some(Cmd::Update { force, edge }) => update(force, edge),
    }
}

fn update(force: bool, edge: bool) -> Result<()> {
    let local = env!("CARGO_PKG_VERSION").to_string();
    let stamped = env!("HUM_VERSION");
    let upstream = match latest_release_tag() {
        Some(t) => t,
        None => {
            eprintln!("could not reach github.com; skipping update");
            return Ok(());
        }
    };
    let upstream_trim = upstream.trim_start_matches('v').to_string();
    println!("local: {stamped}  upstream: {upstream_trim}");
    if edge {
        println!("updating to the latest main HEAD …");
    } else {
        if !force && upstream_trim == local {
            println!("up to date.");
            return Ok(());
        }
        println!("updating to {upstream_trim} …");
    }
    let url = "https://raw.githubusercontent.com/adiled/hum/main/install";
    let script = if edge { format!("curl -fsSL {url} | bash -s -- --edge") } else { format!("curl -fsSL {url} | bash") };
    let status = Command::new("bash")
        .arg("-c")
        .arg(script)
        .status()?;
    if !status.success() {
        anyhow::bail!("installer exited with {status}");
    }
    println!("update complete.");
    Ok(())
}

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
    let needle = "\"tag_name\":";
    let start = body.find(needle)? + needle.len();
    let rest = &body[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')?;
    Some(rest[q1..q1+q2].to_string())
}


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
    s.write_all(b"{\"chi\":\"hello\",\"sid\":\"hum-doctor-probe\"}\n")?;
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


fn summary() -> Result<()> {
    println!("hum {} — `hum help` for the surface", env!("HUM_VERSION"));
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

fn ensemble(verb: Option<EnsembleVerb>) -> Result<()> {
    match verb {
        None => ensemble_show(),
        Some(EnsembleVerb::Peer { action }) => match action {
            PeerAction::Add { humd_id, hint, alias } => ensemble_peer_add(&humd_id, hint, alias),
            PeerAction::Rm { target } => ensemble_peer_rm(&target),
        },
    }
}

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

    println!("\n[versions]");
    println!("  hum CLI:    {}", env!("HUM_VERSION"));
    if let Ok(b) = humd_bin() {
        let v = Command::new(&b).arg("--version").output().ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string()).unwrap_or_else(|| "?".into());
        println!("  humd:       {v}  ({})", b.display());
    } else {
        println!("  humd:       NOT FOUND (set HUM_BIN or run ./install)");
    }
    println!("  os:         {} {}", std::env::consts::OS, std::env::consts::ARCH);

    let hum_json = hum_paths::hum_json();
    let peers_json = hum_paths::peers_json();
    let humd_key = hum_paths::humd_key();
    println!("\n[config + state]");
    println!("  hum.json:   {} {}", hum_json.display(), yn(hum_json.exists()));
    println!("  peers.json: {} {}", peers_json.display(), yn(peers_json.exists()));
    println!("  identity:   {} {}", humd_key.display(), yn(humd_key.exists()));

    println!("\n[hum.json schema validation]");
    match std::fs::read_to_string(&hum_json) {
        Err(_) => println!("  (no hum.json — humd runs on defaults)"),
        Ok(raw) => match hum_paths::config::validate(&raw) {
            Ok(()) => {
                let retired = hum_paths::config::retired_in_raw(&raw);
                if retired.is_empty() {
                    println!("  ✓ valid against hum.schema.json");
                } else {
                    println!("  ✓ valid against hum.schema.json");
                    println!("      retired key(s) the daemon drops on boot: {}", retired.join(", "));
                }
            }
            Err(violations) => {
                println!("  ✗ INVALID — humd will refuse to start:");
                for v in &violations { println!("      - {v}"); }
            }
        },
    }

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
                    let bee = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                    match hum_identity::load_bee_key(&bee) {
                        Ok(key) => {
                            let state = match hum_identity::entry(&bee) {
                                Some(e) if e.exited => format!("exited from {}", e.hive),
                                Some(e) => format!("entered under {}", e.hive),
                                None => "never entered".to_string(),
                            };
                            println!("  {}: ✓ {state}", key.hid.short());
                        }
                        Err(e) => println!("  {bee}: ✗ {sz} bytes — {e}"),
                    }
                }
            }
            if !any { println!("  (none yet)"); }
        }
    }

    println!("\n[env sanity]");
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    let runtime_exists = std::path::Path::new(&runtime).is_dir();
    println!("  XDG_RUNTIME_DIR: {runtime} {}", if runtime_exists { "✓" } else { "✗ DOES NOT EXIST (comb writes will fail — common macOS trap when set to a Linux /run/user path)" });

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

    println!("\n[claude binary]");
    let claude = std::env::var("CLAUDE_CLI_PATH").unwrap_or_else(|_| "claude".into());
    match Command::new(&claude).arg("--version").output() {
        Ok(o) if o.status.success() => println!("  {claude}: {}", String::from_utf8_lossy(&o.stdout).trim()),
        Ok(_) | Err(_) => println!("  {claude}: ✗ NOT RUNNABLE — set CLAUDE_CLI_PATH to the real binary"),
    }

    println!("\n[bees]");
    let _ = bees::list();

    println!("\n[recent humd + worker logs — warnings/errors]");
    print_recent_logs("hum", 60);

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


fn hive(target: Option<String>, action: Option<String>, list: bool) -> Result<()> {
    if list || target.is_none() {
        return hive_list();
    }
    let ref_ = target.unwrap();
    match action.as_deref() {
        Some("install") => hives::install(&ref_),
        Some("uninstall") => hives::uninstall(&ref_),
        Some(act) => anyhow::bail!("unknown hive action '{act}' for '{ref_}' (try: install | uninstall)"),
        None => anyhow::bail!("hum hive {ref_} <action> — try: hum hive {ref_} install"),
    }
}

fn hive_list() -> Result<()> {
    let catalogue = hives::catalogue();
    let installed = hives::installed();
    let mut all: Vec<String> = catalogue.iter().chain(installed.iter()).cloned().collect();
    all.sort();
    all.dedup();
    if all.is_empty() {
        println!("no hives found (catalogue: {})", hives::catalogue_dir().display());
        return Ok(());
    }
    let nest_default = hum_paths::config::load().nest.default;
    println!("Hives (catalogue: {}):\n", hives::catalogue_dir().display());
    let entered = hum_identity::entered();
    println!("  {:<18} {:<10} {:<10} {}", "HIVE", "CATALOGUE", "INSTALLED", "BEES ENTERED");
    for hive in &all {
        let here = installed.contains(hive);
        let count = entered.iter().filter(|(_, e)| &e.hive == hive && !e.exited).count();
        let raises = if count == 0 { String::new() } else { count.to_string() };
        let marker = if hive == &nest_default { "  (nest default)" } else { "" };
        println!(
            "  {:<18} {:<10} {:<10} {}{marker}",
            hive,
            if catalogue.contains(hive) { "✓" } else { "—" },
            if here { "✓" } else { "—" },
            if raises.is_empty() { "—" } else { raises.as_str() },
        );
    }
    println!("\ninstall one: hum hive <name|path|source-url> install   |   bees: hum bee --list");
    Ok(())
}

fn bee(target: Option<String>, verb: Option<String>, hive: Option<String>, list: bool) -> Result<()> {
    if list || (target.is_none() && verb.is_none()) {
        return bees::list();
    }
    match (target.as_deref(), verb, hive) {
        (Some("enter"), None, Some(h)) => bees::enter(&h),
        (Some("enter"), None, None) => anyhow::bail!("hum bee enter --hive <hive>"),
        (Some(id), Some(v), None) => bees::act(id, &v),
        (Some(id), None, _) => anyhow::bail!("hum bee {id} <verb> — exit | reenter"),
        _ => anyhow::bail!("hum bee enter --hive <hive>, hum bee <id> exit|reenter, or hum bee --list"),
    }
}

fn comb_key(key: &str) -> std::borrow::Cow<'_, str> {
    match key.strip_prefix("bee:") {
        Some(rest) => format!("hive:{rest}").into(),
        None => key.into(),
    }
}

fn comb(hive: Option<String>) -> Result<()> {
    let path = hum_paths::comb();
    if !path.exists() {
        println!("no comb.json yet ({})", path.display());
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let raw_counters: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    let mut counters: BTreeMap<String, u64> = BTreeMap::new();
    for (k, v) in &raw_counters {
        if let Some(n) = v.as_u64() {
            *counters.entry(comb_key(k).into_owned()).or_insert(0) += n;
        }
    }

    if let Some(hive) = hive {
        let prefix = format!("hive:{hive}:");
        let mut rows: Vec<(String, u64)> = counters
            .iter()
            .filter_map(|(k, n)| Some((k.strip_prefix(prefix.as_str())?.to_string(), *n)))
            .collect();
        if rows.is_empty() {
            println!("no measures for {hive}");
            return Ok(());
        }
        rows.sort();
        println!("comb — {hive}");
        let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (metric, n) in &rows {
            println!("  {metric:<width$}  {n:>12}", width = width);
        }
        return Ok(());
    }

    let mut activity: Vec<(&str, u64)> = Vec::new();
    let mut nectar: Vec<(&str, u64)> = Vec::new();
    let mut by_hive: BTreeMap<String, Vec<(String, u64)>> = Default::default();

    for (key, n) in &counters {
        let n = *n;
        if let Some(rest) = key.strip_prefix("hive:") {
            let Some((hive, rest)) = rest.split_once(':') else {
                continue;
            };
            by_hive
                .entry(hive.to_string())
                .or_default()
                .push((rest.to_string(), n));
            continue;
        }
        if let Some(rest) = key.strip_prefix("chi:") {
            activity.push((rest, n));
        } else if let Some(rest) = key.strip_prefix("nectar:") {
            nectar.push((rest, n));
        }
    }

    if activity.is_empty() && nectar.is_empty() && by_hive.is_empty() {
        println!("the comb is empty — nothing has crossed the kernel yet");
        return Ok(());
    }

    activity.sort_unstable();
    nectar.sort_unstable();

    println!("comb — lifetime measures");
    if !activity.is_empty() {
        println!("\n  activity");
        let width = activity.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (chi, n) in &activity {
            println!("    {chi:<width$}  {n:>12}", width = width);
        }
    }
    if !nectar.is_empty() {
        println!("\n  nectar");
        let width = nectar.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (metric, n) in &nectar {
            println!("    {metric:<width$}  {n:>12}", width = width);
        }
    }
    if !by_hive.is_empty() {
        println!("\n  by hive");
        for (hive, rows) in &mut by_hive {
            println!("    {hive}");
            rows.sort();
            let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            for (metric, n) in rows.iter() {
                println!("      {metric:<width$}  {n:>12}", width = width);
            }
        }
    }
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
            let install = recipes_dir.join(&n).join(hum_paths::RECIPE_INSTALL_SCRIPT);
            if !install.exists() {
                anyhow::bail!("recipes/{n}/install not found");
            }
            Command::new(install).status()?;
        }
    }
    Ok(())
}

pub(crate) fn repo_root_or_install_dir() -> PathBuf {
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



fn config() -> Result<()> {
    println!("[paths]");
    println!("  config dir:   {}", hum_paths::config_dir().display());
    println!("  state dir:    {}", hum_paths::state_dir().display());
    println!("  hives:        {}", hum_paths::hives_d_dir().display());
    println!("  bee keys:     {}", hum_paths::bees_dir().display());
    println!("  entered bees: {}", hum_paths::entered_dir().display());
    println!("\n[installed hives]");
    for hive in hives::installed() {
        println!("  {hive}");
    }
    Ok(())
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


fn thehum_cmd(verb: ThehumVerb) -> Result<()> {
    match verb {
        ThehumVerb::Status => thehum_status(),
        ThehumVerb::Tail { n } => thehum_tail(n),
        ThehumVerb::Range { author, from, to } => thehum_range(&author, from, to),
        ThehumVerb::Verify => thehum_verify(),
        ThehumVerb::Replay => thehum_replay(),
    }
}

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
