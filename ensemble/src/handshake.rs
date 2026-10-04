use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{HANDSHAKE_DOMAIN, HANDSHAKE_SKEW_MS, Tone, headroom, now_ms};

pub use hum_identity::{Hid, HidParseError, HidPrefix};

pub struct HumdKey(pub SigningKey);

impl HumdKey {
    pub fn generate() -> Self {
        Self(SigningKey::generate(&mut rand::thread_rng()))
    }

    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    pub fn hid(&self) -> Hid {
        Hid::from_pubkey(HidPrefix::Humd, &self.pubkey_bytes())
    }
}

impl fmt::Debug for HumdKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HumdKey")
            .field("pubkey", &hex::encode(self.pubkey_bytes()))
            .finish()
    }
}

fn handshake_message(humd_id: &Hid, signed_at_ms: i64) -> Vec<u8> {
    format!("{}:{}:{}", HANDSHAKE_DOMAIN, humd_id.to_hex(), signed_at_ms).into_bytes()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumdAddr {
    pub id: Hid,
    #[serde(default)]
    pub hints: Vec<String>,
}

impl HumdAddr {
    pub fn new(id: Hid) -> Self {
        Self {
            id,
            hints: Vec::new(),
        }
    }
    pub fn with_hint(mut self, h: impl Into<String>) -> Self {
        self.hints.push(h.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeerCapabilities {
    pub proto_version: String,
    #[serde(default)]
    pub nests: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub can_relay: bool,
    #[serde(default)]
    pub free_slots: Option<usize>,
    #[serde(default)]
    pub headroom: headroom::CellHeadroom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleHello {
    pub humd_id: Hid,
    pub caps: PeerCapabilities,
}

#[derive(Debug, Clone)]
pub enum HelloParse {
    Verified(Hid, PeerCapabilities),
    Unsigned(Hid, PeerCapabilities),
    Invalid,
}

pub fn hello_tone_unsigned(me: &Hid, caps: &PeerCapabilities) -> Tone {
    serde_json::json!({
        "chi": "hello",
        "rid": hum_identity::HumId::mint().to_string(),
        "from": me.to_hex(),
        "humd_id": me.to_hex(),
        "proto_version": caps.proto_version,
        "nests": caps.nests,
        "hosts": caps.hosts,
        "can_relay": caps.can_relay,
        "free_slots": caps.free_slots,
    })
}

pub fn hello_tone(me: &Hid, key: &HumdKey, caps: &PeerCapabilities) -> Tone {
    let signed_at = now_ms();
    let msg = handshake_message(me, signed_at);
    let sig: Signature = key.0.sign(&msg);
    serde_json::json!({
        "chi": "hello",
        "rid": hum_identity::HumId::mint().to_string(),
        "from": me.to_hex(),
        "humd_id": me.to_hex(),
        "pubkey": hex::encode(key.pubkey_bytes()),
        "proto_version": caps.proto_version,
        "nests": caps.nests,
        "hosts": caps.hosts,
        "can_relay": caps.can_relay,
        "free_slots": caps.free_slots,
        "signed_at": signed_at,
        "signature": hex::encode(sig.to_bytes()),
    })
}

#[async_trait]
pub trait PeerConnection: Send + Sync {
    fn peer(&self) -> &HumdAddr;
    fn capabilities(&self) -> &PeerCapabilities;
    async fn send(&self, tone: Tone) -> Result<()>;
    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>>;
    fn close(&self);

    fn arm_opening(&self) {}
    async fn send_opening(&self, tone: Tone) -> Result<()> {
        self.send(tone).await
    }
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn connect(&self, addr: &HumdAddr) -> Result<Arc<dyn PeerConnection>>;
}

pub fn parse_hello_caps(tone: &Tone) -> Option<(Hid, PeerCapabilities)> {
    let proto_version = tone.get("proto_version")?.as_str()?.to_string();

    let claimed_humd_id_hex = tone.get("humd_id")?.as_str()?;
    let claimed_id = match Hid::from_hex(claimed_humd_id_hex) {
        Ok(h) => h,
        Err(_) => {
            tracing::warn!(target: "ensemble", "hello.rejected: humd_id unparseable");
            return None;
        }
    };

    let pubkey_hex = tone.get("pubkey").and_then(|v| v.as_str())?;
    let pubkey_bytes = hex::decode(pubkey_hex).ok()?;
    if pubkey_bytes.len() != 32 {
        tracing::warn!(target: "ensemble", "hello.rejected: pubkey wrong length");
        return None;
    }
    let mut pubkey_arr = [0u8; 32];
    pubkey_arr.copy_from_slice(&pubkey_bytes);

    if Hid::from_pubkey(HidPrefix::Humd, &pubkey_arr) != claimed_id {
        tracing::warn!(
            target: "ensemble",
            humd_id = %claimed_id.short(),
            "hello.rejected: humd_id does not match sha256(pubkey)"
        );
        return None;
    }

    let signed_at = tone.get("signed_at").and_then(|v| v.as_i64())?;
    let drift = (now_ms() - signed_at).abs();
    if drift > HANDSHAKE_SKEW_MS {
        tracing::warn!(
            target: "ensemble",
            humd_id = %claimed_id.short(),
            drift_ms = drift,
            "hello.rejected: signed_at outside skew window"
        );
        return None;
    }

    let sig_hex = tone.get("signature").and_then(|v| v.as_str())?;
    let sig_bytes = hex::decode(sig_hex).ok()?;
    if sig_bytes.len() != 64 {
        tracing::warn!(target: "ensemble", "hello.rejected: signature wrong length");
        return None;
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    let verifying_key = VerifyingKey::from_bytes(&pubkey_arr).ok()?;
    let msg = handshake_message(&claimed_id, signed_at);
    if verifying_key.verify(&msg, &signature).is_err() {
        tracing::warn!(
            target: "ensemble",
            humd_id = %claimed_id.short(),
            "hello.rejected: signature verification failed"
        );
        return None;
    }

    let nests = tone
        .get("nests")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let hosts = tone
        .get("hosts")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let can_relay = tone
        .get("can_relay")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let free_slots = tone.get("free_slots").and_then(|v| {
        if v.is_null() {
            None
        } else {
            v.as_u64().map(|n| n as usize)
        }
    });
    let headroom = tone
        .get("headroom")
        .and_then(|v| serde_json::from_value::<headroom::CellHeadroom>(v.clone()).ok())
        .unwrap_or_default();
    Some((
        claimed_id,
        PeerCapabilities {
            proto_version,
            nests,
            hosts,
            can_relay,
            free_slots,
            headroom,
        },
    ))
}

pub fn parse_hello(tone: &Tone) -> HelloParse {
    let has_pubkey = tone.get("pubkey").and_then(|v| v.as_str()).is_some();
    if has_pubkey {
        match parse_hello_caps(tone) {
            Some((id, caps)) => HelloParse::Verified(id, caps),
            None => HelloParse::Invalid,
        }
    } else {
        let Some(proto_version) = tone
            .get("proto_version")
            .and_then(|v| v.as_str())
            .map(String::from)
        else {
            return HelloParse::Invalid;
        };
        let Some(humd_hex) = tone.get("humd_id").and_then(|v| v.as_str()) else {
            return HelloParse::Invalid;
        };
        let Ok(claimed_id) = Hid::from_hex(humd_hex) else {
            return HelloParse::Invalid;
        };
        let nests = tone
            .get("nests")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let hosts = tone
            .get("hosts")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let can_relay = tone
            .get("can_relay")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let free_slots = tone.get("free_slots").and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_u64().map(|n| n as usize)
            }
        });
        let headroom = tone
            .get("headroom")
            .and_then(|v| serde_json::from_value::<headroom::CellHeadroom>(v.clone()).ok())
            .unwrap_or_default();
        HelloParse::Unsigned(
            claimed_id,
            PeerCapabilities {
                proto_version,
                nests,
                hosts,
                can_relay,
                free_slots,
                headroom,
            },
        )
    }
}
