//! The one part of the Orch spec hum reads: a hive's Orchfile, resolved
//! well enough to find and run its `hive-install` unit. Every other unit
//! is left to the hive.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use the_orch::types::{OrchFile, Service};

fn declared_args(body: &str) -> HashMap<String, String> {
    the_orch::parser::parse_raw(body, 0)
        .map(|raw| raw.args.into_iter().collect())
        .unwrap_or_default()
}

pub fn resolve(hive: &str, src: &Path, body: &str) -> Result<OrchFile> {
    let mut args: HashMap<String, String> = declared_args(body)
        .into_keys()
        .filter_map(|k| std::env::var(&k).ok().map(|v| (k, v)))
        .collect();
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
