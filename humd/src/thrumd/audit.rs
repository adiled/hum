//! Holding the wire to the thrum's spec.
//!
//! Every tone humd hears or sends is checked against `thrum_core::spec`.
//! In the field a stray key is logged once, since a bee can be older or
//! newer than this humd. Under `Strict`, as in the simulator, every stray
//! is kept so a test fails on it: a key the kernel sends, or reads from
//! what it is sent, without the thrum declaring it is a bug, not a dialect.

use std::collections::HashSet;

use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use tracing::{error, warn};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WireCheck {
    #[default]
    Warn,
    Strict,
}

#[derive(Default)]
pub struct Audit {
    mode: RwLock<WireCheck>,
    warned: Mutex<HashSet<String>>,
    strays: Mutex<Vec<String>>,
}

impl Audit {
    pub fn set(&self, mode: WireCheck) {
        *self.mode.write() = mode;
    }

    pub fn heard(&self, client_id: &str, tone: &Value) {
        self.check("heard from", client_id, tone);
    }

    pub fn sent(&self, client_id: &str, tone: &Value) {
        self.check("sent to", client_id, tone);
    }

    pub fn strays(&self) -> Vec<String> {
        self.strays.lock().clone()
    }

    fn check(&self, way: &str, client_id: &str, tone: &Value) {
        let Some((chi, stray)) = thrum_core::undeclared(tone) else {
            return;
        };
        let chi = serde_json::to_value(chi)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        match *self.mode.read() {
            WireCheck::Warn => {
                let fresh: Vec<&String> = {
                    let mut warned = self.warned.lock();
                    stray
                        .iter()
                        .filter(|k| warned.insert(format!("{way}:{chi}:{k}")))
                        .collect()
                };
                if !fresh.is_empty() {
                    warn!(%chi, keys = ?fresh, way, client_id,
                        "thrum.tone.undeclared — a key the thrum does not declare for this chi");
                }
            }
            WireCheck::Strict => {
                error!(%chi, keys = ?stray, way, client_id, "thrum.tone.undeclared");
                self.strays
                    .lock()
                    .push(format!("{way} {client_id}: chi:{chi} carries {stray:?}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_keeps_every_stray_and_warn_keeps_none() {
        let audit = Audit::default();
        let stray = json!({"chi": "cancel", "rid": "r", "nope": 1});
        audit.heard("c", &stray);
        assert!(audit.strays().is_empty());
        audit.set(WireCheck::Strict);
        audit.heard("c", &stray);
        audit.sent("c", &json!({"chi": "cancel", "rid": "r"}));
        assert_eq!(audit.strays().len(), 1);
        assert!(audit.strays()[0].contains("nope"));
    }
}
