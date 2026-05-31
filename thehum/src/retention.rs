//! Retention enforcement. Run periodically; obey the configured mode.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, Utc};

use crate::{RetentionMode, TheHum};

impl TheHum {
    /// Apply retention policy. Idempotent. Safe to call repeatedly.
    pub fn enforce_retention(&self) -> Result<RetentionReport> {
        match self.cfg.mode {
            RetentionMode::Archive => Ok(RetentionReport::default()),
            RetentionMode::Rolling => self.prune_older_than_days(self.cfg.days),
            RetentionMode::Light => self.prune_to_snapshots_only(),
        }
    }

    fn prune_older_than_days(&self, days: u32) -> Result<RetentionReport> {
        let cutoff = (Utc::now() - Duration::days(days as i64)).date_naive();
        let mut report = RetentionReport::default();
        for path in daily_files(&self.dir)? {
            let Some(file_day) = parse_ndjson_date(&path) else { continue };
            if file_day < cutoff {
                std::fs::remove_file(&path)
                    .with_context(|| format!("remove {}", path.display()))?;
                report.removed_files += 1;
            } else {
                report.kept_files += 1;
            }
        }
        Ok(report)
    }

    fn prune_to_snapshots_only(&self) -> Result<RetentionReport> {
        let mut files = daily_files(&self.dir)?;
        files.sort();
        let keep = files.pop();
        let mut report = RetentionReport::default();
        for path in files {
            std::fs::remove_file(&path)?;
            report.removed_files += 1;
        }
        if keep.is_some() { report.kept_files = 1; }
        Ok(report)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RetentionReport {
    pub removed_files: u32,
    pub kept_files: u32,
}

fn daily_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    Ok(std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ndjson"))
        .collect())
}

fn parse_ndjson_date(path: &Path) -> Option<NaiveDate> {
    let stem = path.file_stem()?.to_str()?;
    NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    fn mk(dir: &Path, mode: RetentionMode) -> TheHum {
        let key = SigningKey::generate(&mut OsRng);
        let mut cfg = crate::Config::default();
        cfg.mode = mode;
        cfg.days = 7;
        TheHum::open(dir, key, cfg).unwrap()
    }

    #[test]
    fn archive_keeps_everything() {
        let tmp = TempDir::new().unwrap();
        for day in &["2020-01-01", "2024-06-15", "2026-05-31"] {
            std::fs::write(tmp.path().join(format!("{day}.ndjson")), "").unwrap();
        }
        let t = mk(tmp.path(), RetentionMode::Archive);
        let r = t.enforce_retention().unwrap();
        assert_eq!(r.removed_files, 0);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().filter(|e| {
            e.as_ref().unwrap().path().extension().and_then(|x| x.to_str()) == Some("ndjson")
        }).count(), 3);
    }

    #[test]
    fn rolling_drops_old_days() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("2020-01-01.ndjson"), "").unwrap();
        std::fs::write(tmp.path().join("2020-01-02.ndjson"), "").unwrap();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        std::fs::write(tmp.path().join(format!("{today}.ndjson")), "").unwrap();
        let t = mk(tmp.path(), RetentionMode::Rolling);
        let r = t.enforce_retention().unwrap();
        assert_eq!(r.removed_files, 2);
        assert_eq!(r.kept_files, 1);
    }

    #[test]
    fn light_keeps_only_most_recent() {
        let tmp = TempDir::new().unwrap();
        for day in &["2020-01-01", "2024-06-15", "2026-05-31"] {
            std::fs::write(tmp.path().join(format!("{day}.ndjson")), "").unwrap();
        }
        let t = mk(tmp.path(), RetentionMode::Light);
        let r = t.enforce_retention().unwrap();
        assert_eq!(r.removed_files, 2);
        assert_eq!(r.kept_files, 1);
    }
}
