use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{CAPS_HANDSHAKE_DOMAIN, HANDSHAKE_DOMAIN, HANDSHAKE_SKEW_MS, Tone, headroom, now_ms};

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

fn caps_message(humd_id: &Hid, caps: &PeerCapabilities, signed_at_ms: i64) -> Vec<u8> {
    let nests = caps.nests.join(",");
    let hosts = caps.hosts.join(",");
    let free_slots = caps.free_slots.unwrap_or(0);
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        CAPS_HANDSHAKE_DOMAIN,
        humd_id.to_hex(),
        signed_at_ms,
        nests,
        hosts,
        caps.can_relay,
        free_slots
    )
    .into_bytes()
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
    #[serde(skip)]
    pub caps_verified: bool,
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

pub fn peer_add_tone(me: &Hid, peer: &Hid) -> Tone {
    serde_json::json!({
        "chi": "peer-add",
        "rid": hum_identity::HumId::mint().to_string(),
        "from": me.to_hex(),
        "humd_id": hex::encode(peer.bytes),
        "hints": Vec::<String>::new(),
    })
}

pub fn peer_remove_tone(me: &Hid, peer: &Hid) -> Tone {
    serde_json::json!({
        "chi": "peer-remove",
        "rid": hum_identity::HumId::mint().to_string(),
        "from": me.to_hex(),
        "humd_id": hex::encode(peer.bytes),
    })
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
    let caps_msg = caps_message(me, caps, signed_at);
    let caps_sig: Signature = key.0.sign(&caps_msg);
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
        "capsSignature": hex::encode(caps_sig.to_bytes()),
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

    let nests: Vec<String> = tone
        .get("nests")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let hosts: Vec<String> = tone
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
    let mut caps_verified = false;
    match tone.get("capsSignature").and_then(|v| v.as_str()) {
        None => {
            tracing::warn!(
                target: "ensemble",
                humd_id = %claimed_id.short(),
                "hello.claims.unsigned — nests/hosts/can_relay/free_slots are self-declared"
            );
        }
        Some(caps_hex) => {
            let Ok(caps_bytes) = hex::decode(caps_hex) else {
                tracing::warn!(target: "ensemble", "hello.rejected: capsSignature unparseable");
                return None;
            };
            if caps_bytes.len() != 64 {
                tracing::warn!(target: "ensemble", "hello.rejected: capsSignature wrong length");
                return None;
            }
            let mut caps_arr = [0u8; 64];
            caps_arr.copy_from_slice(&caps_bytes);
            let caps_signature = Signature::from_bytes(&caps_arr);
            let claims = PeerCapabilities {
                proto_version: proto_version.clone(),
                nests: nests.clone(),
                hosts: hosts.clone(),
                can_relay,
                free_slots,
                headroom: headroom.clone(),
                caps_verified: false,
            };
            let caps_msg = caps_message(&claimed_id, &claims, signed_at);
            if verifying_key.verify(&caps_msg, &caps_signature).is_err() {
                tracing::warn!(
                    target: "ensemble",
                    humd_id = %claimed_id.short(),
                    "hello.rejected: capsSignature verification failed — claims are not what this peer signed"
                );
                return None;
            }
            caps_verified = true;
        }
    }

    Some((
        claimed_id,
        PeerCapabilities {
            proto_version,
            nests,
            hosts,
            can_relay,
            free_slots,
            headroom,
            caps_verified,
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
        let nests: Vec<String> = tone
            .get("nests")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let hosts: Vec<String> = tone
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
                caps_verified: false,
            },
        )
    }
}

pub enum SocketIdentity {
    Verified(Hid),
    Unsigned(Option<Hid>),
    Invalid(&'static str),
}

pub fn parse_socket_identity(tone: &Tone) -> SocketIdentity {
    let Some(pubkey_hex) = tone.get("pubkey").and_then(|v| v.as_str()) else {
        let hid = tone
            .get("hid")
            .and_then(|v| v.as_str())
            .and_then(|s| Hid::from_hex(s).ok());
        return SocketIdentity::Unsigned(hid);
    };

    let Some(hid_hex) = tone.get("hid").and_then(|v| v.as_str()) else {
        return SocketIdentity::Invalid("pubkey sent without hid");
    };
    let Ok(claimed) = Hid::from_hex(hid_hex) else {
        return SocketIdentity::Invalid("hid unparseable");
    };
    if !claimed.prefix.is_bee() {
        return SocketIdentity::Invalid("hid prefix is not a bee");
    }

    let Ok(pubkey_bytes) = hex::decode(pubkey_hex) else {
        return SocketIdentity::Invalid("pubkey unparseable");
    };
    if pubkey_bytes.len() != 32 {
        return SocketIdentity::Invalid("pubkey wrong length");
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&pubkey_bytes);
    if Hid::from_pubkey(claimed.prefix, &pubkey) != claimed {
        tracing::warn!(
            target: "ensemble",
            hid = %claimed.short(),
            "socket.hello.rejected: hid does not match sha256(pubkey)"
        );
        return SocketIdentity::Invalid("hid does not match pubkey");
    }

    let Some(signed_at) = tone.get("signedAt").and_then(|v| v.as_i64()) else {
        return SocketIdentity::Invalid("signedAt missing");
    };
    let drift = (now_ms() - signed_at).abs();
    if drift > HANDSHAKE_SKEW_MS {
        tracing::warn!(
            target: "ensemble",
            hid = %claimed.short(),
            drift_ms = drift,
            "socket.hello.rejected: signedAt outside skew window"
        );
        return SocketIdentity::Invalid("signedAt outside skew window");
    }

    let Some(sig_hex) = tone.get("signature").and_then(|v| v.as_str()) else {
        return SocketIdentity::Invalid("signature missing");
    };
    let Ok(sig_bytes) = hex::decode(sig_hex) else {
        return SocketIdentity::Invalid("signature unparseable");
    };
    if sig_bytes.len() != 64 {
        return SocketIdentity::Invalid("signature wrong length");
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    let Ok(verifying_key) = VerifyingKey::from_bytes(&pubkey) else {
        return SocketIdentity::Invalid("pubkey not a valid curve point");
    };
    let msg = hum_identity::socket_hello_message(&claimed, signed_at);
    if verifying_key.verify(&msg, &signature).is_err() {
        tracing::warn!(
            target: "ensemble",
            hid = %claimed.short(),
            "socket.hello.rejected: signature verification failed"
        );
        return SocketIdentity::Invalid("signature verification failed");
    }

    SocketIdentity::Verified(claimed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_hello(caps: &PeerCapabilities) -> (Hid, HumdKey, Tone) {
        let key = HumdKey::generate();
        let id = key.hid();
        let tone = hello_tone(&id, &key, caps);
        (id, key, tone)
    }

    fn caps() -> PeerCapabilities {
        PeerCapabilities {
            proto_version: "0.7.0".into(),
            nests: vec!["claude-cli".into()],
            hosts: vec!["alice".into()],
            can_relay: false,
            free_slots: Some(4),
            ..Default::default()
        }
    }

    #[test]
    fn a_signed_hello_carries_a_verified_claims_signature() {
        let (id, _key, tone) = signed_hello(&caps());
        let parsed = parse_hello_caps(&tone).expect("hello parses");
        assert_eq!(parsed.0, id);
        assert!(
            parsed.1.caps_verified,
            "a hello from a current peer must be marked as having signed claims"
        );
    }

    #[test]
    fn tampering_with_a_claim_after_signing_fails_verification() {
        let (_id, _key, mut tone) = signed_hello(&caps());
        tone["can_relay"] = serde_json::json!(true);
        assert!(
            parse_hello_caps(&tone).is_none(),
            "a claim the peer did not sign must not be admitted"
        );
    }

    #[test]
    fn widening_the_hosts_list_after_signing_fails_verification() {
        let (_id, _key, mut tone) = signed_hello(&caps());
        tone["hosts"] = serde_json::json!(["alice", "mallory"]);
        assert!(parse_hello_caps(&tone).is_none());
    }

    #[test]
    fn a_forged_caps_signature_is_rejected() {
        let (_id, _key, mut tone) = signed_hello(&caps());
        let other = HumdKey::generate();
        let id = other.hid();
        let claims = PeerCapabilities {
            proto_version: "0.7.0".into(),
            nests: vec!["claude-cli".into()],
            hosts: vec!["alice".into()],
            can_relay: true,
            free_slots: Some(4),
            ..Default::default()
        };
        let sig = other.0.sign(&caps_message(&id, &claims, now_ms()));
        tone["capsSignature"] = serde_json::json!(hex::encode(sig.to_bytes()));
        assert!(parse_hello_caps(&tone).is_none());
    }

    #[test]
    fn a_hello_without_a_claims_signature_still_admits_as_unsigned() {
        let (_id, _key, mut tone) = signed_hello(&caps());
        tone.as_object_mut()
            .expect("tone object")
            .remove("capsSignature");
        let parsed = parse_hello_caps(&tone).expect("still parses");
        assert!(
            !parsed.1.caps_verified,
            "an unsigned hello must not be marked as signed"
        );
    }

    #[test]
    fn a_replayed_claims_signature_from_another_peer_is_rejected() {
        let (_id_a, _key_a, tone_a) = signed_hello(&caps());
        let (id_b, _key_b, mut tone_b) = signed_hello(&caps());
        tone_b["capsSignature"] = tone_a["capsSignature"].clone();
        assert!(parse_hello_caps(&tone_b).is_none(), "B={}", id_b.short());
    }

    fn signed_socket_hello() -> (Hid, SigningKey, Tone) {
        let signing = SigningKey::generate(&mut rand::thread_rng());
        let hid = Hid::from_pubkey(HidPrefix::Bee, &signing.verifying_key().to_bytes());
        let signed_at = now_ms();
        let msg = hum_identity::socket_hello_message(&hid, signed_at);
        let sig = signing.sign(&msg);
        let tone = serde_json::json!({
            "chi": "hello",
            "protoVersion": "0.7.0",
            "hive": "claude-cli",
            "hid": hid.to_hex(),
            "pubkey": hex::encode(signing.verifying_key().to_bytes()),
            "signedAt": signed_at,
            "signature": hex::encode(sig.to_bytes()),
        });
        (hid, signing, tone)
    }

    #[test]
    fn a_signed_socket_hello_proves_the_declared_hid() {
        let (hid, _signing, tone) = signed_socket_hello();
        match parse_socket_identity(&tone) {
            SocketIdentity::Verified(id) => assert_eq!(id, hid),
            other => panic!("expected Verified, got {:?}", socket_dbg(&other)),
        }
    }

    #[test]
    fn a_socket_hello_without_a_signature_is_admitted_as_unsigned() {
        let (hid, _signing, mut tone) = signed_socket_hello();
        tone.as_object_mut().expect("tone object").remove("pubkey");
        tone.as_object_mut()
            .expect("tone object")
            .remove("signedAt");
        tone.as_object_mut()
            .expect("tone object")
            .remove("signature");
        match parse_socket_identity(&tone) {
            SocketIdentity::Unsigned(Some(id)) => assert_eq!(id, hid),
            other => panic!("expected Unsigned(Some), got {:?}", socket_dbg(&other)),
        }
    }

    #[test]
    fn claiming_another_bees_hid_fails_verification() {
        let (_hid, _signing, mut tone) = signed_socket_hello();
        let impostor = Hid::random(HidPrefix::Bee);
        tone["hid"] = serde_json::json!(impostor.to_hex());
        assert!(
            matches!(parse_socket_identity(&tone), SocketIdentity::Invalid(_)),
            "a hid the signer did not sign must be rejected"
        );
    }

    #[test]
    fn a_tampered_hid_with_a_rehashed_pubkey_is_rejected() {
        let (_hid, _signing, mut tone) = signed_socket_hello();
        let mallory = SigningKey::generate(&mut rand::thread_rng());
        let mallory_hid = Hid::from_pubkey(HidPrefix::Bee, &mallory.verifying_key().to_bytes());
        tone["hid"] = serde_json::json!(mallory_hid.to_hex());
        tone["pubkey"] = serde_json::json!(hex::encode(mallory.verifying_key().to_bytes()));
        assert!(
            matches!(parse_socket_identity(&tone), SocketIdentity::Invalid(_)),
            "swapping in a consistent keypair must not bypass the signature"
        );
    }

    #[test]
    fn a_stale_timestamp_is_rejected() {
        let (_hid, _signing, mut tone) = signed_socket_hello();
        tone["signedAt"] = serde_json::json!(now_ms() - HANDSHAKE_SKEW_MS - 1_000);
        assert!(
            matches!(parse_socket_identity(&tone), SocketIdentity::Invalid(_)),
            "a hello outside the skew window is a replay and must be rejected"
        );
    }

    #[test]
    fn a_humd_key_claiming_to_be_a_bee_is_rejected() {
        let signing = SigningKey::generate(&mut rand::thread_rng());
        let hid = Hid::from_pubkey(HidPrefix::Humd, &signing.verifying_key().to_bytes());
        let signed_at = now_ms();
        let msg = hum_identity::socket_hello_message(&hid, signed_at);
        let sig = signing.sign(&msg);
        let tone = serde_json::json!({
            "hid": hid.to_hex(),
            "pubkey": hex::encode(signing.verifying_key().to_bytes()),
            "signedAt": signed_at,
            "signature": hex::encode(sig.to_bytes()),
        });
        assert!(
            matches!(parse_socket_identity(&tone), SocketIdentity::Invalid(_)),
            "a daemon key must not register as a bee"
        );
    }

    #[test]
    fn a_signature_over_a_different_domain_is_rejected() {
        let signing = SigningKey::generate(&mut rand::thread_rng());
        let hid = Hid::from_pubkey(HidPrefix::Bee, &signing.verifying_key().to_bytes());
        let signed_at = now_ms();
        let peer_msg = format!("{}:{}:{}", HANDSHAKE_DOMAIN, hid.to_hex(), signed_at);
        let sig = signing.sign(peer_msg.as_bytes());
        let tone = serde_json::json!({
            "hid": hid.to_hex(),
            "pubkey": hex::encode(signing.verifying_key().to_bytes()),
            "signedAt": signed_at,
            "signature": hex::encode(sig.to_bytes()),
        });
        assert!(
            matches!(parse_socket_identity(&tone), SocketIdentity::Invalid(_)),
            "a signature under the peer domain must not verify on the socket path"
        );
    }

    #[test]
    fn a_signature_over_someone_elses_hid_is_rejected() {
        let mallory = SigningKey::generate(&mut rand::thread_rng());
        let victim = SigningKey::generate(&mut rand::thread_rng());
        let victim_hid = Hid::from_pubkey(HidPrefix::Bee, &victim.verifying_key().to_bytes());

        let signed_at = now_ms();
        let msg = hum_identity::socket_hello_message(&victim_hid, signed_at);
        let sig = mallory.sign(&msg);

        let tone = serde_json::json!({
            "hid": victim_hid.to_hex(),
            "pubkey": hex::encode(mallory.verifying_key().to_bytes()),
            "signedAt": signed_at,
            "signature": hex::encode(sig.to_bytes()),
        });
        assert!(
            matches!(parse_socket_identity(&tone), SocketIdentity::Invalid(_)),
            "a valid signature over a hid the signing key does not own must be rejected"
        );
    }

    fn socket_dbg(id: &SocketIdentity) -> &'static str {
        match id {
            SocketIdentity::Verified(_) => "Verified",
            SocketIdentity::Unsigned(_) => "Unsigned",
            SocketIdentity::Invalid(_) => "Invalid",
        }
    }
}
