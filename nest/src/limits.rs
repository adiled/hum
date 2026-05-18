//! Per-cell OS-level resource caps — RSS, fds, CPU shares, wall-clock TTL.
//!
//! Defines `ResourceLimits` and an `apply_pre_exec()` helper that wires the
//! caps into a `std::process::Command` via `CommandExt::pre_exec`. On Linux
//! we call `setrlimit(2)` / `setpriority(2)` in the post-fork pre-exec
//! child. On non-Linux the apply is a no-op so callers compile and degrade
//! gracefully.
//!
//! Wall-clock enforcement does NOT live here; the WorkerBee arms its own
//! timer and kills the child. `wall_clock_ms` is stored on the spec only so
//! it travels alongside the rlimit fields.
//!
//! cgroups, seccomp, namespaces: out of scope at this tier — rlimit only.

use serde::{Deserialize, Serialize};

/// OS-level caps for a cell subprocess. All fields optional — `None`
/// means "no extra cap" (inherits from parent / system default).
///
/// Applied via rlimit (setrlimit) before exec on Linux. On non-Linux
/// platforms `apply_pre_exec` is a no-op — caps degrade gracefully.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Hard cap on resident set size in bytes. Maps to RLIMIT_AS on Linux.
    /// Going past this gets the child OOM-killed by the kernel.
    pub rss_bytes: Option<u64>,
    /// Hard cap on number of open file descriptors. RLIMIT_NOFILE.
    pub fd_count: Option<u32>,
    /// Hard cap on CPU seconds. RLIMIT_CPU.
    /// SIGXCPU at soft limit (= hard here), SIGKILL on overrun.
    pub cpu_secs: Option<u32>,
    /// Hard cap on wall-clock execution time in milliseconds. NOT an
    /// rlimit — caller (the WorkerBee) should arm a timer that kills the
    /// cell when this expires. Stored here so the spec travels together.
    pub wall_clock_ms: Option<u64>,
    /// Nice value adjustment (-20..=19). Skipped if None.
    /// Applied via setpriority(2).
    pub nice: Option<i32>,
}

impl ResourceLimits {
    /// Wire the limits into a `std::process::Command` so the child
    /// inherits them at exec time. Linux-only enforcement; on other
    /// platforms returns Ok(()) without doing anything.
    ///
    /// Implementation: uses `Command::pre_exec` (CommandExt) to call
    /// setrlimit() in the forked child before exec(). Failures inside
    /// pre_exec return an io::Error which the spawn surfaces.
    ///
    /// Safety: pre_exec runs in a post-fork, pre-exec child. Must only
    /// call async-signal-safe APIs. setrlimit and setpriority are both
    /// listed in `signal-safety(7)`.
    #[cfg(target_os = "linux")]
    pub fn apply_pre_exec(&self, cmd: &mut std::process::Command) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;

        let snapshot = self.clone();
        // SAFETY: the closure only invokes setrlimit / setpriority, both of
        // which are listed as async-signal-safe in signal-safety(7). No
        // allocations, no locks, no Rust runtime calls.
        unsafe {
            cmd.pre_exec(move || apply_in_child(&snapshot));
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn apply_pre_exec(&self, _cmd: &mut std::process::Command) -> std::io::Result<()> {
        // Non-Linux: no-op. Caps degrade gracefully on macOS/BSD where the
        // rlimit names and semantics differ. Wall-clock and budget caps
        // still apply because they live above the OS layer.
        Ok(())
    }

    /// Quick builder for "tight" defaults — useful for tests and for
    /// safer-by-default deployments. 1 GiB RSS, 64 fds, 60s CPU,
    /// 10-minute wall clock, nice 5.
    pub fn tight() -> Self {
        Self {
            rss_bytes: Some(1024 * 1024 * 1024),
            fd_count: Some(64),
            cpu_secs: Some(60),
            wall_clock_ms: Some(10 * 60 * 1000),
            nice: Some(5),
        }
    }

    /// "Generous" defaults for trusted nests — 8 GiB RSS, 256 fds,
    /// 600s CPU, no wall clock, no nice.
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

/// Post-fork, pre-exec child body. Must only touch async-signal-safe APIs.
#[cfg(target_os = "linux")]
fn apply_in_child(limits: &ResourceLimits) -> std::io::Result<()> {
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
        // setpriority(PRIO_PROCESS, 0=self, nice). errno needs special
        // handling here: setpriority can legitimately return -1 on
        // success, so the convention is to clear errno first and check
        // it after. We use the simpler "set errno via *__errno_location"
        // dance via std::io::Error::last_os_error after clearing.
        // SAFETY: async-signal-safe per signal-safety(7).
        unsafe {
            // Clear errno before the call.
            *libc::__errno_location() = 0;
            let rc = libc::setpriority(libc::PRIO_PROCESS, 0, nice);
            if rc == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(0) {
                    return Err(err);
                }
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
    // SAFETY: setrlimit is async-signal-safe per signal-safety(7).
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
        let d = ResourceLimits::default();
        assert!(d.rss_bytes.is_none());
        assert!(d.fd_count.is_none());
        assert!(d.cpu_secs.is_none());
        assert!(d.wall_clock_ms.is_none());
        assert!(d.nice.is_none());
    }

    #[test]
    fn tight_has_rss_cap() {
        let t = ResourceLimits::tight();
        assert!(t.rss_bytes.is_some());
    }

    #[test]
    fn generous_has_no_wall_clock() {
        assert!(ResourceLimits::generous().wall_clock_ms.is_none());
    }

    #[test]
    fn serialization_roundtrip() {
        let original = ResourceLimits::tight();
        let json = serde_json::to_string(&original).expect("serialize");
        let back: ResourceLimits = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn apply_on_command_returns_ok_for_default() {
        // Install the pre_exec closure; don't spawn. We only verify that
        // wiring it up succeeds — the closure body runs in the child at
        // spawn time, which we deliberately skip to avoid polluting the
        // test process tree.
        let limits = ResourceLimits::default();
        let mut cmd = Command::new("/bin/true");
        assert!(limits.apply_pre_exec(&mut cmd).is_ok());
    }
}
