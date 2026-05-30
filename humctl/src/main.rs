//! humctl — wrap the `service-manager` crate so the hum installer can register
//! and drive the two long-running hum daemons (`humd`, `humnest`) without
//! shelling through a bash compatibility layer.
//!
//! Surface:
//!   humctl install   {humd|humnest}
//!   humctl uninstall {humd|humnest}
//!   humctl start     {humd|humnest}
//!   humctl stop      {humd|humnest}
//!   humctl restart   {humd|humnest}
//!   humctl status    {humd|humnest}
//!
//! Both daemons are registered at `ServiceLevel::User`; the binary path is
//! `~/.local/bin/<name>` (where `cargo install --root ~/.local` lands them).
//! Linux uses systemd --user; macOS uses launchd LaunchAgents. Status / restart
//! aren't in the `service-manager` trait, so we shell out to the native CLI for
//! those — the rest goes through the crate.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{anyhow, bail, Context, Result};
use service_manager::{
    ServiceInstallCtx, ServiceLabel, ServiceLevel, ServiceManager, ServiceStartCtx, ServiceStopCtx,
    ServiceUninstallCtx,
};

const USAGE: &str = "\
humctl — manage hum's user-level services.

Usage:
  humctl install   {humd|humnest}
  humctl uninstall {humd|humnest}
  humctl start     {humd|humnest}
  humctl stop      {humd|humnest}
  humctl restart   {humd|humnest}
  humctl status    {humd|humnest}
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("humctl: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let verb = args.next().ok_or_else(|| anyhow!("{USAGE}"))?;
    if verb == "--help" || verb == "-h" || verb == "help" {
        print!("{USAGE}");
        return Ok(());
    }
    let unit = args.next().ok_or_else(|| anyhow!("{USAGE}"))?;
    let spec = UnitSpec::resolve(&unit)?;

    match verb.as_str() {
        "install" => install(&spec),
        "uninstall" => uninstall(&spec),
        "start" => start(&spec),
        "stop" => stop(&spec),
        "restart" => restart(&spec),
        "status" => status(&spec),
        other => bail!("unknown verb '{other}'\n{USAGE}"),
    }
}

/// One of the two hum daemons we know how to register.
struct UnitSpec {
    /// Service label. We use application-only labels so unit files come out as
    /// `humd.service` / `humnest.service` on Linux and `humd.plist` /
    /// `humnest.plist` on macOS — readable, no hum prefix collision.
    label: ServiceLabel,
    /// Absolute path to the daemon binary. cargo-install lands it at
    /// `$HOME/.local/bin/<name>`.
    program: PathBuf,
}

impl UnitSpec {
    fn resolve(name: &str) -> Result<Self> {
        match name {
            "humd" | "humnest" => Ok(Self {
                label: ServiceLabel {
                    qualifier: None,
                    organization: None,
                    application: name.to_string(),
                },
                program: home_dir()?.join(".local").join("bin").join(name),
            }),
            other => bail!("unknown unit '{other}' (expected humd or humnest)"),
        }
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("$HOME is not set; humctl runs as a user, not as root via sudo"))
}

fn manager() -> Result<Box<dyn ServiceManager>> {
    let mut mgr = <dyn ServiceManager>::native()
        .context("no native service manager available on this OS")?;
    mgr.set_level(ServiceLevel::User)
        .context("service manager does not support user-level services on this OS")?;
    Ok(mgr)
}

fn install(spec: &UnitSpec) -> Result<()> {
    if !spec.program.exists() {
        bail!(
            "binary {} not found — run `cargo install --path {} --root $HOME/.local` first",
            spec.program.display(),
            spec.label.application
        );
    }
    let ctx = ServiceInstallCtx {
        label: spec.label.clone(),
        program: spec.program.clone(),
        args: Vec::<OsString>::new(),
        contents: None,
        username: None,
        working_directory: None,
        environment: None,
        autostart: true,
    };
    manager()?.install(ctx).with_context(|| {
        format!(
            "installing user service '{}' failed",
            spec.label.application
        )
    })
}

fn uninstall(spec: &UnitSpec) -> Result<()> {
    let ctx = ServiceUninstallCtx {
        label: spec.label.clone(),
    };
    manager()?.uninstall(ctx).with_context(|| {
        format!(
            "uninstalling user service '{}' failed",
            spec.label.application
        )
    })
}

fn start(spec: &UnitSpec) -> Result<()> {
    let ctx = ServiceStartCtx {
        label: spec.label.clone(),
    };
    manager()?
        .start(ctx)
        .with_context(|| format!("starting '{}' failed", spec.label.application))
}

fn stop(spec: &UnitSpec) -> Result<()> {
    let ctx = ServiceStopCtx {
        label: spec.label.clone(),
    };
    manager()?
        .stop(ctx)
        .with_context(|| format!("stopping '{}' failed", spec.label.application))
}

fn restart(spec: &UnitSpec) -> Result<()> {
    // service-manager 0.7 has no restart verb on the trait. Stop is best-effort
    // (unit may already be down); start must succeed.
    let _ = stop(spec);
    start(spec)
}

/// `status` isn't on the trait either, so shell out to the native CLI. We keep
/// this thin: print whatever the supervisor says and return its exit code.
fn status(spec: &UnitSpec) -> Result<()> {
    let name = &spec.label.application;
    #[cfg(target_os = "linux")]
    let status = Command::new("systemctl")
        .args(["--user", "status", "--no-pager", name])
        .status()
        .context("failed to spawn systemctl")?;
    #[cfg(target_os = "macos")]
    let status = {
        let uid = unsafe { libc_geteuid() };
        let target = format!("gui/{uid}/{name}");
        Command::new("launchctl")
            .args(["print", &target])
            .status()
            .context("failed to spawn launchctl")?
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let status = bail!("status is only implemented for linux + macos");

    if status.success() {
        Ok(())
    } else {
        // Code 3 from `systemctl status` means "not active" — surface it, but
        // don't decorate it as an error. The caller decides what to do.
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn geteuid() -> u32;
}

#[cfg(target_os = "macos")]
unsafe fn libc_geteuid() -> u32 {
    geteuid()
}
