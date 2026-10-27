#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

#[test]
fn claude_is_found_where_its_installer_puts_it() {
    let home = tempfile::TempDir::new().unwrap();
    let bare = tempfile::TempDir::new().unwrap();
    let local_bin = home.path().join(".local/bin");
    std::fs::create_dir_all(&local_bin).unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("PATH", bare.path());
        std::env::set_var("CLAUDE_CLI_PATH", "");
    }

    let missing = claude_cli::claude_program().unwrap_err().to_string();
    assert!(missing.contains(&local_bin.display().to_string()), "{missing}");
    assert!(missing.contains("CLAUDE_CLI_PATH"), "{missing}");

    let claude = local_bin.join("claude");
    std::fs::write(&claude, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(claude_cli::claude_program().unwrap(), claude);

    unsafe { std::env::set_var("CLAUDE_CLI_PATH", "/elsewhere/claude") };
    assert_eq!(claude_cli::claude_program().unwrap(), std::path::PathBuf::from("/elsewhere/claude"));
}
