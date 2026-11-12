//! The one part of the Orch spec hum reads: a hive's Orchfile, resolved
//! well enough to find and run its `hive-install` unit. Every other unit
//! is left to the hive.
//!
//! Orch 0.3 takes runtime values as ARG overrides and applies only those
//! the Orchfile declares, so the whole environment goes in and a declared
//! ARG takes its value from it, the way orchd resolves the same file.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use the_orch::types::{OrchFile, Service};

pub fn resolve(hive: &str, src: &Path, body: &str) -> Result<OrchFile> {
    let env = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
    resolve_in(hive, src, body, env)
}

fn resolve_in(
    hive: &str,
    src: &Path,
    body: &str,
    env: impl IntoIterator<Item = (String, String)>,
) -> Result<OrchFile> {
    let mut args: HashMap<String, String> = env.into_iter().collect();
    args.insert("HOME".into(), hum_paths::home().display().to_string());
    args.insert("HUM_SRC".into(), src.display().to_string());
    args.insert("HUM_HIVE".into(), hive.to_string());
    the_orch::parse_files(&[(format!("{hive}/Orchfile"), body.to_string())], &args).map_err(|errs| {
        let msg: Vec<String> = errs.into_iter().map(|e| e.to_string()).collect();
        anyhow!("{hive}'s Orchfile: {}", msg.join("; "))
    })
}

pub fn install_unit(file: &OrchFile) -> Option<&Service> {
    file.services
        .iter()
        .find(|s| s.name == hum_paths::HIVE_INSTALL_UNIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "ARG PORT=1\nSERVICE hive-install\nRUN serve ${PORT} ${HUM_SRC}\nONESHOT true\n";

    fn run_of(file: &OrchFile) -> Option<&str> {
        install_unit(file).and_then(|u| u.run_command.as_deref())
    }

    #[test]
    fn a_declared_arg_takes_its_value_from_the_environment() {
        let body = format!("ARG HUM_SRC=/x\n{BODY}");
        let env = [("PORT".to_string(), "9".to_string())];
        let file = resolve_in("h", Path::new("/hives/h"), &body, env).unwrap();
        assert_eq!(run_of(&file), Some("serve 9 /hives/h"));
    }

    #[test]
    fn a_declared_arg_the_environment_lacks_keeps_its_default() {
        let body = format!("ARG HUM_SRC=/x\n{BODY}");
        let file = resolve_in("h", Path::new("/hives/h"), &body, []).unwrap();
        assert_eq!(run_of(&file), Some("serve 1 /hives/h"));
    }

    #[test]
    fn the_environment_adds_no_arg_the_orchfile_never_declared() {
        let body = format!("ARG HUM_SRC=/x\n{BODY}");
        let env = [("STRAY".to_string(), "leak".to_string())];
        let file = resolve_in("h", Path::new("/hives/h"), &body, env).unwrap();
        assert!(!file.args.contains_key("STRAY"), "args: {:?}", file.args);
    }

    #[test]
    fn hum_sets_the_source_over_whatever_the_environment_says() {
        let body = format!("ARG HUM_SRC=/x\n{BODY}");
        let env = [("HUM_SRC".to_string(), "/stale".to_string())];
        let file = resolve_in("h", Path::new("/hives/h"), &body, env).unwrap();
        assert_eq!(run_of(&file), Some("serve 1 /hives/h"));
    }
}
