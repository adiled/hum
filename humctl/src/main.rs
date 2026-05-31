//! humctl — humd operator. Bootstrap registers humd as a user service; humctl
//! drives it after that. Bee supervision lives in orchd; use `hum bee` /
//! `hum hive` to talk to it.

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
  humctl thehum
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
        "thehum"  => thehum(),
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

fn thehum() -> Result<()> {
    let dir = hum_paths::thehum_dir();
    println!("thehum dir:    {}", dir.display());
    if !dir.exists() {
        println!("files:         0");
        println!("seq:           0");
        println!("latest day:    (none)");
        println!("snapshots:     0");
        println!("most recent root: (none)");
        println!("total bytes:   0");
        return Ok(());
    }

    let mut ndjson: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 0;
    for ent in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let ent = ent?;
        let ft = ent.file_type()?;
        if ft.is_file() {
            total_bytes += ent.metadata().map(|m| m.len()).unwrap_or(0);
            let path = ent.path();
            if path.extension().and_then(|x| x.to_str()) == Some("ndjson") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ndjson.push(stem.to_string());
                }
            }
        }
    }
    ndjson.sort();
    let latest = ndjson.last().cloned().unwrap_or_else(|| "(none)".to_string());

    let seq: u64 = std::fs::read(hum_paths::thehum_seq_file(&dir))
        .ok()
        .and_then(|b| if b.len() == 8 {
            let mut a = [0u8; 8]; a.copy_from_slice(&b); Some(u64::from_le_bytes(a))
        } else { None })
        .unwrap_or(0);

    let snap_dir = hum_paths::thehum_snapshots_dir(&dir);
    let snap_count = match std::fs::read_dir(&snap_dir) {
        Ok(it) => {
            let mut n: usize = 0;
            for e in it { if e.is_ok() { n += 1; } }
            n
        }
        Err(_) => 0,
    };
    // snapshots/ bytes count toward total too.
    if let Ok(it) = std::fs::read_dir(&snap_dir) {
        for e in it.flatten() {
            if e.file_type().map(|f| f.is_file()).unwrap_or(false) {
                total_bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }

    let root = std::fs::read_to_string(hum_paths::thehum_root_file(&dir))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(none)".to_string());

    println!("files:         {}", ndjson.len());
    println!("seq:           {seq}");
    println!("latest day:    {latest}");
    println!("snapshots:     {snap_count}");
    println!("most recent root: {root}");
    println!("total bytes:   {total_bytes}");
    Ok(())
}

#[cfg(target_os = "macos")]
extern "C" { fn geteuid() -> u32; }
