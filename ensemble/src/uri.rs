//! `hum://` URI parsing.
//!
//! The wire-level address scheme for anything addressable across
//! the mesh — cwd, file paths in tool args, attach targets, etc.
//! The URI host segment is, in resolution order:
//!
//! 1. **alias** — a name registered in the asker humd's local
//!    resolver (today: `peers.json`; future: ENS / HNS / DID
//!    adapters slotting into the same trait).
//! 2. **shortid** — `<prefix>_<12hex>` form of a [`Hid`].
//! 3. **full hid** — `<prefix>_<64hex>` form.
//!
//! Schemes other than `hum://` are rejected. Path component is
//! arbitrary opaque text (left to the resolver / forager to
//! interpret — humfs treats it as a local-fs absolute path).
//!
//! Canonicalization: the asker humd resolves aliases to full
//! [`Hid`]s before forwarding any tone downstream, so peer humds
//! never need to consult the asker's local registry. The URI shape
//! stays the same; only the host segment narrows to a hid.
//!
//! ```text
//! hum://workstation/Users/op/auth        — alias
//! hum://humd_a4f2b8c19d3e/Users/op/auth  — shortid
//! hum://humd_a4f2b8c19d3e…64hex/path     — full hid
//! ```
//!
//! The host segment is classified by shape, not by asking: a known role
//! prefix followed by `_` and hex means a hid, everything else is an
//! alias. Aliases may therefore contain underscores.

use std::fmt;

use crate::{Hid, HidParseError, HidPrefix};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumUri {
    pub host: HostRef,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRef {
    Alias(String),
    Hid(Hid),
}

#[derive(Debug)]
pub enum UriParseError {
    WrongScheme,
    EmptyHost,
    BadHid(HidParseError),
}

impl fmt::Display for UriParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UriParseError::WrongScheme => write!(f, "URI must start with `hum://`"),
            UriParseError::EmptyHost => write!(f, "URI has empty host"),
            UriParseError::BadHid(e) => write!(f, "host parses as hid but is malformed: {e}"),
        }
    }
}

impl std::error::Error for UriParseError {}

impl HumUri {
    pub fn parse(s: &str) -> Result<Self, UriParseError> {
        let rest = s.strip_prefix("hum://").ok_or(UriParseError::WrongScheme)?;
        let (host_str, path_str) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };
        if host_str.is_empty() { return Err(UriParseError::EmptyHost); }

        let host = if looks_like_hid(host_str) {
            Hid::from_hex(host_str)
                .map(HostRef::Hid)
                .map_err(UriParseError::BadHid)?
        } else {
            HostRef::Alias(host_str.to_string())
        };
        Ok(HumUri { host, path: path_str.to_string() })
    }

    pub fn starts_with_scheme(s: &str) -> bool {
        s.starts_with("hum://")
    }

    pub fn to_string_canonical(&self) -> String {
        let host = match &self.host {
            HostRef::Alias(a) => a.clone(),
            HostRef::Hid(h) => h.to_hex(),
        };
        format!("hum://{host}/{}", self.path)
    }

    pub fn with_hid(&self, hid: Hid) -> Self {
        HumUri { host: HostRef::Hid(hid), path: self.path.clone() }
    }
}

fn looks_like_hid(host: &str) -> bool {
    let Some((p, rest)) = host.split_once('_') else { return false };
    if HidPrefix::parse(p).is_none() { return false; }
    rest.chars().all(|c| c.is_ascii_hexdigit())
}

pub trait AliasResolver: Send + Sync {
    fn resolve(&self, alias: &str) -> Option<Hid>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alias_host() {
        let u = HumUri::parse("hum://workstation/Users/op/auth").unwrap();
        assert_eq!(u.host, HostRef::Alias("workstation".into()));
        assert_eq!(u.path, "Users/op/auth");
    }

    #[test]
    fn parses_full_hid_host() {
        let id = Hid::random_humd();
        let s = format!("hum://{}/path/to/x", id.to_hex());
        let u = HumUri::parse(&s).unwrap();
        assert_eq!(u.host, HostRef::Hid(id));
        assert_eq!(u.path, "path/to/x");
    }

    #[test]
    fn parses_short_hid_host() {
        let id = Hid::random_humd();
        let short = id.short();
        let s = format!("hum://{}/", short);
        let u = HumUri::parse(&s).unwrap();
        match u.host {
            HostRef::Hid(h) => assert_eq!(h.prefix, HidPrefix::Humd),
            other => panic!("expected hid, got {:?}", other),
        }
        assert_eq!(u.path, "");
    }

    #[test]
    fn alias_with_underscore_not_treated_as_hid() {
        let u = HumUri::parse("hum://my_workstation/Users/op").unwrap();
        assert_eq!(u.host, HostRef::Alias("my_workstation".into()));
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(matches!(HumUri::parse("https://x/y"), Err(UriParseError::WrongScheme)));
        assert!(matches!(HumUri::parse("/Users/op/auth"), Err(UriParseError::WrongScheme)));
    }

    #[test]
    fn rejects_empty_host() {
        assert!(matches!(HumUri::parse("hum:///path"), Err(UriParseError::EmptyHost)));
    }

    #[test]
    fn round_trip_canonical() {
        let s = "hum://workstation/Users/op/auth";
        let u = HumUri::parse(s).unwrap();
        assert_eq!(u.to_string_canonical(), s);
    }

    #[test]
    fn with_hid_replaces_host() {
        let alias = HumUri::parse("hum://workstation/x").unwrap();
        let h = Hid::random_humd();
        let resolved = alias.with_hid(h);
        match resolved.host {
            HostRef::Hid(rh) => assert_eq!(rh, h),
            other => panic!("expected hid host, got {:?}", other),
        }
        assert_eq!(resolved.path, "x");
    }

    #[test]
    fn starts_with_scheme_helper() {
        assert!(HumUri::starts_with_scheme("hum://x/y"));
        assert!(!HumUri::starts_with_scheme("/tmp/x"));
        assert!(!HumUri::starts_with_scheme("https://x"));
    }
}
