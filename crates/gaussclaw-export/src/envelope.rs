//! Cryptographic Trajectory Envelope (Definition 1 of GaussClaw.pdf §VIII).
//!
//! For each SFT (or DPO) record `r_i` produced from a turn `τ_i`, the
//! envelope is the tuple:
//!
//! ```text
//!   E_i = ⟨ r_i, ρ_i, c_n, π_i, TSA(c_n) ⟩
//! ```
//!
//! where:
//!
//! - `r_i` — the SFT or DPO record bytes (canonical serde JSON).
//! - `ρ_i = ⟨turn_id, pk, σ_i, t_i⟩` — the turn's signed receipt
//!   ([`gauss_audit::SignedReceipt`]). Provides EUF-CMA non-repudiation
//!   that the producer signed the turn at chain position `i`.
//! - `c_n` — chain head at envelope creation. Binds the envelope to a
//!   specific tamper-evident chain state.
//! - `π_i` — position witness: the receipt's `post_head` MUST equal the
//!   chain head observed at some index `i ≤ n`. The witness is just
//!   `(i, post_head_i)` — verifiers re-derive consistency against the
//!   producer's chain.
//! - `TSA(c_n)` — optional timestamp-authority anchor proving the
//!   chain head `c_n` existed at wall-clock time. Verifiers refuse the
//!   envelope (in `strict` mode) if the anchor is missing or its
//!   `head ≠ c_n`.
//!
//! The envelope is **optional** for consumers that ignore it (Hermes-
//! compatibility); **mandatory** for federated consumption
//! ([`gaussclaw_fed`]).
//!
//! Hermes upstream emits raw JSONL with no cryptographic surface.
//! GaussClaw's envelope adds the post-hoc verifiable audit chain.

use gauss_audit::{Anchor, SignedReceipt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dpo::DpoRecord;
use crate::sft::SftRecord;

/// Position witness `π_i` for an envelope.
///
/// A producer that emits the envelope reads `ρ_i` from the store at
/// chain position `i ≤ n` and stores `(i, post_head_i)` here. The
/// verifier replays the chain (or consults a precomputed Merkle index)
/// and re-derives the same `post_head_i` at position `i`. The check
/// `i ≤ n` is enforced by the verifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionWitness {
    /// 0-based chain index this receipt occupies.
    pub index: u64,
    /// Chain head digest immediately after this receipt was appended.
    /// Equals `receipt.post_head` for a well-formed envelope.
    #[serde(with = "hex_array_32")]
    pub post_head: [u8; 32],
}

/// The record body — either SFT or DPO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EnvelopeBody {
    /// Supervised fine-tuning record.
    Sft(SftRecord),
    /// Direct preference optimisation record.
    Dpo(DpoRecord),
}

/// The full envelope `E_i`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// `r_i` — the underlying record.
    pub body: EnvelopeBody,
    /// `ρ_i` — turn receipt.
    pub receipt: SignedReceipt,
    /// `c_n` — chain head at envelope creation. Hex-encoded 32 bytes.
    #[serde(with = "hex_array_32")]
    pub chain_head: [u8; 32],
    /// Number of records the chain head covers at envelope creation.
    pub chain_length: u64,
    /// `π_i` — position witness.
    pub witness: PositionWitness,
    /// `TSA(c_n)` — optional TSA anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tsa_anchor: Option<Anchor>,
    /// Canonical bytes of `body` as serialised at envelope time —
    /// stored so verifiers don't depend on serde field-order quirks.
    /// This is the exact byte string that fed `receipt.payload_digest`.
    #[serde(with = "hex_bytes")]
    pub body_canonical: Vec<u8>,
}

/// Error emitted by [`EnvelopeBuilder`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EnvelopeError {
    /// The witness's `post_head` doesn't match the receipt's `post_head`.
    #[error("position witness post_head does not equal receipt.post_head")]
    WitnessHeadMismatch,
    /// The witness's index exceeds the chain length.
    #[error("position witness index {index} > chain length {length}")]
    WitnessIndexExceedsChain {
        /// Witness index.
        index: u64,
        /// Chain length at envelope creation.
        length: u64,
    },
    /// The TSA anchor's head doesn't equal the envelope's `chain_head`.
    #[error("tsa anchor head does not equal envelope chain_head")]
    AnchorHeadMismatch,
    /// Serialisation failed.
    #[error("serialise: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Builder for [`Envelope`].
///
/// Typical use:
///
/// ```ignore
/// let env = EnvelopeBuilder::new(record, receipt)
///     .with_chain(head_bytes, head_length)
///     .with_witness(idx, post_head)
///     .with_tsa(anchor)
///     .build()?;
/// ```
pub struct EnvelopeBuilder {
    body: EnvelopeBody,
    receipt: SignedReceipt,
    chain_head: Option<[u8; 32]>,
    chain_length: u64,
    witness: Option<PositionWitness>,
    tsa_anchor: Option<Anchor>,
}

impl EnvelopeBuilder {
    /// Start a builder around an SFT record + its receipt.
    #[must_use]
    pub fn for_sft(record: SftRecord, receipt: SignedReceipt) -> Self {
        Self {
            body: EnvelopeBody::Sft(record),
            receipt,
            chain_head: None,
            chain_length: 0,
            witness: None,
            tsa_anchor: None,
        }
    }

    /// Start a builder around a DPO record + its receipt.
    #[must_use]
    pub fn for_dpo(record: DpoRecord, receipt: SignedReceipt) -> Self {
        Self {
            body: EnvelopeBody::Dpo(record),
            receipt,
            chain_head: None,
            chain_length: 0,
            witness: None,
            tsa_anchor: None,
        }
    }

    /// Set the chain head and length at envelope creation.
    #[must_use]
    pub const fn with_chain(mut self, head: [u8; 32], length: u64) -> Self {
        self.chain_head = Some(head);
        self.chain_length = length;
        self
    }

    /// Set the position witness explicitly.
    #[must_use]
    pub fn with_witness(mut self, index: u64, post_head: [u8; 32]) -> Self {
        self.witness = Some(PositionWitness { index, post_head });
        self
    }

    /// Attach a TSA anchor over the envelope's chain head.
    #[must_use]
    pub fn with_tsa(mut self, anchor: Anchor) -> Self {
        self.tsa_anchor = Some(anchor);
        self
    }

    /// Finalise. Validates internal consistency before returning.
    ///
    /// # Errors
    /// Returns [`EnvelopeError`] on any internal-consistency failure.
    pub fn build(self) -> Result<Envelope, EnvelopeError> {
        let chain_head = self.chain_head.unwrap_or(self.receipt.post_head);
        let chain_length = if self.chain_length == 0 {
            self.receipt.index.saturating_add(1)
        } else {
            self.chain_length
        };
        let witness = self.witness.unwrap_or(PositionWitness {
            index: self.receipt.index,
            post_head: self.receipt.post_head,
        });
        if witness.post_head != self.receipt.post_head {
            return Err(EnvelopeError::WitnessHeadMismatch);
        }
        if witness.index > chain_length {
            return Err(EnvelopeError::WitnessIndexExceedsChain {
                index: witness.index,
                length: chain_length,
            });
        }
        if let Some(anchor) = &self.tsa_anchor {
            if anchor.head != chain_head {
                return Err(EnvelopeError::AnchorHeadMismatch);
            }
        }
        let body_canonical = serde_json::to_vec(&self.body)?;
        Ok(Envelope {
            body: self.body,
            receipt: self.receipt,
            chain_head,
            chain_length,
            witness,
            tsa_anchor: self.tsa_anchor,
            body_canonical,
        })
    }
}

// ─── serde helpers ──────────────────────────────────────────────────────────

mod hex_array_32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(s).map_err(serde::de::Error::custom)?;
        if v.len() != 32 {
            return Err(serde::de::Error::custom(format!(
                "hex array: expected 32 bytes, got {}",
                v.len()
            )));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        Ok(out)
    }
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sft::{SftMessage, SftRecord};
    use gauss_audit::{Ed25519Signer, ReceiptSigner};
    use gauss_audit::chain::{link, ChainHead};
    use gauss_core::{TaintLabel, TurnId};

    fn sample_receipt() -> SignedReceipt {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([0x11; 32]));
        let prev = ChainHead::from_bytes([0u8; 32]);
        let payload = b"sample-payload".to_vec();
        signer
            .sign_append(
                TurnId::new(7),
                3,
                prev,
                &payload,
                TaintLabel::User,
                1_700_000_000_000,
            )
            .expect("sign")
    }

    fn sample_sft() -> SftRecord {
        SftRecord::from_messages(vec![
            SftMessage::new("user", "q"),
            SftMessage::new("assistant", "a"),
        ])
    }

    #[test]
    fn build_envelope_defaults_chain_to_receipt_post_head() {
        let r = sample_receipt();
        let env = EnvelopeBuilder::for_sft(sample_sft(), r.clone())
            .build()
            .unwrap();
        assert_eq!(env.chain_head, r.post_head);
        assert_eq!(env.witness.index, r.index);
        assert_eq!(env.witness.post_head, r.post_head);
        assert_eq!(env.chain_length, r.index + 1);
        assert!(env.tsa_anchor.is_none());
        assert!(!env.body_canonical.is_empty());
    }

    #[test]
    fn build_envelope_rejects_witness_post_head_mismatch() {
        let r = sample_receipt();
        let bad_head = [0xffu8; 32];
        let err = EnvelopeBuilder::for_sft(sample_sft(), r.clone())
            .with_witness(r.index, bad_head)
            .build()
            .unwrap_err();
        assert!(matches!(err, EnvelopeError::WitnessHeadMismatch));
    }

    #[test]
    fn build_envelope_rejects_witness_index_past_chain_length() {
        let r = sample_receipt();
        let err = EnvelopeBuilder::for_sft(sample_sft(), r.clone())
            .with_chain(r.post_head, 5)
            .with_witness(100, r.post_head)
            .build()
            .unwrap_err();
        assert!(matches!(
            err,
            EnvelopeError::WitnessIndexExceedsChain { index: 100, length: 5 }
        ));
    }

    #[tokio::test]
    async fn build_envelope_rejects_anchor_head_mismatch() {
        use gauss_audit::{SimulatorTsaClient, TsaClient};
        let r = sample_receipt();
        // Anchor a DIFFERENT head so it diverges from the envelope's
        // chain head (which defaults to r.post_head).
        let other = ChainHead::from_bytes([0x99u8; 32]);
        let tsa = SimulatorTsaClient::from_seed([0xddu8; 32]).with_fixed_clock(0);
        let anchor = tsa.anchor(other, r.index).await.unwrap();
        let err = EnvelopeBuilder::for_sft(sample_sft(), r.clone())
            .with_chain(r.post_head, r.index + 1)
            .with_tsa(anchor)
            .build()
            .unwrap_err();
        assert!(matches!(err, EnvelopeError::AnchorHeadMismatch));
    }

    #[test]
    fn envelope_round_trips_through_serde() {
        let r = sample_receipt();
        let env = EnvelopeBuilder::for_sft(sample_sft(), r).build().unwrap();
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn body_canonical_matches_receipt_payload_digest_for_aligned_payload() {
        // We sign the canonical bytes of the SFT body, then build an
        // envelope around it. The body_canonical equals the payload
        // the receipt actually signed.
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([0x22; 32]));
        let body = sample_sft();
        let body_bytes = serde_json::to_vec(&EnvelopeBody::Sft(body.clone())).unwrap();
        let prev = ChainHead::from_bytes([0u8; 32]);
        let receipt = signer
            .sign_append(
                TurnId::new(9),
                0,
                prev,
                &body_bytes,
                TaintLabel::User,
                0,
            )
            .unwrap();
        // Sanity: link reconstruction agrees with receipt.post_head.
        let post = link(prev, &body_bytes);
        assert_eq!(post.as_bytes(), &receipt.post_head);

        let env = EnvelopeBuilder::for_sft(body, receipt.clone())
            .build()
            .unwrap();
        assert_eq!(env.body_canonical, body_bytes);
        // verify() against body_canonical succeeds.
        receipt.verify(&env.body_canonical).unwrap();
    }
}
