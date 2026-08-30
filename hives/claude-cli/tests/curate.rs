//! `WorkerBee::curate` — the compute side of `chi:"curate"`.
//!
//! Curation works on the transcript on disk, not on a live cell: `claude -p`
//! exits after every turn, so a curate almost always arrives with nothing
//! running. These tests spawn no process.
//!
//! `HOME` is repointed at a tempdir because `session_path` resolves
//! `~/.claude/projects/...`.

use std::fs;
use std::sync::{Mutex, MutexGuard, OnceLock};

use claude_cli::graft::session_path;
use claude_cli::ClaudeCliWorker;
use nest::WorkerBee;
use tempfile::TempDir;

// HOME is process-global and cargo runs these tests on parallel threads.
// Every test that repoints it holds this guard for the duration of its
// filesystem work, matching graft_integration.rs.
fn home_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn sandbox() -> (TempDir, MutexGuard<'static, ()>) {
    let guard = home_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var("HOME", dir.path());
    (dir, guard)
}

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Place a fixture transcript where `curate` will look for this sid's.
fn stage_transcript(home: &TempDir, cwd: &str, sid: &ids::HumId, name: &str) -> std::path::PathBuf {
    let derived = sid.to_uuid_v5(ids::NS_CLAUDE_SESSION).to_string();
    let path = session_path(std::path::Path::new(cwd), &derived);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::copy(fixture(name), &path).expect("copy fixture");
    let _ = home;
    path
}

#[tokio::test]
async fn curate_prunes_the_sid_transcript_in_place() {
    let (home, _guard) = sandbox();

    let cwd = "/tmp/proj";
    let sid = ids::HumId::from_foreign("ses_curate_one");
    let path = stage_transcript(&home, cwd, &sid, "with_thinking.jsonl");
    let before = fs::metadata(&path).expect("stat").len();

    let report = ClaudeCliWorker.curate(&sid, cwd).await.expect("curate");

    // Byte counts are of the re-serialized entries, not the raw file, so
    // they track the transcript's size without matching it exactly.
    assert!(
        report.bytes_before > 0,
        "curate must find and measure the transcript for this sid"
    );

    // The fixture carries only four user turns and the default protection
    // window keeps four, so nothing is eligible. Proves curate reached the
    // right file and honoured the protection invariant rather than
    // silently missing the path.
    assert_eq!(report.trimmed(), 0);
    assert_eq!(
        fs::metadata(&path).expect("stat").len(),
        before,
        "a fully protected transcript must survive intact"
    );
}

#[tokio::test]
async fn curate_is_a_no_op_when_no_transcript_exists() {
    let (_home, _guard) = sandbox();

    let sid = ids::HumId::from_foreign("ses_never_prompted");
    let report = ClaudeCliWorker
        .curate(&sid, "/tmp/nonexistent")
        .await
        .expect("curate must not error on a missing transcript");

    // A curate can arrive for a sid this bee has never raised a cell for —
    // humd sprays to every worker. That is not a failure.
    assert_eq!(report.bytes_before, 0);
    assert_eq!(report.bytes_after, 0);
    assert_eq!(report.trimmed(), 0);
}
