//! humctl — humd operator. Bootstrap registers humd as a user service; humctl
//! drives it after that. humnest is opaque here; observe it through `hum nest`
//! and `hum bee info`.

use std::process::{Command, ExitCode};

use anyhow::{anyhow, bail, Context, Result};
use service_manager::{
    ServiceLabel, ServiceLevel, ServiceManager, ServiceStartCtx, ServiceStopCtx,
};

const USAGE: &str = "\
humctl — operate the humd daemon.

Usage:
  humctl start
  humctl stop
  humctl restart
  humctl status
  humctl logs   [-n LINES]
  humctl health
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => { eprintln!("humctl: {e:#}"); ExitCode::from(1) }
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let verb = args.next().ok_or_else(|| anyhow!("{USAGE}"))?;
    if matches!(verb.as_str(), "--help" | "-h" | "help") { print!("{USAGE}"); return Ok(()); }
    match verb.as_str() {
        "start"   => start(),
        "stop"    => stop(),
        "restart" => { let _ = stop(); start() }
        "status"  => status(),
        "logs"    => logs(parse_lines(args.collect::<Vec<_>>())),
        "health"  => health(),
        other     => bail!("unknown verb '{other}'\n{USAGE}"),
    }
}

fn parse_lines(rest: Vec<String>) -> u32 {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "-n" || a == "--lines" {
            if let Some(v) = it.next() { if let Ok(n) = v.parse() { return n; } }
        }
    }
    200
}

fn label() -> ServiceLabel {
    ServiceLabel { qualifier: None, organization: None, application: "humd".to_string() }
}

fn manager() -> Result<Box<dyn ServiceManager>> {
    let mut mgr = <dyn ServiceManager>::native()
        .context("no native service manager available on this OS")?;
    mgr.set_level(ServiceLevel::User)
        .context("service manager does not support user-level services on this OS")?;
    Ok(mgr)
}

fn start()  -> Result<()> { manager()?.start(ServiceStartCtx { label: label() }).context("start humd") }
fn stop()   -> Result<()> { manager()?.stop(ServiceStopCtx  { label: label() }).context("stop humd") }

fn status() -> Result<()> {
    #[cfg(target_os = "linux")]
    let s = Command::new("systemctl").args(["--user", "status", "--no-pager", "humd"]).status()?;
    #[cfg(target_os = "macos")]
    let s = {
        let uid = unsafe { geteuid() };
        Command::new("launchctl").args(["print", &format!("gui/{uid}/humd")]).status()?
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let s = bail!("status is only implemented for linux + macos");
    if !s.success() { std::process::exit(s.code().unwrap_or(1)); }
    Ok(())
}

fn logs(lines: u32) -> Result<()> {
    match hum_paths::daemon_logs("humd") {
        hum_paths::DaemonLogs::Journald { unit } => {
            Command::new("journalctl")
                .args(["--user", "-u", &unit, "--no-pager", "-n", &lines.to_string()])
                .status().context("journalctl")?;
        }
        hum_paths::DaemonLogs::Files { stdout, stderr } => {
            Command::new("tail").args(["-n", &lines.to_string()])
                .arg(stdout).arg(stderr).status().context("tail")?;
        }
    }
    Ok(())
}

fn health() -> Result<()> {
    let sock = hum_paths::thrum_sock_resolved();
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    if !sock.exists() { bail!("socket file missing: {}", sock.display()); }
    let mut s = UnixStream::connect(&sock).with_context(|| format!("connect {}", sock.display()))?;
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    s.set_write_timeout(Some(Duration::from_secs(1)))?;
    s.write_all(b"{\"chi\":\"hello\",\"sid\":\"humctl-health\",\"bee\":[\"worker\"]}\n")?;
    let mut buf = [0u8; 256];
    match s.read(&mut buf) {
        Ok(0) => bail!("socket closed without breath"),
        Ok(_) => { println!("humd: ✓ live at {}", sock.display()); Ok(()) }
        Err(e) => bail!("no breath within 1s: {e}"),
    }
}

#[cfg(target_os = "macos")]
extern "C" { fn geteuid() -> u32; }
