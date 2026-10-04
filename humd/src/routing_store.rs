use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ensemble::{Ensemble, HumdAddr};
use tracing::{info, trace, warn};

#[derive(serde::Serialize, serde::Deserialize)]
struct OnDisk {
    version: u32,
    addrs: Vec<HumdAddr>,
}

pub fn load(path: &Path) -> Vec<HumdAddr> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            trace!(target: "routing", "routing.load.absent");
            return Vec::new();
        }
        Err(e) => {
            warn!(target: "routing", "routing.load.read_failed err={e}");
            return Vec::new();
        }
    };
    match serde_json::from_slice::<OnDisk>(&bytes) {
        Ok(disk) => disk.addrs,
        Err(e) => {
            warn!(target: "routing", "routing.load.parse_failed err={e}");
            Vec::new()
        }
    }
}

pub fn save(ensemble: &Ensemble, path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let disk = OnDisk {
        version: 1,
        addrs: ensemble.routing_snapshot(),
    };
    let bytes =
        serde_json::to_vec(&disk).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn restore_on_boot(ensemble: &Ensemble, path: &Path) {
    let addrs = load(path);
    if addrs.is_empty() {
        return;
    }
    let restored = ensemble.routing_restore(addrs);
    info!(target: "routing", peers = restored, "routing.restored");
}

pub fn spawn_persister(ensemble: Arc<Ensemble>, path: PathBuf, interval: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            tick.tick().await;
            if let Err(e) = save(&ensemble, &path) {
                warn!(target: "routing", "routing.persist.failed err={e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> HumdAddr {
        HumdAddr::new(ensemble::Hid::random_humd()).with_hint("192.0.2.7:7777")
    }

    fn ens() -> Arc<Ensemble> {
        Arc::new(Ensemble::new(ensemble::Hid::random_humd()))
    }

    #[test]
    fn a_missing_file_restores_nothing() {
        let dir = std::env::temp_dir().join(format!(
            "routing-absent-{}",
            ensemble::Hid::random_humd().short()
        ));
        assert!(load(&dir.join("routing.json")).is_empty());
    }

    #[test]
    fn a_saved_table_survives_a_restart() {
        let path = std::env::temp_dir()
            .join(format!("routing-{}", ensemble::Hid::random_humd().short()))
            .join("routing.json");

        let before = ens();
        let peer = addr();
        assert_eq!(before.routing_restore(vec![peer.clone()]), 1);
        save(&before, &path).expect("save must succeed");

        let after = ens();
        restore_on_boot(&after, &path);
        assert_eq!(
            after.kad_routing_table_len(),
            1,
            "the table must survive a restart"
        );
        assert!(
            after
                .kad_closest(&peer.id, 4)
                .iter()
                .any(|a| a.id == peer.id)
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_corrupt_file_restores_nothing_rather_than_panicking() {
        let path = std::env::temp_dir()
            .join(format!(
                "routing-bad-{}",
                ensemble::Hid::random_humd().short()
            ))
            .join("routing.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        assert!(load(&path).is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
