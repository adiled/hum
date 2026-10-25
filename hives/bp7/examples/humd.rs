//! A humd for the tests: the daemon as the `humd` binary runs it, under
//! whatever roots its environment names, until it is told to stop. The
//! ensemble test runs one of these as the humd a DTN node's ask lands on,
//! beside the humd in the test's own process.

use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    hum_paths::init();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("HUM_LOG_LEVEL")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cfg = humd::DaemonConfig::from_env();
    humd::run(cfg, async {
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    })
    .await
}
