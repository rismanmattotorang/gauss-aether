//! Ed25519 receipt signing (paper §IX, Theorem T11).
//!
//! Phase 5 introduces a pluggable [`SigningBackend`] trait so the
//! [`ReceiptSigner`] can run on:
//!
//! * an in-process `Ed25519` key (the default, used by tests and the toy
//!   provider) — see [`Ed25519Signer`];
//! * an HSM / cloud-KMS backend — bind a custom [`SigningBackend`] impl;
//! * an OS keyring-backed loader (production) — also a custom
//!   [`SigningBackend`] impl that pulls the key material lazily.
//!
//! The wire format is deliberately minimal and self-describing:
//!
//! ```text
//! receipt := turn_id ‖ index ‖ prev_head ‖ payload_digest ‖ post_head ‖ taint ‖ signed_at_ms
//! ```
//!
//! `signature = Ed25519.sign(sk, receipt)`.
//!
//! [`SignedReceipt::verify`] is a pure function over the receipt struct and a
//! caller-supplied public key — no global state. Cross-process verifiers
//! consume [`SignedReceipt`] (serde-friendly) plus the chain payloads.

use core::fmt;

use ed25519_dalek::{
    Signer as _, SigningKey, Verifier as _, VerifyingKey, KEYPAIR_LENGTH, PUBLIC_KEY_LENGTH,
    SECRET_KEY_LENGTH, SIGNATURE_LENGTH,
};
use gauss_core::{GaussError, GaussResult, TaintLabel, TurnId};
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::chain::{link, ChainHead};

/// Length, in bytes, of an Ed25519 signature.
pub const ED25519_SIGNATURE_LEN: usize = SIGNATURE_LENGTH;
/// Length, in bytes, of an Ed25519 public key.
pub const ED25519_PUBLIC_KEY_LEN: usize = PUBLIC_KEY_LENGTH;
/// Length, in bytes, of an Ed25519 secret seed.
pub const ED25519_SECRET_KEY_LEN: usize = SECRET_KEY_LENGTH;

/// A signed receipt covering one append at the audit chain.
///
/// `post_head = link(prev_head, payload)` is recomputed at verification time;
/// `payload_digest = SHA-256(payload)` is stored so verifiers can avoid
/// re-streaming the entire payload when the underlying log already supplies
/// a content-addressed handle.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct SignedReceipt {
    /// Originating turn identifier.
    pub turn_id: TurnId,
    /// 0-based position of this record in the chain.
    pub index: u64,
    /// SHA-256 chain head BEFORE this record was applied.
    pub prev_head: [u8; 32],
    /// SHA-256 of the raw payload (for handle-style verification).
    pub payload_digest: [u8; 32],
    /// SHA-256 chain head AFTER this record was applied.
    pub post_head: [u8; 32],
    /// Information-flow taint of the underlying payload.
    pub taint: TaintLabel,
    /// Wall-clock milliseconds at signing (UTC; informational — DO NOT rely
    /// on this for ordering; the chain index is authoritative).
    pub signed_at_ms: u64,
    /// Ed25519 public key bytes that should verify `signature`.
    pub public_key: [u8; ED25519_PUBLIC_KEY_LEN],
    /// Raw 64-byte Ed25519 signature over the canonical receipt bytes.
    #[serde(with = "BigArray")]
    pub signature: [u8; ED25519_SIGNATURE_LEN],
}

impl SignedReceipt {
    /// Recompute the canonical signing input. Public so cross-language
    /// verifiers can replicate it bit-for-bit.
    ///
    /// Layout (little-endian integer fields):
    /// `turn_id (16) ‖ index (8) ‖ prev_head (32) ‖ payload_digest (32)
    ///  ‖ post_head (32) ‖ taint (1) ‖ signed_at_ms (8)` — total 129 bytes.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(16 + 8 + 32 + 32 + 32 + 1 + 8);
        buf.extend_from_slice(&self.turn_id.0.to_le_bytes());
        buf.extend_from_slice(&self.index.to_le_bytes());
        buf.extend_from_slice(&self.prev_head);
        buf.extend_from_slice(&self.payload_digest);
        buf.extend_from_slice(&self.post_head);
        buf.push(taint_to_byte(self.taint));
        buf.extend_from_slice(&self.signed_at_ms.to_le_bytes());
        buf
    }

    /// Verify the receipt's Ed25519 signature against its embedded public key
    /// AND recompute the chain link `link(prev_head, payload) == post_head`
    /// against a caller-supplied payload.
    ///
    /// # Errors
    /// Returns [`GaussError::SignatureInvalid`] when the signature fails the
    /// elliptic-curve check or when the recomputed chain head diverges.
    pub fn verify(&self, payload: &[u8]) -> GaussResult<()> {
        // 1. Payload digest matches what was signed.
        let observed_digest = sha256(payload);
        if observed_digest != self.payload_digest {
            return Err(GaussError::SignatureInvalid {
                reason: "payload digest mismatch".into(),
            });
        }
        // 2. Chain link is consistent.
        let recomputed = link(ChainHead::from_bytes(self.prev_head), payload);
        if recomputed.as_bytes() != &self.post_head {
            return Err(GaussError::SignatureInvalid {
                reason: "chain head mismatch".into(),
            });
        }
        // 3. Signature verifies.
        let vk = VerifyingKey::from_bytes(&self.public_key).map_err(|e| {
            GaussError::SignatureInvalid {
                reason: format!("public key: {e}"),
            }
        })?;
        let sig = ed25519_dalek::Signature::from_bytes(&self.signature);
        vk.verify(&self.canonical_bytes(), &sig)
            .map_err(|e| GaussError::SignatureInvalid {
                reason: format!("verify: {e}"),
            })
    }

    /// Convenience: verify against a separately-supplied
    /// [`VerifyingKey`]. The verifier MAY rotate the trusted-keys set without
    /// trusting the key embedded in the receipt — useful for revocation.
    ///
    /// # Errors
    /// Same conditions as [`Self::verify`].
    pub fn verify_with_key(&self, payload: &[u8], vk: &VerifyingKey) -> GaussResult<()> {
        if vk.to_bytes() != self.public_key {
            return Err(GaussError::SignatureInvalid {
                reason: "public key mismatch (rotation? revoked?)".into(),
            });
        }
        self.verify(payload)
    }
}

/// Pluggable signing backend. Concrete impls live close to the deployment
/// (HSM client, OS keyring, etc.). The trait is sync because Ed25519 is fast
/// in-process and any async backend can wrap it.
pub trait SigningBackend: Send + Sync {
    /// Public key associated with this backend's signing identity.
    fn public_key(&self) -> [u8; ED25519_PUBLIC_KEY_LEN];

    /// Sign `message` with the backend's secret key.
    ///
    /// # Errors
    /// Propagates backend-specific signing failures.
    fn sign(&self, message: &[u8]) -> GaussResult<[u8; ED25519_SIGNATURE_LEN]>;
}

/// In-process `Ed25519` signer.
///
/// Holds the secret key in a `Zeroize`-on-drop buffer; the public verification
/// key is cached for hot-path use. Construct via [`Self::from_seed`] for
/// deterministic test vectors or [`Self::generate`] for fresh keys.
pub struct Ed25519Signer {
    inner: SigningKey,
    public_key: [u8; ED25519_PUBLIC_KEY_LEN],
}

impl fmt::Debug for Ed25519Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ed25519Signer")
            .field("public_key", &hex::encode(self.public_key))
            .finish_non_exhaustive()
    }
}

impl Ed25519Signer {
    /// Generate a fresh signer from a `CryptoRng`. Use `OsRng` in production.
    pub fn generate<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let inner = SigningKey::generate(rng);
        let public_key = inner.verifying_key().to_bytes();
        Self { inner, public_key }
    }

    /// Build a signer from a 32-byte secret seed (deterministic test vector).
    #[must_use]
    pub fn from_seed(seed: [u8; ED25519_SECRET_KEY_LEN]) -> Self {
        let inner = SigningKey::from_bytes(&seed);
        let public_key = inner.verifying_key().to_bytes();
        // The dalek crate already zeroises on drop; we explicitly drop the
        // caller's seed so it doesn't linger on the stack.
        let mut tmp = seed;
        tmp.zeroize();
        Self { inner, public_key }
    }

    /// Load a 64-byte expanded keypair (32-byte seed ‖ 32-byte public key).
    ///
    /// # Errors
    /// Returns [`GaussError::Internal`] when the embedded public key does not
    /// match the one derived from the seed (corruption detection).
    pub fn from_keypair_bytes(keypair: [u8; KEYPAIR_LENGTH]) -> GaussResult<Self> {
        let mut seed = [0u8; ED25519_SECRET_KEY_LEN];
        seed.copy_from_slice(&keypair[..ED25519_SECRET_KEY_LEN]);
        let signer = Self::from_seed(seed);
        let embedded_pk = &keypair[ED25519_SECRET_KEY_LEN..];
        if embedded_pk != signer.public_key {
            return Err(GaussError::Internal(
                "ed25519 keypair: embedded public key does not match the seed".into(),
            ));
        }
        Ok(signer)
    }

    /// Read-only access to the public key.
    #[must_use]
    pub const fn public_key(&self) -> &[u8; ED25519_PUBLIC_KEY_LEN] {
        &self.public_key
    }
}

impl SigningBackend for Ed25519Signer {
    fn public_key(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
        self.public_key
    }

    fn sign(&self, message: &[u8]) -> GaussResult<[u8; ED25519_SIGNATURE_LEN]> {
        Ok(self.inner.sign(message).to_bytes())
    }
}

/// High-level receipt signer that drives a [`SigningBackend`] over the
/// chain primitives. Construct once per session and reuse — the signer is
/// `Send + Sync` and holds no mutable state.
pub struct ReceiptSigner<B: SigningBackend> {
    backend: B,
}

impl<B: SigningBackend> fmt::Debug for ReceiptSigner<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiptSigner")
            .field("public_key", &hex::encode(self.backend.public_key()))
            .finish_non_exhaustive()
    }
}

impl<B: SigningBackend> ReceiptSigner<B> {
    /// Wrap a signing backend.
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Borrow the backend (e.g. to read the public key from a deployment).
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// Produce a [`SignedReceipt`] for one chain append.
    ///
    /// `signed_at_ms` is the verifier-friendly timestamp; pass UTC ms since
    /// the epoch. Tests pass a fixed value for determinism.
    ///
    /// # Errors
    /// Propagates the backend's signing error.
    pub fn sign_append(
        &self,
        turn_id: TurnId,
        index: u64,
        prev: ChainHead,
        payload: &[u8],
        taint: TaintLabel,
        signed_at_ms: u64,
    ) -> GaussResult<SignedReceipt> {
        let post = link(prev, payload);
        let payload_digest = sha256(payload);
        let mut receipt = SignedReceipt {
            turn_id,
            index,
            prev_head: *prev.as_bytes(),
            payload_digest,
            post_head: *post.as_bytes(),
            taint,
            signed_at_ms,
            public_key: self.backend.public_key(),
            signature: [0u8; ED25519_SIGNATURE_LEN],
        };
        let bytes = receipt.canonical_bytes();
        receipt.signature = self.backend.sign(&bytes)?;
        Ok(receipt)
    }
}

#[inline]
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&out);
    digest
}

#[inline]
const fn taint_to_byte(taint: TaintLabel) -> u8 {
    match taint {
        TaintLabel::Trusted => 0,
        TaintLabel::User => 1,
        TaintLabel::Web => 2,
        TaintLabel::Adversarial => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det_signer() -> Ed25519Signer {
        Ed25519Signer::from_seed([7u8; 32])
    }

    #[test]
    fn signer_round_trips_through_verifier() {
        let signer = ReceiptSigner::new(det_signer());
        let prev = ChainHead::ZERO;
        let payload = b"hello, gauss".to_vec();
        let receipt = signer
            .sign_append(
                TurnId::new(42),
                0,
                prev,
                &payload,
                TaintLabel::User,
                1_234_567_890,
            )
            .unwrap();
        receipt.verify(&payload).unwrap();
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let signer = ReceiptSigner::new(det_signer());
        let receipt = signer
            .sign_append(
                TurnId::new(1),
                0,
                ChainHead::ZERO,
                b"ok",
                TaintLabel::User,
                0,
            )
            .unwrap();
        let err = receipt.verify(b"forged").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("payload digest mismatch"));
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let signer = ReceiptSigner::new(det_signer());
        let mut receipt = signer
            .sign_append(
                TurnId::new(1),
                0,
                ChainHead::ZERO,
                b"x",
                TaintLabel::User,
                0,
            )
            .unwrap();
        // Flip a bit in the signature.
        receipt.signature[0] ^= 0x01;
        let err = receipt.verify(b"x").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("verify"));
    }

    #[test]
    fn from_seed_is_deterministic() {
        let a = Ed25519Signer::from_seed([3u8; 32]);
        let b = Ed25519Signer::from_seed([3u8; 32]);
        assert_eq!(a.public_key(), b.public_key());
    }

    #[test]
    fn keypair_round_trips() {
        let seed = [9u8; 32];
        let signer = Ed25519Signer::from_seed(seed);
        let mut kp = [0u8; KEYPAIR_LENGTH];
        kp[..ED25519_SECRET_KEY_LEN].copy_from_slice(&seed);
        kp[ED25519_SECRET_KEY_LEN..].copy_from_slice(signer.public_key());
        let parsed = Ed25519Signer::from_keypair_bytes(kp).unwrap();
        assert_eq!(parsed.public_key(), signer.public_key());
    }

    #[test]
    fn keypair_corruption_is_detected() {
        let mut kp = [0u8; KEYPAIR_LENGTH];
        kp[..ED25519_SECRET_KEY_LEN].copy_from_slice(&[1u8; 32]);
        // Public key half is zero — won't match the derived one.
        let err = Ed25519Signer::from_keypair_bytes(kp).unwrap_err();
        assert!(format!("{err}").contains("embedded public key"));
    }

    #[test]
    fn verify_with_key_rejects_pk_rotation() {
        let signer = ReceiptSigner::new(det_signer());
        let other = Ed25519Signer::from_seed([8u8; 32]);
        let receipt = signer
            .sign_append(
                TurnId::new(1),
                0,
                ChainHead::ZERO,
                b"x",
                TaintLabel::User,
                0,
            )
            .unwrap();
        let other_vk = ed25519_dalek::VerifyingKey::from_bytes(other.public_key()).unwrap();
        let err = receipt.verify_with_key(b"x", &other_vk).unwrap_err();
        assert!(format!("{err}").contains("public key mismatch"));
    }

    #[test]
    fn canonical_bytes_are_stable_for_fixed_input() {
        let signer = ReceiptSigner::new(det_signer());
        let r = signer
            .sign_append(
                TurnId::new(7),
                3,
                ChainHead::ZERO,
                b"x",
                TaintLabel::User,
                100,
            )
            .unwrap();
        let a = r.canonical_bytes();
        let b = r.canonical_bytes();
        assert_eq!(a, b);
    }
}
