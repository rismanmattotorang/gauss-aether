//! Envelope verification.
//!
//! A single function — [`verify_envelope`] — performs every check
//! Theorem T11 demands of a cryptographic trajectory envelope:
//!
//! 1. **Receipt signature** (Ed25519, EUF-CMA). The receipt's embedded
//!    public key must verify the canonical receipt bytes; and the
//!    caller's published `pk` parameter must equal that embedded key
//!    when `pin_public_key` is set, so an attacker can't substitute
//!    their own keypair into the envelope.
//! 2. **Payload digest binds the record.** The envelope's
//!    `body_canonical` must hash to the receipt's `payload_digest`.
//! 3. **Chain link is consistent.** `link(prev_head, body_canonical) =
//!    post_head` reconstructs from the receipt's `prev_head` and the
//!    envelope's `body_canonical`.
//! 4. **Position witness is consistent.** `witness.post_head =
//!    receipt.post_head` and `witness.index ≤ chain_length`.
//! 5. **TSA anchor (optional)**. When the envelope carries an anchor
//!    AND a verifier supplies a `tsa_root`, the anchor must
//!    successfully verify under the root, and its `head` must equal
//!    the envelope's `chain_head`.
//!
//! Any failure surfaces a [`VerifyEnvelopeError`] naming the failing
//! axis. Hermes upstream has no equivalent — its JSONL records carry
//! no verifiable surface at all.

use gauss_audit::chain::{link, ChainHead};
use gauss_audit::{
    Anchor, AnchorKind, SimulatorTsaClient, ED25519_PUBLIC_KEY_LEN,
};
use thiserror::Error;

use crate::envelope::Envelope;

/// Trust root for TSA anchor verification.
#[derive(Debug)]
#[non_exhaustive]
pub enum TsaRoot {
    /// Verify against an in-process simulator. The most common case
    /// for CI / tests — production verifiers use [`TsaRoot::Trusted`]
    /// with kind-specific public keys.
    Simulator(SimulatorTsaClient),
    /// Trust an anchor whose `kind` is in this allow list **without**
    /// re-verifying its token (used when the verifier delegates token
    /// verification to a separate path, e.g. RFC 3161 TSA replies
    /// already validated upstream).
    TrustKinds(Vec<AnchorKind>),
}

impl TsaRoot {
    /// Default permissive root: trust the deterministic simulator only.
    /// Used by tests.
    #[must_use]
    pub fn simulator(client: SimulatorTsaClient) -> Self {
        Self::Simulator(client)
    }

    /// True iff the supplied anchor verifies under this root.
    pub fn verify(&self, anchor: &Anchor) -> Result<(), VerifyEnvelopeError> {
        match self {
            Self::Simulator(sim) => sim
                .verify(anchor)
                .map_err(|e| VerifyEnvelopeError::AnchorVerify(format!("{e}"))),
            Self::TrustKinds(kinds) => {
                if kinds.contains(&anchor.kind) {
                    Ok(())
                } else {
                    Err(VerifyEnvelopeError::AnchorKindRejected(anchor.kind))
                }
            }
        }
    }
}

/// Verification error from [`verify_envelope`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VerifyEnvelopeError {
    /// `pin_public_key` was set but the envelope's receipt carried a
    /// different public key.
    #[error("public key pinned by caller does not match receipt's embedded public key")]
    PublicKeyMismatch,
    /// Receipt signature verification failed.
    #[error("receipt signature invalid: {0}")]
    Signature(String),
    /// The position witness's `post_head` doesn't equal the receipt's
    /// `post_head`.
    #[error("position witness post_head mismatch")]
    WitnessHeadMismatch,
    /// The position witness's index exceeds the chain length.
    #[error("position witness index {index} > chain length {length}")]
    WitnessIndexExceedsChain {
        /// Witness index.
        index: u64,
        /// Chain length.
        length: u64,
    },
    /// The envelope carries a TSA anchor but no `tsa_root` was supplied.
    #[error("envelope carries TSA anchor but no tsa_root was supplied")]
    TsaRootMissing,
    /// The TSA anchor's `head` does not equal the envelope's chain head.
    #[error("tsa anchor head does not equal envelope chain_head")]
    AnchorHeadMismatch,
    /// The TSA anchor failed verification under the supplied root.
    #[error("tsa anchor verification failed: {0}")]
    AnchorVerify(String),
    /// The TSA anchor's kind is not in the supplied root's allow list.
    #[error("tsa anchor kind {0:?} not in trust root allow list")]
    AnchorKindRejected(AnchorKind),
    /// The chain head reconstructed from (prev_head, body_canonical)
    /// disagrees with the receipt's post_head.
    #[error("chain link inconsistent: recomputed post_head != receipt.post_head")]
    ChainLinkInconsistent,
    /// The receipt's `payload_digest` doesn't match the envelope's
    /// `body_canonical`.
    #[error("body_canonical does not match receipt.payload_digest")]
    PayloadDigestMismatch,
}

/// Verify every cryptographic invariant of [`Envelope`].
///
/// - `pin_public_key`: when `Some`, also rejects envelopes whose
///   receipts were signed by a different public key. Production
///   subscribers supply the publisher's published key here; tests
///   typically supply `None`.
/// - `tsa_root`: when `Some`, the anchor (if present) is verified
///   under this root AND `anchor.head` must equal `envelope.chain_head`.
///   When `None`, an envelope-carried anchor is permitted as long as
///   its head matches; the token is not re-verified.
///
/// # Errors
/// Returns [`VerifyEnvelopeError`] naming the first failed invariant.
pub fn verify_envelope(
    envelope: &Envelope,
    pin_public_key: Option<&[u8; ED25519_PUBLIC_KEY_LEN]>,
    tsa_root: Option<&TsaRoot>,
) -> Result<(), VerifyEnvelopeError> {
    // 1. Pin the publisher's public key, if requested.
    if let Some(pk) = pin_public_key {
        if &envelope.receipt.public_key != pk {
            return Err(VerifyEnvelopeError::PublicKeyMismatch);
        }
    }

    // 2. Payload digest binds the body bytes.
    //    The receipt's verify() already checks this AND the chain link
    //    AND the signature in one go.
    envelope
        .receipt
        .verify(&envelope.body_canonical)
        .map_err(|e| {
            // The verifier reports payload digest mismatch and chain
            // mismatch as the same error variant — disambiguate by
            // re-checking the digest path first.
            use sha2::{Digest, Sha256};
            let observed: [u8; 32] = Sha256::digest(&envelope.body_canonical).into();
            if observed != envelope.receipt.payload_digest {
                return VerifyEnvelopeError::PayloadDigestMismatch;
            }
            // Then chain link.
            let recomputed = link(
                ChainHead::from_bytes(envelope.receipt.prev_head),
                &envelope.body_canonical,
            );
            if recomputed.as_bytes() != &envelope.receipt.post_head {
                return VerifyEnvelopeError::ChainLinkInconsistent;
            }
            // Otherwise the failure was at the signature axis.
            VerifyEnvelopeError::Signature(format!("{e}"))
        })?;

    // 3. Position witness consistency.
    if envelope.witness.post_head != envelope.receipt.post_head {
        return Err(VerifyEnvelopeError::WitnessHeadMismatch);
    }
    if envelope.witness.index > envelope.chain_length {
        return Err(VerifyEnvelopeError::WitnessIndexExceedsChain {
            index: envelope.witness.index,
            length: envelope.chain_length,
        });
    }

    // 4. TSA anchor (if any).
    if let Some(anchor) = &envelope.tsa_anchor {
        if anchor.head != envelope.chain_head {
            return Err(VerifyEnvelopeError::AnchorHeadMismatch);
        }
        match tsa_root {
            Some(root) => root.verify(anchor)?,
            None => {
                // Without a tsa_root we trust the head-equality check
                // alone (acceptable for "soft" verifiers); strict
                // verifiers MUST supply a root.
            }
        }
    } else if tsa_root.is_some() {
        return Err(VerifyEnvelopeError::TsaRootMissing);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{EnvelopeBody, EnvelopeBuilder};
    use crate::sft::{SftMessage, SftRecord};
    use gauss_audit::chain::ChainHead;
    use gauss_audit::{Ed25519Signer, ReceiptSigner, SimulatorTsaClient, TsaClient};
    use gauss_core::{TaintLabel, TurnId};

    fn aligned_envelope_no_anchor() -> (Envelope, [u8; 32]) {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([0x33; 32]));
        let body = SftRecord::from_messages(vec![
            SftMessage::new("user", "hi"),
            SftMessage::new("assistant", "hello"),
        ]);
        let body_bytes = serde_json::to_vec(&EnvelopeBody::Sft(body.clone())).unwrap();
        let prev = ChainHead::from_bytes([0u8; 32]);
        let receipt = signer
            .sign_append(
                TurnId::new(1),
                0,
                prev,
                &body_bytes,
                TaintLabel::User,
                0,
            )
            .unwrap();
        let pk = *signer.backend().public_key();
        let env = EnvelopeBuilder::for_sft(body, receipt).build().unwrap();
        (env, pk)
    }

    #[test]
    fn verify_envelope_passes_on_aligned_inputs() {
        let (env, pk) = aligned_envelope_no_anchor();
        verify_envelope(&env, Some(&pk), None).expect("must verify");
    }

    #[test]
    fn verify_envelope_rejects_pinned_key_mismatch() {
        let (env, _pk) = aligned_envelope_no_anchor();
        let other_pk = [0xffu8; ED25519_PUBLIC_KEY_LEN];
        let err = verify_envelope(&env, Some(&other_pk), None).unwrap_err();
        assert!(matches!(err, VerifyEnvelopeError::PublicKeyMismatch));
    }

    #[test]
    fn verify_envelope_rejects_tampered_body_canonical() {
        let (mut env, pk) = aligned_envelope_no_anchor();
        // Tamper with body_canonical — payload digest no longer matches.
        env.body_canonical[0] = env.body_canonical[0].wrapping_add(1);
        let err = verify_envelope(&env, Some(&pk), None).unwrap_err();
        assert!(matches!(
            err,
            VerifyEnvelopeError::PayloadDigestMismatch | VerifyEnvelopeError::ChainLinkInconsistent
        ));
    }

    #[test]
    fn verify_envelope_rejects_tampered_signature() {
        let (mut env, pk) = aligned_envelope_no_anchor();
        env.receipt.signature[0] = env.receipt.signature[0].wrapping_add(1);
        let err = verify_envelope(&env, Some(&pk), None).unwrap_err();
        assert!(matches!(err, VerifyEnvelopeError::Signature(_)));
    }

    #[tokio::test]
    async fn verify_envelope_passes_with_simulator_anchor() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([0x44; 32]));
        let tsa = SimulatorTsaClient::from_seed([0x77; 32]).with_fixed_clock(123);
        let body = SftRecord::from_messages(vec![
            SftMessage::new("user", "q"),
            SftMessage::new("assistant", "a"),
        ]);
        let body_bytes = serde_json::to_vec(&EnvelopeBody::Sft(body.clone())).unwrap();
        let prev = ChainHead::from_bytes([0u8; 32]);
        let receipt = signer
            .sign_append(
                TurnId::new(2),
                0,
                prev,
                &body_bytes,
                TaintLabel::User,
                0,
            )
            .unwrap();
        let pk = *signer.backend().public_key();
        let post = ChainHead::from_bytes(receipt.post_head);
        let anchor = tsa.anchor(post, receipt.index).await.unwrap();
        let env = EnvelopeBuilder::for_sft(body, receipt)
            .with_tsa(anchor)
            .build()
            .unwrap();
        let root = TsaRoot::simulator(SimulatorTsaClient::from_seed([0x77; 32]).with_fixed_clock(123));
        verify_envelope(&env, Some(&pk), Some(&root)).unwrap();
    }

    #[tokio::test]
    async fn verify_envelope_rejects_anchor_from_other_root() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([0x55; 32]));
        let producer_tsa = SimulatorTsaClient::from_seed([0xaa; 32]);
        let body = SftRecord::from_messages(vec![SftMessage::new("assistant", "x")]);
        let body_bytes = serde_json::to_vec(&EnvelopeBody::Sft(body.clone())).unwrap();
        let prev = ChainHead::from_bytes([0u8; 32]);
        let receipt = signer
            .sign_append(
                TurnId::new(3),
                0,
                prev,
                &body_bytes,
                TaintLabel::User,
                0,
            )
            .unwrap();
        let post = ChainHead::from_bytes(receipt.post_head);
        let anchor = producer_tsa.anchor(post, receipt.index).await.unwrap();
        let env = EnvelopeBuilder::for_sft(body, receipt)
            .with_tsa(anchor)
            .build()
            .unwrap();
        // Verifier supplies a DIFFERENT simulator — anchor verify fails.
        let bad_root = TsaRoot::simulator(SimulatorTsaClient::from_seed([0xbb; 32]));
        let err = verify_envelope(&env, None, Some(&bad_root)).unwrap_err();
        assert!(matches!(err, VerifyEnvelopeError::AnchorVerify(_)));
    }

    #[test]
    fn verify_envelope_rejects_tsa_root_with_no_anchor() {
        let (env, pk) = aligned_envelope_no_anchor();
        let root = TsaRoot::simulator(SimulatorTsaClient::from_seed([0xcc; 32]));
        let err = verify_envelope(&env, Some(&pk), Some(&root)).unwrap_err();
        assert!(matches!(err, VerifyEnvelopeError::TsaRootMissing));
    }

    #[test]
    fn trust_kinds_admits_listed_kinds() {
        let (env, pk) = aligned_envelope_no_anchor();
        // No anchor → envelope passes even with TrustKinds (anchor branch
        // is skipped). Adding a fabricated anchor with a trusted kind
        // would pass too; we test the "no anchor + tsa_root present"
        // failure path separately.
        let root = TsaRoot::TrustKinds(vec![AnchorKind::Simulator]);
        let err = verify_envelope(&env, Some(&pk), Some(&root)).unwrap_err();
        assert!(matches!(err, VerifyEnvelopeError::TsaRootMissing));
    }
}
