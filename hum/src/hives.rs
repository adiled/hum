//! Hives — the kinds a humd is taught.
//!
//! A directory with an `Orchfile` is a hive. hum recognises one unit in
//! it, `hive-install`, runs it at install, and leaves everything else to
//! the hive: what runs, how it is supervised, and which bees it enters.
//! The hive is recorded before `hive-install` runs, so the unit can enter
//! bees under it, and the record is taken back if the unit fails.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::orchfile;

pub fn resolve_dir(reference: &str) -> Result<PathBuf> {
    if let Some(rest) = reference.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(5, '/').collect();
        if parts.len() == 5 && parts[2] == "tree" {
            let (org, repo, branch, sub) = (parts[0], parts[1], parts[3], parts[4]);
            if org == "adiled" && repo == "hum" {
                return Ok(crate::repo_root_or_install_dir().join(sub));
            }
            let cache = hum_paths::foreign_hive_cache(org, repo, branch);
            if !cache.exists() {
                std::fs::create_dir_all(cache.parent().unwrap()).ok();
                let url = format!("https://github.com/{org}/{repo}");
                println!("cloning {url} @ {branch} ...");
                let ok = Command::new("git")
                    .args(["clone", "--depth", "1", "--branch", branch, &url])
                    .arg(&cache)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !ok {
                    bail!("git clone failed: {url}");
                }
            }
            return Ok(cache.join(sub));
        }
        bail!("unrecognized github source URL (want .../tree/<branch>/<path>): {reference}");
    }
    let p = PathBuf::from(reference);
    if p.is_dir() {
        return Ok(p);
    }
    let bundled = catalogue_dir().join(reference);
    if bundled.exists() {
        return Ok(bundled);
    }
    bail!("can't resolve hive '{reference}' (not a bundled name, path, or github source URL)");
}

pub fn catalogue_dir() -> PathBuf {
    crate::repo_root_or_install_dir().join(hum_paths::HIVES_SUBDIR)
}

pub fn catalogue() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(catalogue_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(hum_paths::ORCHFILE_BASENAME).is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    out.sort();
    out
}

pub fn name_of(dir: &Path) -> Result<String> {
    let name = dir
        .canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !hum_paths::is_plain_name(&name) {
        bail!("cannot name a hive after {} — its directory name must be a plain name", dir.display());
    }
    Ok(name)
}

pub fn installed() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(hum_paths::hives_d_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".orch").map(str::to_string)
        })
        .filter(|h| hum_paths::is_plain_name(h))
        .collect();
    out.sort();
    out
}

pub fn install(reference: &str) -> Result<()> {
    let dir = resolve_dir(reference)?;
    let dir = dir.canonicalize().unwrap_or(dir);
    let orchfile = dir.join(hum_paths::ORCHFILE_BASENAME);
    if !orchfile.is_file() {
        bail!("{} is not a hive — it has no Orchfile", dir.display());
    }
    let hive = name_of(&dir)?;
    let body = std::fs::read_to_string(&orchfile)
        .with_context(|| format!("read {}", orchfile.display()))?;
    let resolved = orchfile::resolve(&hive, &dir, &with_source(&body, &hive, &dir))?;

    let dest = hum_paths::hive_installation(&hive);
    let before = std::fs::read(&dest).ok();
    std::fs::create_dir_all(hum_paths::hives_d_dir())?;
    std::fs::write(&dest, with_source(&body, &hive, &dir))
        .with_context(|| format!("write {}", dest.display()))?;

    if let Some(unit) = orchfile::install_unit(&resolved)
        && let Err(e) = run_install_unit(&hive, &dir, unit)
    {
        match before {
            Some(prior) => std::fs::write(&dest, prior)?,
            None => std::fs::remove_file(&dest)?,
        }
        return Err(e);
    }
    println!("installed {hive} ({})", dest.display());
    println!("  this humd now understands bees {hive} enters, wherever they land from");
    Ok(())
}

pub fn uninstall(reference: &str) -> Result<()> {
    let hive = if hum_paths::hive_installed(reference) {
        reference.to_string()
    } else {
        name_of(&resolve_dir(reference)?)?
    };
    let dest = hum_paths::hive_installation(&hive);
    if !dest.is_file() {
        bail!("{hive} is not installed on this humd");
    }
    std::fs::remove_file(&dest).with_context(|| format!("remove {}", dest.display()))?;
    println!("uninstalled {hive}");
    println!("  whatever {hive} started is its own to stop; a running humd understands it until restart");
    Ok(())
}

fn hum_on_path() -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .into_iter()
        .collect();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

fn run_install_unit(hive: &str, dir: &Path, unit: &the_orch::types::Service) -> Result<()> {
    let Some(run) = unit.run_command.as_deref() else {
        bail!("{hive}'s {} unit must RUN on the host", hum_paths::HIVE_INSTALL_UNIT);
    };
    let workdir = unit
        .workdir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.to_path_buf());
    println!("installing {hive}: {run}");
    let status = Command::new("sh")
        .arg("-c")
        .arg(run)
        .current_dir(&workdir)
        .envs(&unit.env)
        .env("HUM_HIVE", hive)
        .env("HUM_SRC", dir)
        .env("PATH", hum_on_path())
        .status()
        .with_context(|| format!("run {hive}'s {}", hum_paths::HIVE_INSTALL_UNIT))?;
    if !status.success() {
        bail!("{hive}'s {} failed ({status}); the hive is not installed", hum_paths::HIVE_INSTALL_UNIT);
    }
    Ok(())
}

pub fn with_source(body: &str, hive: &str, dir: &Path) -> String {
    let mut out = format!("ARG HUM_SRC={}\nARG HUM_HIVE={hive}\n", dir.display());
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("ARG HUM_SRC=") || t.starts_with("ARG HUM_HIVE=") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_and_hive_args_are_set_once_and_first() {
        let body = "ARG HUM_SRC=/stale\nSERVICE a\nRUN ${HUM_SRC}/a\n";
        let out = with_source(body, "a", Path::new("/somewhere/else"));
        assert!(out.starts_with("ARG HUM_SRC=/somewhere/else\nARG HUM_HIVE=a\n"));
        assert_eq!(out.matches("ARG HUM_SRC=").count(), 1);
    }

    #[test]
    fn the_install_unit_is_found_and_resolved_against_the_hive() {
        let body = "SERVICE hive-install\nRUN ${HUM_SRC}/build && echo ${HUM_HIVE}\nONESHOT true\n\nSERVICE whatever\nRUN /bin/true\n";
        let src = Path::new("/hives/dwarf");
        let file = orchfile::resolve("dwarf", src, &with_source(body, "dwarf", src)).unwrap();
        let unit = orchfile::install_unit(&file).expect("hive-install");
        assert_eq!(unit.run_command.as_deref(), Some("/hives/dwarf/build && echo dwarf"));
    }

    #[test]
    fn an_orchfile_with_no_units_is_still_a_hive() {
        let body = "# its bees live on phones\n";
        let src = Path::new("/hives/plantpot");
        let file = orchfile::resolve("plantpot", src, &with_source(body, "plantpot", src)).unwrap();
        assert!(orchfile::install_unit(&file).is_none());
    }
}
