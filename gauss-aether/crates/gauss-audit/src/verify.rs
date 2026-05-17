//! Public verifier API (paper §IX.E + SPECS §9.3).
//!
//! Phase 5 ships the in-process verifier surface. A future HTTP wrapper
//! (Phase 9) is an additive layer that calls these functions through a JSON
//! body. Verification proceeds in three stages:
//!
//! 1. Per-record: signature over the canonical receipt bytes + payload
//!    digest + chain link.
//! 2. Whole-chain: the receipts' `post_head`s form a consistent chain,
//!    optionally cross-checked against a reference head.
//! 3. Anchors: each [`crate::tsa::Anchor`] is verified against its
//!    [`crate::tsa::TsaClient`].

use ed25519_dalek::VerifyingKey;
use gauss_core::{GaussError, GaussResult};

use crate::chain::{link, ChainHead};
use crate::sign::SignedReceipt;
use crate::tsa::{Anchor, AnchorKind, SimulatorTsaClient};

/// Verify one signed receipt against `payload`.
///
/// This is the per-record verifier. For chain-level guarantees use
/// [`verify_chain`].
///
/// # Errors
/// Returns [`GaussError::SignatureInvalid`] on signature, digest, or chain
/// link mismatch.
pub fn verify_receipt(receipt: &SignedReceipt, payload: &[u8]) -> GaussResult<()> {
    receipt.verify(payload)
}

/// Verify a contiguous run of receipts and their payloads.
///
/// Checks:
///
/// * Each receipt's signature + payload digest + chain link (per-record).
/// * The chain is contiguous: `receipts[i].prev_head == receipts[i-1].post_head`.
/// * Indices are strictly increasing (1-step) starting from `receipts[0].index`.
/// * If `expected_final_head` is `Some(h)`, the last receipt's `post_head`
///   must equal `h`.
///
/// # Errors
/// First failing condition is reported via [`GaussError::SignatureInvalid`]
/// or [`GaussError::AuditChainBroken`].
pub fn verify_chain(
    receipts: &[SignedReceipt],
    payloads: &[&[u8]],
    expected_final_head: Option<ChainHead>,
) -> GaussResult<()> {
    if receipts.len() != payloads.len() {
        return Err(GaussError::AuditChainBroken);
    }
    if receipts.is_empty() {
        if let Some(h) = expected_final_head {
            if h != ChainHead::ZERO {
                return Err(GaussError::AuditChainBroken);
            }
        }
        return Ok(());
    }
    let mut prev_post: Option<[u8; 32]> = None;
    let mut prev_index: Option<u64> = None;
    for (r, p) in receipts.iter().zip(payloads.iter()) {
        // Per-record signature + digest + link.
        verify_receipt(r, p)?;
        // Contiguity.
        if let Some(prev) = prev_post {
            if r.prev_head != prev {
                return Err(GaussError::AuditChainBroken);
            }
        }
        if let Some(idx) = prev_index {
            if r.index != idx.saturating_add(1) {
                return Err(GaussError::AuditChainBroken);
            }
        }
        prev_post = Some(r.post_head);
        prev_index = Some(r.index);
    }
    if let Some(expected) = expected_final_head {
        // Safe to unwrap: we returned early on empty.
        let last = receipts.last().expect("non-empty checked above");
        if last.post_head != *expected.as_bytes() {
            return Err(GaussError::AuditChainBroken);
        }
    }
    Ok(())
}

/// Verify an anchor against a single trusted Ed25519 simulator key.
///
/// Production deployments install verifiers for [`AnchorKind::Rfc3161`] and
/// [`AnchorKind::OpenTimestamps`] via additional functions; the trait surface
/// here keeps the conformance suite offline-only.
///
/// # Errors
/// Returns [`GaussError::AnchorFailed`] on token-length, kind, or
/// signature mismatch.
pub fn verify_simulator_anchor(anchor: &Anchor, simulator: &SimulatorTsaClient) -> GaussResult<()> {
    if anchor.kind != AnchorKind::Simulator {
        return Err(GaussError::AnchorFailed(format!(
            "verify_simulator_anchor called on {:?}",
            anchor.kind
        )));
    }
    simulator.verify(anchor)
}

/// Verify that `anchor` covers the head produced by replaying `payloads`.
///
/// This is the "anchor-then-replay" path: the verifier checks the anchor's
/// upstream signature (via `simulator`), then independently replays the
/// payloads through SHA-256 and compares against `anchor.head`.
///
/// # Errors
/// Returns [`GaussError::AnchorFailed`] on signature mismatch or
/// [`GaussError::AuditChainBroken`] when the replay diverges from the
/// anchored head.
pub fn verify_anchor_replay(
    anchor: &Anchor,
    simulator: &SimulatorTsaClient,
    payloads: &[&[u8]],
) -> GaussResult<()> {
    verify_simulator_anchor(anchor, simulator)?;
    let mut head = ChainHead::ZERO;
    for p in payloads {
        head = link(head, p);
    }
    if head.as_bytes() != &anchor.head {
        return Err(GaussError::AuditChainBroken);
    }
    Ok(())
}

/// Decode a 32-byte Ed25519 public key into a [`VerifyingKey`].
///
/// Re-exported for cross-language verifiers that pass a hex-decoded array.
///
/// # Errors
/// Returns [`GaussError::SignatureInvalid`] if the bytes do not form a
/// valid Edwards-curve point.
pub fn verifying_key_from_bytes(bytes: &[u8; 32]) -> GaussResult<VerifyingKey> {
    VerifyingKey::from_bytes(bytes).map_err(|e| GaussError::SignatureInvalid {
        reason: format!("verifying key: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::ReceiptChain;
    use crate::sign::{Ed25519Signer, ReceiptSigner};
    use crate::tsa::{SimulatorTsaClient, TsaClient};
    use gauss_core::{TaintLabel, TurnId};

    fn drive_chain(
        signer: &ReceiptSigner<Ed25519Signer>,
        payloads: &[&[u8]],
    ) -> (Vec<SignedReceipt>, ChainHead) {
        let mut chain = ReceiptChain::new();
        let mut receipts = Vec::with_capacity(payloads.len());
        for (i, p) in payloads.iter().enumerate() {
            let prev = chain.head();
            chain.append(p);
            let idx = u64::try_from(i).unwrap();
            let r = signer
                .sign_append(
                    TurnId::new(u128::from(idx).wrapping_add(1)),
                    idx,
                    prev,
                    p,
                    TaintLabel::User,
                    idx.wrapping_add(100),
                )
                .unwrap();
            receipts.push(r);
        }
        (receipts, chain.head())
    }

    #[test]
    fn verify_chain_accepts_a_valid_run() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([2u8; 32]));
        let payloads: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let (receipts, head) = drive_chain(&signer, &payloads);
        verify_chain(&receipts, &payloads, Some(head)).unwrap();
    }

    #[test]
    fn verify_chain_rejects_index_gap() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([2u8; 32]));
        let payloads: Vec<&[u8]> = vec![b"a", b"b"];
        let (mut receipts, head) = drive_chain(&signer, &payloads);
        receipts[1].index = 99;
        // Re-sign so the per-record check passes; the chain check should still
        // fail because indices are not contiguous.
        receipts[1] = signer
            .sign_append(
                receipts[1].turn_id,
                99,
                ChainHead::from_bytes(receipts[1].prev_head),
                payloads[1],
                TaintLabel::User,
                receipts[1].signed_at_ms,
            )
            .unwrap();
        let err = verify_chain(&receipts, &payloads, Some(head)).unwrap_err();
        assert!(matches!(err, GaussError::AuditChainBroken));
    }

    #[test]
    fn verify_chain_rejects_payload_swap() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([2u8; 32]));
        let payloads: Vec<&[u8]> = vec![b"first", b"second"];
        let (receipts, _) = drive_chain(&signer, &payloads);
        // Swap the payloads — receipt for "first" no longer matches "second".
        let swapped: Vec<&[u8]> = vec![b"second", b"first"];
        verify_chain(&receipts, &swapped, None).unwrap_err();
    }

    #[test]
    fn verify_chain_rejects_final_head_mismatch() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([2u8; 32]));
        let payloads: Vec<&[u8]> = vec![b"x", b"y"];
        let (receipts, _head) = drive_chain(&signer, &payloads);
        let wrong = ChainHead::from_bytes([0xee; 32]);
        let err = verify_chain(&receipts, &payloads, Some(wrong)).unwrap_err();
        assert!(matches!(err, GaussError::AuditChainBroken));
    }

    #[tokio::test]
    async fn anchor_replay_succeeds_for_consistent_log() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([2u8; 32]));
        let payloads: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let (_, head) = drive_chain(&signer, &payloads);
        let sim = SimulatorTsaClient::from_seed([6u8; 32]).with_fixed_clock(1);
        let anchor = sim.anchor(head, 2).await.unwrap();
        verify_anchor_replay(&anchor, &sim, &payloads).unwrap();
    }

    #[tokio::test]
    async fn anchor_replay_rejects_tampered_payload() {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([2u8; 32]));
        let payloads: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let (_, head) = drive_chain(&signer, &payloads);
        let sim = SimulatorTsaClient::from_seed([6u8; 32]).with_fixed_clock(1);
        let anchor = sim.anchor(head, 2).await.unwrap();
        let tampered: Vec<&[u8]> = vec![b"alpha", b"BETA", b"gamma"];
        let err = verify_anchor_replay(&anchor, &sim, &tampered).unwrap_err();
        assert!(matches!(err, GaussError::AuditChainBroken));
    }

    #[test]
    fn verifying_key_round_trips() {
        let signer = Ed25519Signer::from_seed([4u8; 32]);
        let vk = verifying_key_from_bytes(signer.public_key()).unwrap();
        assert_eq!(vk.to_bytes(), *signer.public_key());
    }
}
