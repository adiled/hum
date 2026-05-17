//! Compile `proto/hum.proto` into Rust at build time.

fn main() -> std::io::Result<()> {
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .compile_protos(&["proto/hum.proto"], &["proto"])?;
    Ok(())
}
