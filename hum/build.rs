fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let stamped = match describe() {
        Some(d) if d.starts_with(&format!("v{version}")) => d,
        Some(d) => format!("v{version}@{d}"),
        None => format!("v{version}"),
    };
    println!("cargo:rustc-env=HUM_VERSION={stamped}");
}

fn describe() -> Option<String> {
    std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
