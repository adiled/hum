//! Universal identity primitive for everything addressable on the hum
//! wire: SHA-256 of an Ed25519 public key, written `<prefix>_<hex>`.
//! Content-addressable: no registry, no central naming.
//!
//! The prefix tells a daemon from a participant and nothing more. A
//! bee's role is a claim in its hello, so a worker that starts foraging
//! is still the same bee.
//!
//! The 12-char short form widens with zeros so it still round-trips
//! through serializers. It is for logs and sigils, not for
//! security-bearing addresses.

use std::fmt;

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hid {
    pub prefix: HidPrefix,
    pub bytes: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HidPrefix {
    Humd,
    Bee,
}

impl HidPrefix {
    pub fn as_str(self) -> &'static str {
        match self {
            HidPrefix::Humd => "humd",
            HidPrefix::Bee => "bee",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "humd" => Some(HidPrefix::Humd),
            "bee" => Some(HidPrefix::Bee),
            _ => None,
        }
    }

    pub fn is_bee(self) -> bool {
        self == HidPrefix::Bee
    }
}

impl Hid {
    pub fn from_pubkey(prefix: HidPrefix, pubkey: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(pubkey);
        let digest = h.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest[..32]);
        Self { prefix, bytes }
    }

    pub fn random(prefix: HidPrefix) -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self { prefix, bytes }
    }

    pub fn random_humd() -> Self {
        Self::random(HidPrefix::Humd)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    pub fn to_hex(&self) -> String {
        format!("{}_{}", self.prefix.as_str(), hex::encode(self.bytes))
    }

    pub fn short(&self) -> String {
        format!("{}_{}", self.prefix.as_str(), hex::encode(&self.bytes[..6]))
    }

    pub fn from_hex(s: &str) -> Result<Self, HidParseError> {
        if let Some((p, h)) = s.split_once('_') {
            let prefix = HidPrefix::parse(p).ok_or(HidParseError::UnknownPrefix)?;
            let tail = hex::decode(h).map_err(|_| HidParseError::BadHex)?;
            if tail.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&tail);
                return Ok(Hid { prefix, bytes });
            }
            if tail.len() == 6 {
                let mut bytes = [0u8; 32];
                bytes[..6].copy_from_slice(&tail);
                return Ok(Hid { prefix, bytes });
            }
            return Err(HidParseError::WrongLength(tail.len()));
        }
        let bytes_vec = hex::decode(s).map_err(|_| HidParseError::BadHex)?;
        if bytes_vec.len() != 32 {
            return Err(HidParseError::WrongLength(bytes_vec.len()));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&bytes_vec);
        Ok(Hid {
            prefix: HidPrefix::Humd,
            bytes,
        })
    }
}

#[derive(Debug)]
pub enum HidParseError {
    UnknownPrefix,
    BadHex,
    WrongLength(usize),
}

impl fmt::Display for HidParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HidParseError::UnknownPrefix => write!(f, "unknown hid prefix"),
            HidParseError::BadHex => write!(f, "bad hex"),
            HidParseError::WrongLength(n) => write!(f, "wrong hid length ({n} bytes)"),
        }
    }
}

impl std::error::Error for HidParseError {}

impl fmt::Display for Hid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl Serialize for Hid {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Hid {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        Hid::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

impl From<[u8; 32]> for Hid {
    fn from(bytes: [u8; 32]) -> Self {
        Self {
            prefix: HidPrefix::Humd,
            bytes,
        }
    }
}
