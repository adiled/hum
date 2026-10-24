//! Per-cell OS-level resource caps — RSS, fds, CPU shares, wall-clock TTL.
//!
//! Defines `Bounds` and an `apply_pre_exec()` helper that wires the
//! caps into a `std::process::Command` via `CommandExt::pre_exec`. On Linux
//! we call `setrlimit(2)` / `setpriority(2)` in the post-fork pre-exec
//! child. On non-Linux the apply is a no-op so callers compile and degrade
//! gracefully.
//!
//! Wall-clock enforcement does NOT live here; the Worker arms its own
//! timer and kills the child. `wall_clock_ms` is stored on the spec only so
//! it travels alongside the rlimit fields.
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
//! - `setrlimit` and `setpriority` are both listed as async-signal-safe
//!   in `signal-safety(7)`.
//! - `*libc::__errno_location()` is a plain memory write.
//! - `std::io::Error::last_os_error()` reads errno; it does not allocate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub rss_bytes: Option<u64>,
    pub fd_count: Option<u32>,
    pub cpu_secs: Option<u32>,
    pub wall_clock_ms: Option<u64>,
    pub nice: Option<i32>,
}

impl Bounds {
    #[cfg(target_os = "linux")]
    pub fn apply_pre_exec(&self, cmd: &mut std::process::Command) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;

        let snapshot = self.clone();
        unsafe {
            cmd.pre_exec(move || apply_in_child(&snapshot));
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply_pre_exec(&self, _cmd: &mut std::process::Command) -> std::io::Result<()> {
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
fn apply_in_child(limits: &Bounds) -> std::io::Result<()> {
    if let Some(rss) = limits.rss_bytes {
        set_rlimit(libc::RLIMIT_AS, rss)?;
    }
    if let Some(fds) = limits.fd_count {
        set_rlimit(libc::RLIMIT_NOFILE, fds as u64)?;
    }
    if let Some(cpu) = limits.cpu_secs {
        set_rlimit(libc::RLIMIT_CPU, cpu as u64)?;
    }
    if let Some(nice) = limits.nice {
        set_priority(nice)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_priority(nice: i32) -> std::io::Result<()> {
    unsafe {
        *libc::__errno_location() = 0;
        let rc = libc::setpriority(libc::PRIO_PROCESS, 0, nice);
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
fn set_rlimit(resource: libc::__rlimit_resource_t, value: u64) -> std::io::Result<()> {
    let rl = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    let rc = unsafe { libc::setrlimit(resource, &rl) };
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
    }
}
