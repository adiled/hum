use anyhow::Result;
use tokio::signal::unix::{signal, SignalKind};
use tracing::trace;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_env("HUM_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).compact().init();
    humnest::run(wait_for_shutdown()).await
}

async fn wait_for_shutdown() {
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM");
    let mut int  = signal(SignalKind::interrupt()).expect("SIGINT");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => trace!("humnest.shutdown.ctrl-c"),
        _ = term.recv() => trace!("humnest.shutdown.sigterm"),
        _ = int.recv() => trace!("humnest.shutdown.sigint"),
    }
}
