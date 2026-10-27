//! Per-cell OS-level resource caps — address space, fds, CPU time,
//! niceness — and the wall clock the nest keeps.
//!
//! Defines `Bounds` and an `apply_pre_exec()` helper that wires the
//! caps into a `std::process::Command` via `CommandExt::pre_exec`. On Linux
//! we call `prlimit(2)` / `setpriority(2)` in the post-fork pre-exec
//! child. `apply_to()` lays the same caps on a process already running,
//! for a spawner that takes no `pre_exec`, so they land just after its
//! exec rather than before. On non-Linux both are no-ops so callers
//! compile and degrade gracefully.
//!
//! Each rlimit is set soft and hard alike, on the one process; what it
//! spawns inherits the limit, and counts against it on its own.
//!
//! `wall_clock_ms` is no rlimit: `serve_nurse` ends a cell that outlives
//! it, on every platform.
//!
//! cgroups, seccomp, namespaces: out of scope at this tier — rlimit only.
//!
//! # Safety
//!
//! The `unsafe` blocks below run in a post-fork, pre-exec child, where
//! only async-signal-safe APIs may be called. Each one is confined to a
//! single libc call, and nothing between the fork and the exec allocates,
//! takes a lock, or touches the Rust runtime:
//!
//! - `prlimit` and `setpriority` are each a bare system call, taking no
//!   lock and allocating nothing.
//! - `*libc::__errno_location()` is a plain memory write.
//! - `std::io::Error::last_os_error()` reads errno; it does not allocate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    /// Caps the process's virtual address space (`RLIMIT_AS`), not its
    /// resident set: every mapping counts, touched or not, and one past it
    /// fails with `ENOMEM`.
    pub rss_bytes: Option<u64>,
    pub fd_count: Option<u32>,
    pub cpu_secs: Option<u32>,
    /// How long a cell may live from its raise before the nest ends it.
    pub wall_clock_ms: Option<u64>,
    pub nice: Option<i32>,
}

impl Bounds {
    #[cfg(target_os = "linux")]
    pub fn apply_pre_exec(&self, cmd: &mut std::process::Command) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;

        let snapshot = self.clone();
        unsafe {
            cmd.pre_exec(move || apply(&snapshot, 0));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn apply_to(&self, pid: u32) -> std::io::Result<()> {
        apply(self, pid as libc::pid_t)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply_pre_exec(&self, _cmd: &mut std::process::Command) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply_to(&self, _pid: u32) -> std::io::Result<()> {
        Ok(())
    }

    pub fn tight() -> Self {
        Self {
            rss_bytes: Some(1024 * 1024 * 1024),
            fd_count: Some(64),
            cpu_secs: Some(60),
            wall_clock_ms: Some(10 * 60 * 1000),
            nice: Some(5),
        }
    }

    pub fn generous() -> Self {
        Self {
            rss_bytes: Some(8 * 1024 * 1024 * 1024),
            fd_count: Some(256),
            cpu_secs: Some(600),
            wall_clock_ms: None,
            nice: None,
        }
    }
}

#[cfg(target_os = "linux")]
fn apply(limits: &Bounds, pid: libc::pid_t) -> std::io::Result<()> {
    if let Some(bytes) = limits.rss_bytes {
        set_rlimit(pid, libc::RLIMIT_AS, bytes)?;
    }
    if let Some(fds) = limits.fd_count {
        set_rlimit(pid, libc::RLIMIT_NOFILE, fds as u64)?;
    }
    if let Some(cpu) = limits.cpu_secs {
        set_rlimit(pid, libc::RLIMIT_CPU, cpu as u64)?;
    }
    if let Some(nice) = limits.nice {
        set_priority(pid, nice)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_priority(pid: libc::pid_t, nice: i32) -> std::io::Result<()> {
    unsafe {
        *libc::__errno_location() = 0;
        let rc = libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice);
        if rc == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(0) {
                return Err(err);
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_rlimit(pid: libc::pid_t, resource: libc::__rlimit_resource_t, value: u64) -> std::io::Result<()> {
    let rl = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    let rc = unsafe { libc::prlimit(pid, resource, &rl, std::ptr::null_mut()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn default_is_empty() {
        let d = Bounds::default();
        assert!(d.rss_bytes.is_none());
        assert!(d.fd_count.is_none());
        assert!(d.cpu_secs.is_none());
        assert!(d.wall_clock_ms.is_none());
        assert!(d.nice.is_none());
    }

    #[test]
    fn tight_has_rss_cap() {
        let t = Bounds::tight();
        assert!(t.rss_bytes.is_some());
    }

    #[test]
    fn generous_has_no_wall_clock() {
        assert!(Bounds::generous().wall_clock_ms.is_none());
    }

    #[test]
    fn serialization_roundtrip() {
        let original = Bounds::tight();
        let json = serde_json::to_string(&original).expect("serialize");
        let back: Bounds = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn apply_on_command_returns_ok_for_default() {
        let limits = Bounds::default();
        let mut cmd = Command::new("/bin/true");
        assert!(limits.apply_pre_exec(&mut cmd).is_ok());
        assert!(limits.apply_to(std::process::id()).is_ok());
    }

    #[cfg(target_os = "linux")]
    const TIGHT: [&str; 4] = ["1073741824 1073741824", "64 64", "60 60", "5"];

    #[cfg(target_os = "linux")]
    fn bounds_of(pid: u32) -> Vec<String> {
        let limits = std::fs::read_to_string(format!("/proc/{pid}/limits")).expect("limits");
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("stat");
        let mut seen: Vec<String> = ["Max address space", "Max open files", "Max cpu time"].iter().map(|name| {
            let line = limits.lines().find(|l| l.starts_with(name)).expect(name);
            line[name.len()..].split_whitespace().take(2).collect::<Vec<_>>().join(" ")
        }).collect();
        seen.push(stat.rsplit_once(')').expect("stat").1.split_whitespace().nth(16).expect("nice").to_string());
        seen
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rss_bytes_caps_the_address_space_before_exec() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        Bounds::tight().apply_pre_exec(&mut cmd).expect("apply");
        let mut child = cmd.spawn().expect("spawn sleep");
        let seen = bounds_of(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(seen, TIGHT);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_running_process_takes_the_same_bounds() {
        let mut child = Command::new("sleep").arg("5").spawn().expect("spawn sleep");
        let applied = Bounds::tight().apply_to(child.id());
        let seen = bounds_of(child.id());
        let _ = child.kill();
        let _ = child.wait();
        applied.expect("apply");
        assert_eq!(seen, TIGHT);
    }
}
