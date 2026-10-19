fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let stamped = match hash {
        Some(h) => format!("v{version}@{h}"),
        None => format!("v{version}"),
    };
    println!("cargo:rustc-env=HUM_VERSION={stamped}");
}
