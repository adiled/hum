//! The spool: what the forager holds between a bundle arriving and its
//! answer leaving, on disk, so neither a restart of the forager nor one
//! of humd loses an ask a DTN node handed over.
//!
//! ```text
//! asks/<key>.json      an ask held: who asked, when it expires, which bloom it is on
//! asks/<key>.payload   its words, as they arrived
//! asks/<key>.answer    its answer, once grown and until it has gone home
//! pieces/<key>/        the pieces of an ask that arrived fragmented
//! heard/<key>          an ask answered or let go, kept until it expires
//! ```
//!
//! A key is the sha-256 of the ask's id. The `.json` is written last, so
//! an ask is held once it exists. An ask is heard once: while it is held
//! or in `heard/`, a copy of it, whole or in pieces, is a duplicate, and
//! once it has expired no copy can be taken anyway, so what is past its
//! expiry is forgotten. The spool holds at most `hold` asks and pieced
//! asks at a time; past that it takes nothing new.

use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dtn::Ask;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Held {
    pub id: String,
    pub source: String,
    pub expires: u64,
    pub lifetime_ms: u64,
    pub peer: SocketAddr,
    #[serde(default)]
    pub attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Grown {
    pub ok: bool,
    pub text: String,
}

#[derive(Debug, PartialEq)]
pub enum Taken {
    New(String),
    Partial,
    Duplicate,
    Full,
}

pub struct Spool {
    root: PathBuf,
    hold: usize,
    held: usize,
}

pub fn key(id: &str) -> String {
    Sha256::digest(id.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect()
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    hum_paths::layout::write_atomic_private(path, bytes).with_context(|| format!("write {}", path.display()))?;
    if let Some(dir) = path.parent() {
        fs::File::open(dir).and_then(|d| d.sync_all()).ok();
    }
    Ok(())
}

fn names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default()
}

fn expires_of(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

impl Spool {
    pub fn open(root: &Path, hold: usize) -> Result<Spool> {
        for sub in ["asks", "pieces", "heard"] {
            fs::create_dir_all(root.join(sub)).with_context(|| format!("create {}", root.join(sub).display()))?;
        }
        let mut spool = Spool { root: root.to_path_buf(), hold, held: 0 };
        spool.held = spool.waiting().len() + names(&spool.root.join("pieces")).len();
        Ok(spool)
    }

    fn ask_path(&self, key: &str, ext: &str) -> PathBuf {
        self.root.join("asks").join(format!("{key}.{ext}"))
    }

    fn heard_path(&self, key: &str) -> PathBuf {
        self.root.join("heard").join(key)
    }

    fn pieces_dir(&self, key: &str) -> PathBuf {
        self.root.join("pieces").join(key)
    }

    fn known(&self, key: &str) -> bool {
        self.heard_path(key).exists() || self.ask_path(key, "json").exists()
    }

    pub fn take(&mut self, ask: &Ask, payload: &[u8], peer: SocketAddr) -> Result<Taken> {
        let key = key(&ask.id);
        if self.known(&key) {
            return Ok(Taken::Duplicate);
        }
        if self.held >= self.hold {
            return Ok(Taken::Full);
        }
        self.hold_ask(&key, ask, payload, peer)?;
        self.held += 1;
        Ok(Taken::New(key))
    }

    fn hold_ask(&self, key: &str, ask: &Ask, payload: &[u8], peer: SocketAddr) -> Result<()> {
        let held = Held {
            id: ask.id.clone(),
            source: ask.source.clone(),
            expires: ask.expires,
            lifetime_ms: ask.lifetime_ms,
            peer,
            attempts: 0,
        };
        write(&self.ask_path(key, "payload"), payload)?;
        write(&self.ask_path(key, "json"), &serde_json::to_vec(&held)?)
    }

    pub fn piece(&mut self, ask: &Ask, offset: u64, total: u64, data: &[u8], peer: SocketAddr) -> Result<Taken> {
        let key = key(&ask.id);
        if self.known(&key) {
            return Ok(Taken::Duplicate);
        }
        let dir = self.pieces_dir(&key);
        if !dir.exists() {
            if self.held >= self.hold {
                return Ok(Taken::Full);
            }
            fs::create_dir_all(&dir)?;
            write(&dir.join("expires"), ask.expires.to_string().as_bytes())?;
            write(&dir.join("total"), total.to_string().as_bytes())?;
            self.held += 1;
        }
        if expires_of(&dir.join("total")) != Some(total) {
            return Ok(Taken::Partial);
        }
        let at = dir.join(format!("{offset}-{}", data.len()));
        if !at.exists() {
            write(&at, data)?;
        }
        let mut spans: BTreeMap<u64, u64> = BTreeMap::new();
        for name in names(&dir) {
            if let Some((off, len)) = name.split_once('-').and_then(|(o, l)| Some((o.parse::<u64>().ok()?, l.parse::<u64>().ok()?))) {
                let end = off + len;
                let e = spans.entry(off).or_insert(end);
                *e = (*e).max(end);
            }
        }
        let mut reach = 0u64;
        for (off, end) in &spans {
            if *off > reach {
                break;
            }
            reach = reach.max(*end);
        }
        if reach < total {
            return Ok(Taken::Partial);
        }
        let mut whole = vec![0u8; total as usize];
        for name in names(&dir) {
            if let Some(off) = name.split_once('-').and_then(|(o, _)| o.parse::<usize>().ok()) {
                let bytes = fs::read(dir.join(&name))?;
                whole[off..off + bytes.len()].copy_from_slice(&bytes);
            }
        }
        self.hold_ask(&key, ask, &whole, peer)?;
        fs::remove_dir_all(&dir).ok();
        Ok(Taken::New(key))
    }

    pub fn waiting(&self) -> Vec<String> {
        names(&self.root.join("asks"))
            .into_iter()
            .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
            .collect()
    }

    pub fn load(&self, key: &str) -> Result<(Held, Vec<u8>, Option<Grown>)> {
        let held: Held = serde_json::from_slice(&fs::read(self.ask_path(key, "json"))?)?;
        let payload = fs::read(self.ask_path(key, "payload"))?;
        let grown = fs::read(self.ask_path(key, "answer")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        Ok((held, payload, grown))
    }

    pub fn attempt(&self, key: &str) -> Result<u32> {
        let (mut held, _, _) = self.load(key)?;
        held.attempts += 1;
        write(&self.ask_path(key, "json"), &serde_json::to_vec(&held)?)?;
        Ok(held.attempts)
    }

    pub fn grown(&self, key: &str, grown: &Grown) -> Result<()> {
        write(&self.ask_path(key, "answer"), &serde_json::to_vec(grown)?)
    }

    pub fn heard(&mut self, key: &str, expires: u64) -> Result<()> {
        write(&self.heard_path(key), expires.to_string().as_bytes())?;
        self.release(key);
        Ok(())
    }

    fn release(&mut self, key: &str) {
        let had = self.ask_path(key, "json").exists();
        for ext in ["json", "payload", "answer"] {
            fs::remove_file(self.ask_path(key, ext)).ok();
        }
        if had {
            self.held = self.held.saturating_sub(1);
        }
    }

    pub fn forget_expired(&mut self, now_ms: u64) -> Vec<String> {
        let mut let_go = vec![];
        for key in names(&self.root.join("heard")) {
            if expires_of(&self.heard_path(&key)).is_none_or(|e| e <= now_ms) {
                fs::remove_file(self.heard_path(&key)).ok();
            }
        }
        for key in names(&self.root.join("pieces")) {
            let dir = self.pieces_dir(&key);
            if expires_of(&dir.join("expires")).is_none_or(|e| e <= now_ms) {
                fs::remove_dir_all(&dir).ok();
                self.held = self.held.saturating_sub(1);
            }
        }
        for key in self.waiting() {
            if self.load(&key).map(|(h, _, _)| h.expires <= now_ms).unwrap_or(true) {
                self.release(&key);
                let_go.push(key);
            }
        }
        let_go
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(id: &str, expires: u64) -> Ask {
        Ask { id: id.into(), source: "dtn://you/".into(), expires, lifetime_ms: 60_000 }
    }

    fn peer() -> SocketAddr {
        "127.0.0.1:4556".parse().unwrap()
    }

    #[test]
    fn an_ask_is_taken_once_until_it_expires() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Spool::open(dir.path(), 10).unwrap();
        let a = ask("a", 1_000);
        let Taken::New(k) = s.take(&a, b"q", peer()).unwrap() else { panic!() };
        assert_eq!(s.take(&a, b"q", peer()).unwrap(), Taken::Duplicate);
        s.heard(&k, a.expires).unwrap();
        assert_eq!(s.take(&a, b"q", peer()).unwrap(), Taken::Duplicate);
        assert!(s.forget_expired(999).is_empty());
        assert_eq!(s.take(&a, b"q", peer()).unwrap(), Taken::Duplicate);
        s.forget_expired(1_000);
        assert!(matches!(s.take(&a, b"q", peer()).unwrap(), Taken::New(_)));
    }

    #[test]
    fn pieces_make_a_whole_whatever_their_order_and_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Spool::open(dir.path(), 10).unwrap();
        let a = ask("p", u64::MAX);
        let whole = b"0123456789abcdef";
        assert_eq!(s.piece(&a, 10, 16, &whole[10..], peer()).unwrap(), Taken::Partial);
        assert_eq!(s.piece(&a, 0, 16, &whole[..6], peer()).unwrap(), Taken::Partial);
        assert_eq!(s.piece(&a, 0, 16, &whole[..6], peer()).unwrap(), Taken::Partial);
        assert_eq!(s.piece(&a, 0, 9_999, &[b'!'; 30], peer()).unwrap(), Taken::Partial, "a piece of another whole is ignored");
        let Taken::New(k) = s.piece(&a, 4, 16, &whole[4..12], peer()).unwrap() else { panic!() };
        let (_, payload, _) = s.load(&k).unwrap();
        assert_eq!(payload, whole);
        assert_eq!(s.piece(&a, 0, 16, &whole[..6], peer()).unwrap(), Taken::Duplicate);
    }

    #[test]
    fn a_full_spool_takes_nothing_new_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Spool::open(dir.path(), 2).unwrap();
        assert!(matches!(s.take(&ask("1", u64::MAX), b"q", peer()).unwrap(), Taken::New(_)));
        assert_eq!(s.piece(&ask("2", u64::MAX), 0, 9, b"abc", peer()).unwrap(), Taken::Partial);
        assert_eq!(s.take(&ask("3", u64::MAX), b"q", peer()).unwrap(), Taken::Full);
        let mut again = Spool::open(dir.path(), 2).unwrap();
        assert_eq!(again.waiting().len(), 1);
        assert_eq!(again.take(&ask("3", u64::MAX), b"q", peer()).unwrap(), Taken::Full);
    }
}
