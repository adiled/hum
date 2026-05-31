//! ed25519 signing + verification over canonical event bytes.

use anyhow::{anyhow, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::Hash32;

/// Sign canonical bytes; return hex-encoded 64-byte signature.
pub fn sign_canonical(key: &SigningKey, canonical: &[u8]) -> String {
    let sig: Signature = key.sign(canonical);
    hex::encode(sig.to_bytes())
}

/// Verify a signature over canonical bytes against a pubkey.
pub fn verify_canonical(pubkey: &VerifyingKey, canonical: &[u8], sig_hex: &str) -> Result<()> {
    let sig_bytes = hex::decode(sig_hex).context("decode sig hex")?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|e| anyhow!("sig parse: {e}"))?;
    pubkey.verify(canonical, &sig).map_err(|e| anyhow!("verify: {e}"))
}

/// sha256 of any bytes → 32-byte hash. Used for prev_hash chaining.
pub fn hash256(bytes: &[u8]) -> Hash32 {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn sign_verify_roundtrip() {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let msg = b"some canonical event bytes";
        let sig = sign_canonical(&key, msg);
        verify_canonical(&key.verifying_key(), msg, &sig).expect("verify ok");
    }

    #[test]
    fn verify_rejects_tampered_bytes() {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let sig = sign_canonical(&key, b"original");
        assert!(verify_canonical(&key.verifying_key(), b"tampered", &sig).is_err());
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let mut rng = OsRng;
        let k1 = SigningKey::generate(&mut rng);
        let k2 = SigningKey::generate(&mut rng);
        let sig = sign_canonical(&k1, b"msg");
        assert!(verify_canonical(&k2.verifying_key(), b"msg", &sig).is_err());
    }

    #[test]
    fn hash256_is_deterministic() {
        assert_eq!(hash256(b"hello"), hash256(b"hello"));
        assert_ne!(hash256(b"hello"), hash256(b"world"));
    }
}
