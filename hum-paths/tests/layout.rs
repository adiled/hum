//! Every path hum-paths hands out lands inside the layout, and anything
//! else under a root is a stray.

use std::path::Path;

use hum_paths::layout::{self, Root};

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"x").unwrap();
}

#[test]
fn the_paths_hum_hands_out_are_the_layout() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_STATE_HOME", tmp.path().join("state"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CACHE_HOME", tmp.path().join("cache"));
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path().join("run"));
    }
    let id = format!("bee_{}", "0f".repeat(32));

    for file in [
        hum_paths::humd_key(),
        hum_paths::thrum_sock(),
        hum_paths::runtime_info(),
        hum_paths::routing_json(),
        hum_paths::bees_snapshot(),
        hum_paths::revoked_bees(),
        hum_paths::bee_key(&id),
        hum_paths::bee_entry(&id),
        hum_paths::thehum_seq_file(&hum_paths::thehum_dir()),
        hum_paths::hum_json(),
        hum_paths::peers_json(),
        hum_paths::hive_installation("dwarfstar"),
        hum_paths::hive_config("dwarfstar"),
        hum_paths::hive_state_dir("dwarfstar").join("orch").join("anything"),
        hum_paths::src_dir().join("Cargo.toml"),
        hum_paths::foreign_hive_cache("org", "repo", "main").join("Orchfile"),
        hum_paths::comb(),
        hum_paths::http_sock(),
    ] {
        touch(&file);
    }
    for root in Root::ALL {
        touch(&root.tmp().join("anything"));
        assert!(layout::strays(root).is_empty(), "{} has strays: {:?}", root.name(), layout::strays(root));
    }

    touch(&hum_paths::state_dir().join("openai-server").join("audit.log"));
    touch(&hum_paths::bees_dir().join("dwarfstar.key"));
    touch(&hum_paths::config_dir().join("Orchfile"));
    assert_eq!(
        layout::strays(Root::State),
        vec![hum_paths::bees_dir().join("dwarfstar.key"), hum_paths::state_dir().join("openai-server")]
    );
    assert_eq!(layout::strays(Root::Config), vec![hum_paths::config_dir().join("Orchfile")]);

    let written = hum_paths::state_dir().join("routing.json");
    hum_paths::write_atomic(&written, b"{}").unwrap();
    assert_eq!(std::fs::read(&written).unwrap(), b"{}");
    hum_paths::write_atomic_private(&hum_paths::bee_key(&id), b"seed").unwrap();
    let staged: Vec<_> = std::fs::read_dir(Root::State.tmp())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(layout::STAGING_PREFIX))
        .collect();
    assert!(staged.is_empty(), "a staged write was left behind");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(hum_paths::bee_key(&id)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(Root::of(&hum_paths::bee_key(&id)), Some(Root::State));
    assert_eq!(Root::of(Path::new("/nowhere/x")), None);
}
