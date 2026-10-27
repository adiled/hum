use std::path::PathBuf;

#[test]
fn an_empty_variable_is_unset() {
    let state = tempfile::TempDir::new().unwrap();
    unsafe {
        std::env::set_var("XDG_STATE_HOME", state.path());
        std::env::set_var("HUM_THRUM_SOCK", "");
        std::env::set_var("HUM_SOCKET", "");
    }
    let default = state.path().join("hum").join("thrum.sock");
    assert_eq!(hum_paths::thrum_sock(), default, "an empty HUM_THRUM_SOCK names no socket");

    unsafe { std::env::set_var("HUM_THRUM_SOCK", "/tmp/x.sock") };
    assert_eq!(hum_paths::thrum_sock(), PathBuf::from("/tmp/x.sock"));

    unsafe {
        std::env::remove_var("HUM_THRUM_SOCK");
        std::env::set_var("XDG_STATE_HOME", "");
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert_eq!(hum_paths::thrum_sock(), home.join(".local/state/hum/thrum.sock"), "an empty XDG_STATE_HOME is its default");
}
