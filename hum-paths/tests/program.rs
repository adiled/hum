#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn write(dir: &std::path::Path, name: &str, mode: u32) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    p
}

#[test]
fn a_program_is_found_off_a_bare_path() {
    let home = tempfile::TempDir::new().unwrap();
    let on_path = tempfile::TempDir::new().unwrap();
    let local_bin = home.path().join(".local/bin");
    std::fs::create_dir_all(&local_bin).unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("PATH", on_path.path());
    }

    let dirs = hum_paths::program_dirs();
    assert_eq!(dirs[0], on_path.path(), "PATH comes first");
    assert!(dirs.contains(&local_bin), "~/.local/bin is looked in though PATH leaves it out");

    assert_eq!(hum_paths::find_program("hum-test-prog"), None);
    write(&local_bin, "hum-test-prog", 0o644);
    assert_eq!(hum_paths::find_program("hum-test-prog"), None, "a file that cannot run is not a program");
    let found = write(&local_bin, "hum-test-prog", 0o755);
    assert_eq!(hum_paths::find_program("hum-test-prog"), Some(found));
    let first = write(on_path.path(), "hum-test-prog", 0o755);
    assert_eq!(hum_paths::find_program("hum-test-prog"), Some(first), "PATH wins");
}
