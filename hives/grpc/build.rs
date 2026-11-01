//! Compile `proto/hum.proto` into Rust at build time: the server the
//! forager serves, and the client its tests stream through.

fn main() -> std::io::Result<()> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/hum.proto"], &["proto"])?;
    Ok(())
}
