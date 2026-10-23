//! orchd — the bee supervisor hum drives.
//!
//! orchd renamed its verbs: up/down/restart/status became
//! grow/survey/fell. hum speaks the new names, and keeps its own state
//! dir so orchd's units never mix with an unrelated stack.
//!
//! hum reaches orchd the way orchd's own CONFIG.md says to: a
//! `.orchrc` in hum's project dir carries the namespace and the paths,
//! so nothing lands in the global orchd folders. The Orchfile itself
//! is composed the way the Orch spec composes: hum keeps one base
//! Orchfile that declares the shared variables, and every hive's
//! Orchfile is an overlay on it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub const MIN_VERSION: &str = "0.4.0";
pub const MAX_VERSION: &str = "0.4.0";
pub const REQUIRED_SUBCOMMANDS: [&str; 4] = ["grow", "survey", "fell", "logs"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bee {
    pub name: String,
    pub state: String,
    pub sub: String,
    pub pid: Option<u32>,
}

impl Bee {
    pub fn running(&self) -> bool {
        self.state == "active" && self.pid.is_some()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Job {
    pub state: String,
    pub pid: Option<u32>,
    pub last_exit: Option<i32>,
}

impl Job {
    pub fn running(&self) -> bool {
        self.state == "running" && self.pid != Some(0)
    }
}

pub fn bin() -> Option<PathBuf> {
    let abs = hum_paths::hum_bin("orchd");
    if abs.exists() { Some(abs) } else { None }
}

pub fn cmd() -> Command {
    let mut c = match bin() {
        Some(p) => Command::new(p),
        None => Command::new("orchd"),
    };
    c.arg("--project-dir").arg(hum_paths::orch_project_dir());
    c
}

pub fn with_overlays(mut c: Command, overlays: &[PathBuf]) -> Command {
    for o in overlays {
        c.arg("--overlay").arg(o);
    }
    c
}

pub fn orchrc_body() -> String {
    format!(
        "# Written by hum. orchd reads this from the project dir.\n\
         namespace={ns}\n\
         scope=user\n\
         state_dir={state}\n\
         data_dir={data}\n\
         orchfile={orchfile}\n",
        ns = hum_paths::orch_namespace(),
        state = hum_paths::orch_state_dir().display(),
        data = hum_paths::orch_state_dir().join("data").display(),
        orchfile = hum_paths::orchfile().display(),
    )
}

pub fn base_orchfile_body() -> String {
    format!(
        "# Written by hum. The base every hive Orchfile overlays.\n\
         ARG HOME={home}\n",
        home = hum_paths::home().display(),
    )
}

pub fn ensure_config() -> Result<()> {
    let project = hum_paths::orch_project_dir();
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(hum_paths::orch_d_dir())?;

    let orchrc = hum_paths::orchrc_path();
    let want = orchrc_body();
    if std::fs::read_to_string(&orchrc).ok().as_deref() != Some(want.as_str()) {
        std::fs::write(&orchrc, &want)
            .with_context(|| format!("write {}", orchrc.display()))?;
    }

    let base = hum_paths::orchfile();
    let want = base_orchfile_body();
    if std::fs::read_to_string(&base).ok().as_deref() != Some(want.as_str()) {
        std::fs::write(&base, &want)
            .with_context(|| format!("write {}", base.display()))?;
    }
    Ok(())
}

pub fn overlay_path(kind: &str) -> PathBuf {
    hum_paths::orch_d_dir().join(format!("{kind}.orch"))
}

pub fn overlays() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(hum_paths::orch_d_dir()) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("orch") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn decl_args(text: &str) -> std::collections::HashMap<String, String> {
    match the_orch::parser::parse_raw(text, 0) {
        Ok(raw) => {
            let mut args = std::collections::HashMap::new();
            for (k, v) in raw.args {
                args.insert(k, v);
            }
            args
        }
        Err(_) => std::collections::HashMap::new(),
    }
}

pub fn declared_args(text: &str) -> Vec<String> {
    match the_orch::parser::parse_raw(text, 0) {
        Ok(raw) => {
            let mut names: Vec<String> = raw.args.keys().cloned().collect();
            names.sort();
            names
        }
        Err(_) => Vec::new(),
    }
}

pub fn env_overrides(declared: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    declared
        .iter()
        .filter_map(|(n, _)| std::env::var(n).ok().map(|v| (n.clone(), v)))
        .collect()
}

pub fn parse(files: &[(String, String)], overrides: &std::collections::HashMap<String, String>) -> Result<the_orch::types::OrchFile> {
    the_orch::parse_files(files, overrides).map_err(|errs| {
        let msg: Vec<String> = errs.into_iter().map(|e| e.to_string()).collect();
        anyhow::anyhow!("orchfile: {}", msg.join("; "))
    })
}

pub fn parse_file(path: &std::path::Path) -> Result<the_orch::types::OrchFile> {
    let body = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    parse(&[(path.display().to_string(), body)], &std::collections::HashMap::new())
}

pub fn services_in(path: &std::path::Path) -> Result<Vec<String>> {
    Ok(parse_file(path)?.services.into_iter().map(|s| s.name).collect())
}

pub fn overlay_for_service(service: &str) -> Result<std::path::PathBuf> {
    for overlay in overlays() {
        if services_in(&overlay)?.iter().any(|s| s == service) {
            return Ok(overlay);
        }
    }
    anyhow::bail!("no registered hive declares {service}")
}

pub fn base_and_overlay(kind: &str) -> Result<(String, String)> {
    let base = std::fs::read_to_string(hum_paths::orchfile())
        .with_context(|| format!("read {}", hum_paths::orchfile().display()))?;
    let overlay_path = overlay_path(kind);
    let overlay = std::fs::read_to_string(&overlay_path)
        .with_context(|| format!("read {}", overlay_path.display()))?;
    Ok((base, overlay))
}

pub fn grow(service: &str, overlay: &Path) -> Result<()> {
    let overlay_body = std::fs::read_to_string(overlay)
        .with_context(|| format!("read {}", overlay.display()))?;
    let base = std::fs::read_to_string(hum_paths::orchfile())
        .with_context(|| format!("read {}", hum_paths::orchfile().display()))?;
    let mut args = decl_args(&overlay_body);
    let base_args = decl_args(&std::fs::read_to_string(hum_paths::orchfile())
        .with_context(|| format!("read {}", hum_paths::orchfile().display()))?);
    for (k, v) in base_args {
        args.entry(k).or_insert(v);
    }
    let overrides = env_overrides(&args);
    let mut c = with_overlays(cmd(), &[overlay.to_path_buf()]);
    for (k, v) in &overrides {
        c.arg("--arg").arg(format!("{k}={v}"));
    }
    let status = c
        .arg("grow")
        .arg(service)
        .status()
        .map_err(|e| anyhow::anyhow!("orchd not found: {e}"))?;
    if !status.success() {
        bail!("orchd grow {service} failed");
    }
    Ok(())
}

pub fn unload(service: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        let plist = hum_paths::home()
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", label(service)));
        let status = Command::new("launchctl")
            .arg("bootout")
            .arg(format!("gui/{}", uid()))
            .arg(&plist)
            .status()
            .map_err(|e| anyhow::anyhow!("launchctl bootout: {e}"))?;
        if !status.success() {
            bail!("launchctl bootout {} failed", label(service));
        }
    } else {
        let status = Command::new("systemctl")
            .arg("--user")
            .arg("stop")
            .arg(unit(service))
            .status()
            .map_err(|e| anyhow::anyhow!("systemctl stop: {e}"))?;
        if !status.success() {
            bail!("systemctl stop {} failed", unit(service));
        }
    }
    Ok(())
}


pub fn label(kind: &str) -> String {
    format!("{}.{}", hum_paths::orch_namespace(), kind)
}

pub fn unit(kind: &str) -> String {
    format!("{}-{}.service", hum_paths::orch_namespace(), kind)
}

pub fn version() -> Option<String> {
    let out = Command::new(bin()?).arg("--version").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn help_text() -> Option<String> {
    let out = Command::new(bin()?).arg("--help").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn missing_subcommands() -> Vec<String> {
    let help = match help_text() {
        Some(h) => h,
        None => return REQUIRED_SUBCOMMANDS.iter().map(|s| s.to_string()).collect(),
    };
    REQUIRED_SUBCOMMANDS
        .iter()
        .filter(|s| !help_has_subcommand(&help, s))
        .map(|s| s.to_string())
        .collect()
}

fn help_has_subcommand(help: &str, sub: &str) -> bool {
    help.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(sub) && t[sub.len()..].starts_with(char::is_whitespace)
    })
}

pub fn version_tuple(v: &str) -> Vec<u32> {
    v.split(|c: char| c == '.' || !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<u32>().ok())
        .collect()
}

pub fn version_at_least(found: &str, min: &str) -> bool {
    let f = version_tuple(found);
    let m = version_tuple(min);
    for (i, mv) in m.iter().enumerate() {
        let fv = f.get(i).copied().unwrap_or(0);
        if fv > *mv { return true; }
        if fv < *mv { return false; }
    }
    true
}

pub fn survey() -> Result<BTreeMap<String, Bee>> {
    let out = with_overlays(cmd(), &overlays())
        .arg("survey")
        .arg("--json")
        .output()
        .map_err(|e| anyhow::anyhow!("orchd not found: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!("orchd survey failed: {err}");
    }
    parse_survey(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_survey(raw: &str) -> Result<BTreeMap<String, Bee>> {
    let v: serde_json::Value = serde_json::from_str(raw.trim())
        .map_err(|e| anyhow::anyhow!("orchd survey json: {e}"))?;
    let arr = match v.as_array() {
        Some(a) => a,
        None => bail!("orchd survey json is not an array"),
    };
    let mut out = BTreeMap::new();
    for item in arr {
        let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if name.is_empty() { continue; }
        out.insert(
            name.clone(),
            Bee {
                name,
                state: item.get("state").and_then(|x| x.as_str()).unwrap_or("unknown").to_string(),
                sub: item.get("sub").and_then(|x| x.as_str()).unwrap_or("unknown").to_string(),
                pid: item.get("pid").and_then(|x| x.as_u64()).map(|p| p as u32),
            },
        );
    }
    Ok(out)
}

pub fn uid() -> u32 {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

pub fn job(kind: &str) -> Option<Job> {
    if cfg!(target_os = "macos") {
        let out = Command::new("launchctl")
            .arg("print")
            .arg(format!("gui/{}/{}", uid(), label(kind)))
            .output()
            .ok()?;
        if !out.status.success() { return None; }
        Some(parse_launchctl_print(&String::from_utf8_lossy(&out.stdout)))
    } else {
        let code = Command::new("systemctl")
            .arg("--user")
            .arg("show")
            .arg(unit(kind))
            .arg("--property=ExecMainStatus")
            .arg("--value")
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i32>().ok());
        let pid = Command::new("systemctl")
            .arg("--user")
            .arg("show")
            .arg(unit(kind))
            .arg("--property=MainPID")
            .arg("--value")
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok());
        let state = Command::new("systemctl")
            .arg("--user")
            .arg("is-active")
            .arg(unit(kind))
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let state = normalize_systemd_state(&state);
        Some(Job {
            state,
            pid,
            last_exit: code,
        })
    }
}

pub fn normalize_systemd_state(state: &str) -> String {
    if state == "active" { "running".to_string() } else { state.to_string() }
}

pub fn parse_launchctl_print(raw: &str) -> Job {
    let mut job = Job {
        state: "unknown".to_string(),
        pid: None,
        last_exit: None,
    };
    for line in raw.lines() {
        let t = line.trim();
        if job.state == "unknown"
            && let Some(v) = t.strip_prefix("state =")
        {
            job.state = v.trim().trim_end_matches(',').to_string();
        }
        if job.pid.is_none()
            && let Some(v) = t.strip_prefix("pid =")
            && let Ok(p) = v.trim().trim_end_matches(',').parse::<u32>()
            && p > 0
        {
            job.pid = Some(p);
        }
        if job.last_exit.is_none()
            && let Some(v) = t.strip_prefix("last exit code =")
            && let Ok(c) = v.trim().trim_end_matches(',').parse::<i32>()
        {
            job.last_exit = Some(c);
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survey_json_maps_name_to_state() {
        let raw = r#"[{"name":"humfs","state":"active","sub":"running","pid":42},
                       {"name":"wryme-hive","state":"inactive","sub":"dead","pid":null}]"#;
        let map = parse_survey(raw).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map["humfs"].running());
        assert_eq!(map["humfs"].pid, Some(42));
        assert!(!map["wryme-hive"].running());
        assert_eq!(map["wryme-hive"].pid, None);
    }

    #[test]
    fn survey_json_rejects_non_array() {
        assert!(parse_survey("{}").is_err());
        assert!(parse_survey("not json").is_err());
    }

    #[test]
    fn launchctl_first_state_wins() {
        let raw = "	path = /Users/x/Library/LaunchAgents/humd.plist\n\
                   state = running\n\
                   pid = 791\n\
                   last exit code = (never exited)\n\
                   \t\tstate = active\n\
                   \t\tstate = active\n";
        let job = parse_launchctl_print(raw);
        assert_eq!(job.state, "running");
        assert_eq!(job.pid, Some(791));
        assert_eq!(job.last_exit, None);
        assert!(job.running());
    }

    #[test]
    fn systemd_state_normalizes_to_running() {
        assert_eq!(normalize_systemd_state("active"), "running");
        assert_eq!(normalize_systemd_state("inactive"), "inactive");
        assert_eq!(normalize_systemd_state("failed"), "failed");
    }

    #[test]
    fn launchctl_reads_last_exit_code() {
        let raw = "	state = not running\n\
                   \n\
                   last exit code = 1\n";
        let job = parse_launchctl_print(raw);
        assert_eq!(job.state, "not running");
        assert_eq!(job.last_exit, Some(1));
        assert!(!job.running());
    }

    #[test]
    fn version_gate_orders_minor_and_patch() {
        assert!(version_at_least("0.3.4", "0.3.4"));
        assert!(version_at_least("0.3.5", "0.3.4"));
        assert!(version_at_least("0.4.0", "0.3.4"));
        assert!(version_at_least("1.0.0", "0.3.4"));
        assert!(!version_at_least("0.3.3", "0.3.4"));
        assert!(!version_at_least("0.2.99", "0.3.4"));
    }

    #[test]
    fn version_tuple_reads_dotted_versions() {
        assert_eq!(version_tuple("orchd 0.3.4"), vec![0, 3, 4]);
        assert_eq!(version_tuple("0.3.4"), vec![0, 3, 4]);
    }

    #[test]
    fn help_subcommand_detection() {
        let help = "Commands:\n  grow    Bring the grove up\n  survey  Report the grove\n";
        assert!(help_has_subcommand(help, "grow"));
        assert!(help_has_subcommand(help, "survey"));
        assert!(!help_has_subcommand(help, "fell"));
    }
}
