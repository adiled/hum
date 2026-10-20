//! orchd — the bee supervisor hum drives.
//!
//! orchd renamed its verbs: up/down/restart/status became
//! grow/survey/fell. hum speaks the new names, and keeps its own state
//! dir so orchd's units never mix with an unrelated stack.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Result, bail};

pub const MIN_VERSION: &str = "0.3.4";
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
    c.arg("--orchfile").arg(hum_paths::orchfile())
        .arg("--user")
        .arg("--namespace").arg(hum_paths::orch_namespace())
        .arg("--state-dir").arg(hum_paths::orch_state_dir());
    c
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
    let out = cmd()
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
