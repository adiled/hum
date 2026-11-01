//! The guard keeps the hum roots to the layout: strays move to their
//! root's `tmp/`, a fresh config stray waits until it is still, a week in
//! `tmp/` moves on to the archive, and a crash's staged write goes.

use std::path::Path;
use std::time::{Duration, SystemTime};

use hum_paths::layout::{self, Root};
use humd::guard::{AGE, Guard, STALE_STAGING};

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"x").unwrap();
}

fn back_date(path: &Path, by: Duration) {
    let then = SystemTime::now() - by;
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(then)
        .unwrap();
}

#[test]
fn the_guard_keeps_the_roots_to_the_layout() {
    let tmp = tempfile::TempDir::new().unwrap();
    unsafe {
        std::env::set_var("HOME", tmp.path().join("home"));
        std::env::set_var("XDG_STATE_HOME", tmp.path().join("state"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CACHE_HOME", tmp.path().join("cache"));
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path().join("run"));
    }
    let id = format!("bee_{}", "1a".repeat(32));
    let state = hum_paths::state_dir();
    let config = hum_paths::config_dir();

    touch(&hum_paths::humd_key());
    touch(&hum_paths::bee_key(&id));
    touch(&hum_paths::hive_state_dir("dwarfstar").join("gpu.lock"));
    touch(&hum_paths::hum_json());
    touch(&state.join("openai-server").join("audit.log"));
    touch(&hum_paths::bees_dir().join("dwarfstar.key"));
    touch(&config.join(".hum.json.swp"));
    let old_config = config.join("Orchfile");
    touch(&old_config);
    back_date(&old_config, Duration::from_secs(3600));

    let mut guard = Guard::default();
    guard.sweep();

    assert!(hum_paths::humd_key().is_file());
    assert!(hum_paths::bee_key(&id).is_file());
    assert!(hum_paths::hive_state_dir("dwarfstar").join("gpu.lock").is_file());
    assert!(hum_paths::hum_json().is_file());

    assert!(!state.join("openai-server").exists());
    assert!(Root::State.tmp().join("openai-server").join("audit.log").is_file());
    assert!(!hum_paths::bees_dir().join("dwarfstar.key").exists());
    assert!(Root::State.tmp().join("bees").join("dwarfstar.key").is_file());
    assert!(Root::Config.tmp().join("Orchfile").is_file(), "a still config stray moves");
    assert!(config.join(".hum.json.swp").is_file(), "a fresh config stray waits");
    assert!(layout::strays(Root::State).is_empty());

    touch(&state.join("openai-server").join("audit.log"));
    guard.sweep();
    let again: Vec<_> = std::fs::read_dir(Root::State.tmp())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("openai-server"))
        .collect();
    assert_eq!(again.len(), 2, "a second stray of one name keeps both: {again:?}");

    let staged = Root::State.tmp().join(format!("{}123-0-0", layout::STAGING_PREFIX));
    touch(&staged);
    let later = SystemTime::now() + STALE_STAGING + Duration::from_secs(60);
    guard.sweep_at(later);
    assert!(!staged.exists(), "a crash's staged write goes");
    assert!(Root::State.tmp().join("bees").exists(), "an hour is not a week");

    let week_on = SystemTime::now() + AGE + Duration::from_secs(60);
    guard.sweep_at(week_on);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let archived = layout::archive_dir().join(&today).join("state");
    assert!(archived.join("openai-server").join("audit.log").is_file());
    assert!(archived.join("bees").join("dwarfstar.key").is_file());
    assert!(layout::archive_dir().join(&today).join("config").join("Orchfile").is_file());
    assert!(!Root::State.tmp().join("openai-server").exists());
}
