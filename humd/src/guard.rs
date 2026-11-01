//! The guard: while humd runs, the hum roots hold the layout and nothing
//! else.
//!
//! Every [`SWEEP`] the guard lists each root and moves whatever the layout
//! does not declare into that root's `tmp/`, under the same path it had
//! below the root, so whoever wrote it finds it beside where it wrote it.
//! A writer that still has the file open keeps writing into it there. A
//! config stray moves only once it has been still for [`Root::settle`],
//! because an editor leaves swap files beside what it edits.
//!
//! What has sat in a `tmp/` for [`AGE`] moves on to
//! `~/.hum-archive/<date>/<root>/`, the date being the day it arrived in
//! `tmp/`. A staged write a crash left behind is removed after
//! [`STALE_STAGING`]. Each stray is logged once when it is first moved.
//!
//! The guard starts with the humd binary and never inside `run`, so a
//! simulated humd never polices the real roots.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hum_paths::layout::{self, Root};
use tracing::{debug, info, warn};

pub const SWEEP: Duration = Duration::from_secs(2);
pub const AGE: Duration = Duration::from_secs(7 * 24 * 3600);
pub const STALE_STAGING: Duration = Duration::from_secs(3600);

pub fn spawn() -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("guard".into()).spawn(|| {
        let mut guard = Guard::default();
        info!(sweep_ms = SWEEP.as_millis() as u64, "guard.watching — the hum roots hold the layout");
        loop {
            guard.sweep();
            std::thread::sleep(SWEEP);
        }
    })
}

#[derive(Default)]
pub struct Guard {
    moved: HashMap<PathBuf, u64>,
    stuck: HashSet<PathBuf>,
}

fn still_for(path: &Path, now: SystemTime) -> Duration {
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| now.duration_since(t).ok())
        .unwrap_or(Duration::ZERO)
}

fn arrived(meta: &std::fs::Metadata) -> SystemTime {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.ctime() > 0 {
            return UNIX_EPOCH + Duration::from_secs(meta.ctime() as u64);
        }
    }
    meta.modified().unwrap_or(UNIX_EPOCH)
}

fn unclaimed(target: PathBuf, now: SystemTime) -> PathBuf {
    if std::fs::symlink_metadata(&target).is_err() {
        return target;
    }
    let ms = now.duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    target.with_file_name(format!("{name}.{ms}"))
}

fn day_of(t: SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = t.into();
    local.format("%Y-%m-%d").to_string()
}

impl Guard {
    pub fn sweep(&mut self) {
        self.sweep_at(SystemTime::now());
    }

    pub fn sweep_at(&mut self, now: SystemTime) {
        for root in Root::ALL {
            if !root.dir().is_dir() {
                continue;
            }
            for stray in layout::strays(root) {
                if still_for(&stray, now) >= root.settle() {
                    self.quarantine(root, &stray, now);
                }
            }
            self.age(root, now);
        }
    }

    fn quarantine(&mut self, root: Root, stray: &Path, now: SystemTime) {
        let Ok(rel) = stray.strip_prefix(root.dir()).map(Path::to_path_buf) else {
            return;
        };
        let target = unclaimed(root.tmp().join(&rel), now);
        let moved = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(stray, &target));
        match moved {
            Ok(()) => {
                let times = self.moved.entry(rel.clone()).or_insert(0);
                *times += 1;
                if *times == 1 {
                    warn!(
                        root = root.name(),
                        from = %stray.display(),
                        to = %target.display(),
                        "guard.moved — {} is not in the hum layout; it is in {} now",
                        rel.display(),
                        target.display()
                    );
                } else {
                    debug!(root = root.name(), path = %rel.display(), times = *times, "guard.moved.again");
                }
            }
            Err(e) => {
                if self.stuck.insert(stray.to_path_buf()) {
                    warn!(path = %stray.display(), err = %e, "guard.move.failed — left in place");
                }
            }
        }
    }

    fn age(&mut self, root: Root, now: SystemTime) {
        let tmp = root.tmp();
        let Ok(read) = std::fs::read_dir(&tmp) else {
            return;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(layout::STAGING_PREFIX) {
                if still_for(&path, now) >= STALE_STAGING {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            let when = arrived(&meta);
            if now.duration_since(when).unwrap_or(Duration::ZERO) < AGE {
                continue;
            }
            let target = unclaimed(
                layout::archive_dir().join(day_of(when)).join(root.name()).join(&name),
                now,
            );
            let archived = target
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::rename(&path, &target));
            match archived {
                Ok(()) => info!(from = %path.display(), to = %target.display(), "guard.archived"),
                Err(e) => {
                    if self.stuck.insert(path.clone()) {
                        warn!(path = %path.display(), err = %e, "guard.archive.failed — left in tmp");
                    }
                }
            }
        }
    }
}
