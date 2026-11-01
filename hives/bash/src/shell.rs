//! One bash shell, kept for a sigil.
//!
//! The shell is `bash --noprofile --norc`, started in the directory the
//! bloom builds in with the builder's own environment, in a process group
//! of its own, its stderr joined to its stdout. A command is handed over
//! as text, read whole by `read` from a quoted heredoc and run by `eval`
//! with its stdin from `/dev/null`, so `cd` and `export` stay with the
//! shell, a command that waits for input gets none, and an unclosed quote
//! fails at once rather than swallowing the next command. A random marker
//! printed after it, with the exit code, ends its output.
//!
//! Output past [`OUTPUT_MOST`] bytes keeps its first third and its last
//! two thirds, and says how much was cut between them. A command that
//! outlives its timeout takes the shell with it: the whole process group
//! is killed, so nothing it started lingers. One that exits the shell
//! ends it the same way, and either way the next command gets a new one.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub const OUTPUT_MOST: usize = 30_000;

const KEPT_WHILE_READING: usize = 4 * 1024 * 1024;

pub struct Shell {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    started_in: PathBuf,
    used: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ran {
    pub output: String,
    pub exit: i32,
    pub alive: bool,
}

impl Shell {
    pub async fn start(dir: &Path) -> Result<Shell> {
        if !dir.is_dir() {
            return Err(anyhow!("{} is not a directory on this host", dir.display()));
        }
        let mut child = Command::new("bash")
            .arg("--noprofile")
            .arg("--norc")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .context("start bash")?;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("bash has no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("bash has no stdout"))?;
        stdin.write_all(b"exec 2>&1\n").await.context("join stderr to stdout")?;
        Ok(Shell { child, stdin, stdout, started_in: dir.to_path_buf(), used: Instant::now() })
    }

    pub fn started_in(&self) -> &Path {
        &self.started_in
    }

    pub fn idle_for(&self) -> Duration {
        self.used.elapsed()
    }

    pub async fn run(&mut self, command: &str, timeout: Duration) -> Result<Option<Ran>> {
        self.used = Instant::now();
        let marker = format!("__hum_bash_{:016x}__", rand::random::<u64>());
        let script = format!(
            "IFS= read -r -d '' __hum_bash <<'{marker}'\n{command}\n{marker}\neval \"$__hum_bash\" < /dev/null\nprintf '\\n{marker}%d\\n' \"$?\"\nunset __hum_bash\n"
        );
        self.stdin.write_all(script.as_bytes()).await.context("hand bash the command")?;
        self.stdin.flush().await.context("hand bash the command")?;
        let read = tokio::time::timeout(timeout, read_until(&mut self.stdout, &marker)).await;
        self.used = Instant::now();
        let Ok(read) = read else { return Ok(None) };
        let (bytes, exit, cut) = read?;
        let (exit, alive) = match exit {
            Some(code) => (code, true),
            None => (self.child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1), false),
        };
        Ok(Some(Ran { output: present(&bytes, cut, exit, alive), exit, alive }))
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

async fn read_until(out: &mut ChildStdout, marker: &str) -> Result<(Vec<u8>, Option<i32>, usize)> {
    let needle = format!("\n{marker}").into_bytes();
    let mut kept: Vec<u8> = Vec::new();
    let mut cut = 0usize;
    let mut searched = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        let n = out.read(&mut chunk).await.context("read bash")?;
        if n == 0 {
            return Ok((kept, None, cut));
        }
        kept.extend_from_slice(&chunk[..n]);
        let from = searched.saturating_sub(needle.len());
        if let Some(at) = find(&kept[from..], &needle).map(|i| i + from) {
            let rest = &kept[at + needle.len()..];
            if let Some(nl) = rest.iter().position(|b| *b == b'\n') {
                let code = std::str::from_utf8(&rest[..nl]).ok().and_then(|s| s.trim().parse().ok());
                kept.truncate(at);
                return Ok((kept, code.or(Some(-1)), cut));
            }
            searched = at;
        } else {
            searched = kept.len();
        }
        if kept.len() > KEPT_WHILE_READING {
            let head = OUTPUT_MOST / 3;
            let drop_to = kept.len() - OUTPUT_MOST;
            cut += drop_to - head;
            kept.drain(head..drop_to);
            searched = searched.saturating_sub(drop_to - head);
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn present(bytes: &[u8], cut: usize, exit: i32, alive: bool) -> String {
    let head = OUTPUT_MOST / 3;
    let tail = OUTPUT_MOST - head;
    let over = bytes.len().saturating_sub(OUTPUT_MOST);
    let mut text = if over > 0 {
        format!(
            "{}\n[… {} bytes cut …]\n{}",
            String::from_utf8_lossy(&bytes[..head]),
            cut + over,
            String::from_utf8_lossy(&bytes[bytes.len() - tail..]),
        )
    } else if cut > 0 {
        format!("{}\n[… {cut} bytes cut …]", String::from_utf8_lossy(bytes))
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    if !alive {
        text.push_str(&format!("\n(the shell exited with code {exit}; the next command starts a new one)"));
    } else if exit != 0 {
        text.push_str(&format!("\n(exit code {exit})"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(10);

    async fn shell() -> (tempfile::TempDir, Shell) {
        let dir = tempfile::tempdir().unwrap();
        let shell = Shell::start(dir.path()).await.unwrap();
        (dir, shell)
    }

    #[tokio::test]
    async fn the_shell_keeps_its_directory_and_environment() {
        let (dir, mut sh) = shell().await;
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let first = sh.run("cd sub && export HUM_SEEN=yes", LONG).await.unwrap().unwrap();
        assert_eq!(first.exit, 0);
        let second = sh.run("pwd; echo $HUM_SEEN", LONG).await.unwrap().unwrap();
        assert!(second.output.trim_end().ends_with("yes"), "{}", second.output);
        assert!(second.output.contains("/sub"), "{}", second.output);
    }

    #[tokio::test]
    async fn stdout_and_stderr_come_back_together_with_the_exit_code() {
        let (_dir, mut sh) = shell().await;
        let ran = sh.run("echo out; echo err >&2; false", LONG).await.unwrap().unwrap();
        assert_eq!(ran.exit, 1);
        assert!(ran.alive);
        assert!(ran.output.contains("out") && ran.output.contains("err"), "{}", ran.output);
        assert!(ran.output.ends_with("(exit code 1)"), "{}", ran.output);
    }

    #[tokio::test]
    async fn a_command_waiting_for_input_gets_none() {
        let (_dir, mut sh) = shell().await;
        let ran = sh.run("cat; read line; echo got:$line", LONG).await.unwrap().unwrap();
        assert_eq!(ran.output.trim(), "got:");
    }

    #[tokio::test]
    async fn an_unclosed_quote_fails_without_eating_the_next_command() {
        let (_dir, mut sh) = shell().await;
        let broken = sh.run("echo \"never closed", LONG).await.unwrap().unwrap();
        assert_ne!(broken.exit, 0);
        let after = sh.run("echo fine", LONG).await.unwrap().unwrap();
        assert_eq!(after.output.trim(), "fine");
    }

    #[tokio::test]
    async fn a_heredoc_and_a_trailing_backslash_are_just_text() {
        let (_dir, mut sh) = shell().await;
        let ran = sh.run("cat <<'EOF'\nline one\nEOF\necho tail \\", LONG).await.unwrap().unwrap();
        assert!(ran.output.contains("line one"), "{}", ran.output);
        let after = sh.run("echo still here", LONG).await.unwrap().unwrap();
        assert_eq!(after.output.trim(), "still here");
    }

    #[tokio::test]
    async fn a_command_past_its_timeout_takes_the_shell() {
        let (_dir, mut sh) = shell().await;
        let ran = sh.run("sleep 5", Duration::from_millis(200)).await.unwrap();
        assert!(ran.is_none());
    }

    #[tokio::test]
    async fn exiting_ends_the_shell() {
        let (_dir, mut sh) = shell().await;
        let ran = sh.run("echo bye; exit 3", LONG).await.unwrap().unwrap();
        assert!(!ran.alive);
        assert_eq!(ran.exit, 3);
        assert!(ran.output.starts_with("bye"), "{}", ran.output);
    }

    #[tokio::test]
    async fn long_output_keeps_its_ends() {
        let (_dir, mut sh) = shell().await;
        let ran = sh.run("echo FIRST; head -c 6000000 /dev/zero | tr '\\0' 'x'; echo; echo LAST", LONG).await.unwrap().unwrap();
        assert!(ran.output.len() < OUTPUT_MOST + 200, "{}", ran.output.len());
        assert!(ran.output.starts_with("FIRST"));
        assert!(ran.output.trim_end().ends_with("LAST"));
        assert!(ran.output.contains("bytes cut"));
    }

    #[tokio::test]
    async fn a_missing_directory_starts_nothing() {
        assert!(Shell::start(Path::new("/no/such/dir/for/hum")).await.is_err());
    }
}
