//! humnest — bee supervisor.

pub mod supervisor;
pub mod control;
pub mod log_capture;

pub use supervisor::{Supervisor, BeeStatus};

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

pub async fn run<F>(shutdown: F) -> Result<()>
where F: std::future::Future<Output = ()> + Send,
{
    hum_paths::init();
    let cfg = config::load();
    let bees = cfg.humnest.bees.clone();
    info!(count = bees.len(), "humnest.boot");

    let supervisor = Arc::new(Supervisor::new());
    supervisor.clone().spawn_all(bees).await;

    let socket = hum_paths::humnest_sock();
    let _ctl = control::serve(socket, supervisor.clone()).await?;

    shutdown.await;
    info!("humnest.shutdown");
    supervisor.kill_all().await;
    hum_paths::HumnestRuntimeInfo::remove();
    Ok(())
}
