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

use std::fmt;

use crate::{Hid, HidParseError};

/// A parsed `hum://` URI. Cheap to clone (`HostRef::Alias` is the
/// only heap allocation, and that's just the alias string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumUri {
    pub host: HostRef,
    /// Path component, always leading-slash-stripped. Empty string
    /// when the URI was `hum://<host>/` with no body. Forager-side
    /// interpretation: humfs treats this as a local absolute path
    /// (prepends `/`); other foragers may interpret differently.
    pub path: String,
}

/// What the URI's host segment names. The asker humd canonicalizes
/// [`HostRef::Alias`] to [`HostRef::Hid`] before any cross-humd
/// forward — peer humds see only the resolved form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRef {
    /// Alias to be resolved by [`AliasResolver`]. Free-form string
    /// matching `[A-Za-z0-9._-]+` — anything that doesn't look like
    /// a hid (no `_` after the first 4 chars in the role prefix
    /// pattern) lands here.
    Alias(String),
    /// Resolved (or directly-specified) hid. Short or full form;
    /// short widens with zeros on parse but routing always uses
    /// the canonical hex repr.
    Hid(Hid),
}

#[derive(Debug)]
pub enum UriParseError {
    /// Input doesn't start with `hum://`.
    WrongScheme,
    /// No host segment between `hum://` and the next `/`.
    EmptyHost,
    /// Looks like a hid (has `<prefix>_`) but doesn't parse.
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
    /// Parse a `hum://<host>/<path>` string. Bare paths (no
    /// `hum://` prefix) are rejected — callers wanting "either
    /// bare-path or hum-uri" should branch on
    /// [`HumUri::starts_with_scheme`] first.
    pub fn parse(s: &str) -> Result<Self, UriParseError> {
        let rest = s.strip_prefix("hum://").ok_or(UriParseError::WrongScheme)?;
        let (host_str, path_str) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };
        if host_str.is_empty() { return Err(UriParseError::EmptyHost); }

        // Hid heuristic: starts with one of the known prefixes
        // followed by `_`. Otherwise treat as alias.
        let host = if looks_like_hid(host_str) {
            Hid::from_hex(host_str)
                .map(HostRef::Hid)
                .map_err(UriParseError::BadHid)?
        } else {
            HostRef::Alias(host_str.to_string())
        };
        Ok(HumUri { host, path: path_str.to_string() })
    }

    /// True when the string carries a `hum://` scheme prefix.
    /// Callers building cwd / tool-arg paths use this to decide
    /// whether to route the value through [`HumUri::parse`] or
    /// treat it as a plain local path.
    pub fn starts_with_scheme(s: &str) -> bool {
        s.starts_with("hum://")
    }

    /// Render back to canonical `hum://<host>/<path>` form.
    pub fn to_string_canonical(&self) -> String {
        let host = match &self.host {
            HostRef::Alias(a) => a.clone(),
            HostRef::Hid(h) => h.to_hex(),
        };
        format!("hum://{host}/{}", self.path)
    }

    /// Substitute the host with a resolved [`Hid`]. Returns a new
    /// URI; the original stays unchanged (URIs are values, not
    /// handles). Used by the canonicalization pass in the asker
    /// humd before forwarding tones across the ensemble.
    pub fn with_hid(&self, hid: Hid) -> Self {
        HumUri { host: HostRef::Hid(hid), path: self.path.clone() }
    }
}

fn looks_like_hid(host: &str) -> bool {
    // A hid is `<prefix>_<hex>` where prefix is one of the known
    // tokens. Anything else (no underscore, or unknown prefix) is
    // an alias. Aliases CAN contain underscores (`my_workstation`)
    // — we use prefix matching, not just presence of `_`, so the
    // disambiguation is unambiguous.
    let Some((p, rest)) = host.split_once('_') else { return false };
    if !matches!(p, "humd" | "wbee" | "fbee") { return false; }
    // Rest must be pure hex (length checked by from_hex).
    rest.chars().all(|c| c.is_ascii_hexdigit())
}

/// Trait the asker humd uses to canonicalize aliases to [`Hid`]s
/// before forwarding tones. v0 impl reads `peers.json`; future
/// adapters chain ENS, HNS, libp2p kademlia, etc.
pub trait AliasResolver: Send + Sync {
    /// Resolve `alias` → [`Hid`]. Returns `None` when the alias is
    /// not known to this resolver (callers fall back to the next
    /// resolver in the chain, or surface an error).
    fn resolve(&self, alias: &str) -> Option<Hid>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HidPrefix;

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
        // Underscore in alias but not a known prefix — must stay
        // alias. The disambiguator is prefix-match, not just `_`.
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
