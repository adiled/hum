//! Advisories: what humd tells a bee it should fix, without refusing it.
//!
//! humd refuses what it must (an unsigned hello, a bee its hive did not
//! enter, a revoked key, an incompatible protocol) and advises on what it
//! can live with. An advisory is noted against the bee's id, sent to the
//! bee as a `log` tone whose `fields.advisory` is its code, and kept in
//! `advisories.json` in the state root, which `hum doctor` and `hum bee
//! --list` read. hum's own client turns the log tone into an advisory
//! event and prints it into the hive's log.
//!
//! What humd advises on:
//!
//! - `undeclared-key`: a tone from the bee carries keys the thrum does
//!   not declare for its chi.
//! - `proto-drift`: the bee speaks another minor version of the thrum.
//! - `reconnect-storm`: the bee said hello [`STORM_HELLOS`] times within
//!   [`STORM_WINDOW`].
//! - `did-not-return`: the bee was nestled when humd last stopped and has
//!   not said hello within [`RETURN_GRACE`] of humd starting again. A bee
//!   must outlive humd (THRUM.md, The client), and this is the
//!   one rule humd sees broken only by an absence. The bee hears it when
//!   it does return.
//!
//! An advisory is noted once per bee, code and detail in a humd run, and
//! counted across runs. An entry nobody renews for [`KEPT`] is dropped, so
//! the book holds what is still true.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thrum_core::AdvisoryCode;
use tracing::{trace, warn};

pub const STORM_HELLOS: usize = 10;
pub const STORM_WINDOW: Duration = Duration::from_secs(600);
pub const RETURN_GRACE: Duration = Duration::from_secs(120);
pub const KEPT: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    pub first: i64,
    pub last: i64,
    pub count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BeeBook {
    pub hive: String,
    pub advisories: Vec<Entry>,
}

pub type Book = BTreeMap<String, BeeBook>;

pub fn read(path: &Path) -> Book {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn tone(entry: &Entry) -> Value {
    let mut fields = entry.fields.clone();
    fields.insert("advisory".into(), json!(entry.code));
    json!({
        "chi": "log",
        "level": "warn",
        "message": entry.message,
        "fields": fields,
    })
}

#[derive(Default)]
pub struct Advisories {
    path: Option<PathBuf>,
    book: Mutex<Book>,
    noted: Mutex<HashSet<String>>,
    hellos: Mutex<HashMap<String, VecDeque<i64>>>,
    expected: Mutex<HashMap<String, String>>,
    returned: Mutex<HashSet<String>>,
    unreturned: Mutex<HashMap<String, Entry>>,
}

impl Advisories {
    pub fn load(path: PathBuf, now_ms: i64) -> Self {
        let mut book = read(&path);
        let horizon = now_ms - KEPT.as_millis() as i64;
        for bee in book.values_mut() {
            bee.advisories.retain(|e| e.last >= horizon);
        }
        book.retain(|_, bee| !bee.advisories.is_empty());
        Self {
            path: Some(path),
            book: Mutex::new(book),
            ..Self::default()
        }
    }

    pub fn book(&self) -> Book {
        self.book.lock().clone()
    }

    pub fn note(
        &self,
        bee: &str,
        hive: &str,
        code: AdvisoryCode,
        message: String,
        fields: Map<String, Value>,
        now_ms: i64,
    ) -> Option<Entry> {
        let key = format!("{bee}:{}:{}", code.as_str(), Value::Object(fields.clone()));
        if !self.noted.lock().insert(key) {
            return None;
        }
        let entry = {
            let mut book = self.book.lock();
            let page = book.entry(bee.to_string()).or_default();
            page.hive = hive.to_string();
            match page
                .advisories
                .iter_mut()
                .find(|e| e.code == code.as_str() && e.fields == fields)
            {
                Some(e) => {
                    e.message = message;
                    e.last = now_ms;
                    e.count += 1;
                    e.clone()
                }
                None => {
                    let e = Entry {
                        code: code.as_str().to_string(),
                        message,
                        fields,
                        first: now_ms,
                        last: now_ms,
                        count: 1,
                    };
                    page.advisories.push(e.clone());
                    e
                }
            }
        };
        warn!(bee = %short(bee), hive, code = %entry.code, "bee.advisory — {}", entry.message);
        self.persist();
        Some(entry)
    }

    pub fn expect(&self, bees: HashMap<String, String>) {
        *self.expected.lock() = bees;
    }

    pub fn hello(&self, bee: &str, hive: &str, now_ms: i64) -> Vec<Entry> {
        let mut out = Vec::new();
        self.returned.lock().insert(bee.to_string());
        if let Some(entry) = self.unreturned.lock().remove(bee) {
            out.push(entry);
        }
        let storm = {
            let mut hellos = self.hellos.lock();
            let seen = hellos.entry(bee.to_string()).or_default();
            seen.push_back(now_ms);
            let since = now_ms - STORM_WINDOW.as_millis() as i64;
            while seen.front().is_some_and(|t| *t < since) {
                seen.pop_front();
            }
            seen.len() >= STORM_HELLOS
        };
        if storm {
            let mut fields = Map::new();
            fields.insert("hellos".into(), json!(STORM_HELLOS));
            fields.insert("withinSecs".into(), json!(STORM_WINDOW.as_secs()));
            let message = format!(
                "said hello {STORM_HELLOS} times in {} minutes; a bee that keeps reconnecting is \
                 being dropped by something, or is not waiting between dials",
                STORM_WINDOW.as_secs() / 60
            );
            if let Some(entry) = self.note(bee, hive, AdvisoryCode::ReconnectStorm, message, fields, now_ms) {
                out.push(entry);
            }
        }
        out
    }

    pub fn unreturned(&self, now_ms: i64) -> Vec<String> {
        let expected = std::mem::take(&mut *self.expected.lock());
        let returned = self.returned.lock().clone();
        let mut missing = Vec::new();
        for (bee, hive) in expected {
            if returned.contains(&bee) {
                continue;
            }
            let message = format!(
                "was nestled when humd stopped and had not come back {}s after humd started; \
                 a bee must reconnect when humd restarts, or exit non-zero so its supervisor \
                 starts it again (stopped on purpose? then ignore this)",
                RETURN_GRACE.as_secs()
            );
            if let Some(entry) = self.note(&bee, &hive, AdvisoryCode::DidNotReturn, message, Map::new(), now_ms) {
                self.unreturned.lock().insert(bee.clone(), entry);
                missing.push(bee);
            }
        }
        missing
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        let bytes = serde_json::to_vec_pretty(&*self.book.lock()).unwrap_or_default();
        if let Err(e) = hum_paths::write_atomic(path, &bytes) {
            trace!(path = %path.display(), err = %e, "advisories.write.failed");
        }
    }
}

fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_793_000_000_000;

    #[test]
    fn an_advisory_is_noted_once_a_run_and_counted_across_runs() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("advisories.json");
        let ads = Advisories::load(path.clone(), T);
        let mut fields = Map::new();
        fields.insert("keys".into(), json!(["text"]));
        assert!(ads.note("bee_a", "dwarfstar", AdvisoryCode::UndeclaredKey, "m".into(), fields.clone(), T).is_some());
        assert!(ads.note("bee_a", "dwarfstar", AdvisoryCode::UndeclaredKey, "m".into(), fields.clone(), T).is_none());

        let again = Advisories::load(path.clone(), T + 1);
        let entry = again.note("bee_a", "dwarfstar", AdvisoryCode::UndeclaredKey, "m".into(), fields, T + 1).unwrap();
        assert_eq!((entry.count, entry.first, entry.last), (2, T, T + 1));
        assert_eq!(read(&path)["bee_a"].advisories.len(), 1);

        let later = Advisories::load(path.clone(), T + 1 + KEPT.as_millis() as i64 + 1);
        assert!(later.book().is_empty(), "an entry nobody renewed goes");
    }

    #[test]
    fn a_bee_that_keeps_saying_hello_is_a_storm() {
        let ads = Advisories::default();
        for i in 0..STORM_HELLOS - 1 {
            assert!(ads.hello("bee_a", "h", T + i as i64).is_empty());
        }
        let out = ads.hello("bee_a", "h", T + 100);
        assert_eq!(out[0].code, "reconnect-storm");
        assert!(ads.hello("bee_b", "h", T + STORM_WINDOW.as_millis() as i64).is_empty());
    }

    #[test]
    fn a_bee_that_does_not_come_back_is_advised_when_it_does() {
        let ads = Advisories::default();
        ads.expect(HashMap::from([
            ("bee_back".to_string(), "h".to_string()),
            ("bee_gone".to_string(), "h".to_string()),
        ]));
        ads.hello("bee_back", "h", T);
        assert_eq!(ads.unreturned(T + 1), vec!["bee_gone".to_string()]);
        assert!(ads.unreturned(T + 2).is_empty(), "noted once");
        let out = ads.hello("bee_gone", "h", T + 3);
        assert_eq!(out[0].code, "did-not-return");
        assert!(ads.hello("bee_gone", "h", T + 4).is_empty());
    }

    #[test]
    fn the_tone_carries_the_code_in_its_fields() {
        let entry = Entry {
            code: "undeclared-key".into(),
            message: "unfurl carries delta".into(),
            fields: serde_json::from_value(json!({"keys": ["delta"]})).unwrap(),
            first: T,
            last: T,
            count: 1,
        };
        let t = tone(&entry);
        assert_eq!(t["fields"]["advisory"], "undeclared-key");
        assert!(thrum_core::undeclared(&t).is_none());
    }
}
