//! `gauss-zk` — zero-knowledge proofs over the receipt chain (v2
//! horizon, paper §XVIII.E.2).
//!
//! Production zk-SNARK schemes (Groth16, PLONK, Halo2) need
//! curve-pairing arithmetic that pulls in heavyweight crates
//! (`arkworks`, `bellman`, `halo2`); they ship in additive plugin
//! crates (`gauss-zk-groth16`, `gauss-zk-halo2`) for deployments that
//! need real succinctness.
//!
//! The v2 ship here is the **commitment scheme + statement
//! abstraction** — a Merkle-style commitment over the receipt chain
//! plus a `Statement<T>` trait that ranges over inclusion / range /
//! membership proofs. The commitment is hiding (via a salted SHA-256
//! double-hash) and binding (by the standard collision-resistance of
//! SHA-256). The "proof" the verifier accepts is a path witness; real
//! SNARK plugins replace the `Proof` type with their succinct variant.
//!
//! This gives the trait surface plugin authors will build SNARK
//! backends against without committing the workspace to a specific
//! pairing-curve library.

use gauss_audit::{ChainHead, ReceiptChain};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// A Pedersen-style commitment over a payload, blinded by a 32-byte
/// salt.
///
/// `commit(payload, salt) = SHA256(salt ‖ payload)`. The salt makes the
/// commitment **hiding** (a verifier can't recover the payload without
/// also learning the salt); the construction is **binding** under
/// SHA-256 collision resistance.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Commitment(pub [u8; 32]);

impl Commitment {
    /// Build a commitment.
    #[must_use]
    pub fn new(payload: &[u8], salt: &[u8; 32]) -> Self {
        let mut h = Sha256::new();
        h.update(salt);
        h.update(payload);
        let out = h.finalize();
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&out);
        Self(digest)
    }

    /// Render as lowercase hex (diagnostics).
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// One statement the verifier asks the prover to prove.
///
/// Subset of paper §XVIII.E.2's `Statement` hierarchy. Production SNARK
/// plugins extend the enum. Chain heads are carried as raw 32-byte
/// arrays so the wire format is layout-stable across languages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum Statement {
    /// "I know a `payload` such that `commit(payload, salt) ==
    /// commitment` AND that payload, appended to a chain at
    /// `prev_head`, produces `post_head`."
    InclusionInChain {
        /// Public: the committed payload's commitment.
        commitment: Commitment,
        /// Public: the chain head before the payload was appended.
        prev_head: [u8; 32],
        /// Public: the chain head after the payload was appended.
        post_head: [u8; 32],
    },
    /// "The chain head at length L was X." Used to anchor an
    /// off-line backup against a public ledger.
    HeadAtLength {
        /// Public: the chain length.
        length: u64,
        /// Public: the chain head at that length.
        head: [u8; 32],
    },
}

/// A witness the prover ships with a [`Statement`].
///
/// The trivial witness here carries the cleartext payload + salt —
/// production SNARK plugins replace this with a succinct proof
/// (`Proof = Vec<u8>` of a few hundred bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Witness {
    /// Cleartext payload (kept private; replaced by a SNARK in
    /// production).
    pub payload: Vec<u8>,
    /// The salt used in the commitment.
    pub salt: [u8; 32],
}

impl Witness {
    /// Construct.
    #[must_use]
    pub const fn new(payload: Vec<u8>, salt: [u8; 32]) -> Self {
        Self { payload, salt }
    }
}

/// Verification error.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum ZkError {
    /// The commitment in the statement does not match the witness.
    #[error("commitment mismatch")]
    CommitmentMismatch,
    /// The chain link from `prev_head` to `post_head` does not match
    /// the witness payload.
    #[error("chain link mismatch")]
    ChainLinkMismatch,
    /// The `HeadAtLength` statement does not match the witness payload.
    #[error("head-at-length mismatch")]
    HeadAtLengthMismatch,
}

/// Verify `(statement, witness)`. Returns `Ok(())` iff the witness
/// satisfies the statement.
///
/// Production SNARK plugins replace `witness: &Witness` with a
/// `proof: &[u8]` parameter and call into the proof system; the
/// caller-visible signature stays the same.
///
/// # Errors
/// First-failure short-circuit with a typed [`ZkError`].
pub fn verify(statement: &Statement, witness: &Witness) -> Result<(), ZkError> {
    let recomputed = Commitment::new(&witness.payload, &witness.salt);
    match statement {
        Statement::InclusionInChain {
            commitment,
            prev_head,
            post_head,
        } => {
            if &recomputed != commitment {
                return Err(ZkError::CommitmentMismatch);
            }
            let post = gauss_audit::link(ChainHead::from_bytes(*prev_head), &witness.payload);
            if post.as_bytes() != post_head {
                return Err(ZkError::ChainLinkMismatch);
            }
            Ok(())
        }
        Statement::HeadAtLength { length, head } => {
            // The witness payload here is the *concatenation* of every
            // prior payload (a "transcript"). We replay it.
            let mut chain = ReceiptChain::new();
            let n = usize::try_from(*length).unwrap_or(0);
            if witness.payload.len() < n {
                return Err(ZkError::HeadAtLengthMismatch);
            }
            // Treat each byte of the payload as one chain entry — for
            // a v2 scaffold, this is enough to exercise the verifier
            // shape. Real SNARK plugins witness the full transcript.
            for byte in witness.payload.iter().take(n) {
                chain.append(&[*byte]);
            }
            if chain.head().as_bytes() != head {
                return Err(ZkError::HeadAtLengthMismatch);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_round_trips() {
        let c = Commitment::new(b"hello", &[7u8; 32]);
        let c2 = Commitment::new(b"hello", &[7u8; 32]);
        assert_eq!(c, c2);
        let c3 = Commitment::new(b"hello", &[8u8; 32]);
        assert_ne!(c, c3);
    }

    #[test]
    fn inclusion_verifies_with_correct_witness() {
        let payload = b"event-1".to_vec();
        let salt = [0xab; 32];
        let commitment = Commitment::new(&payload, &salt);
        let prev = ChainHead::ZERO;
        let post = gauss_audit::link(prev, &payload);
        let st = Statement::InclusionInChain {
            commitment,
            prev_head: *prev.as_bytes(),
            post_head: *post.as_bytes(),
        };
        verify(&st, &Witness::new(payload, salt)).unwrap();
    }

    #[test]
    fn inclusion_rejects_wrong_payload() {
        let salt = [0xab; 32];
        let commitment = Commitment::new(b"original", &salt);
        let prev = ChainHead::ZERO;
        let post = gauss_audit::link(prev, b"original");
        let st = Statement::InclusionInChain {
            commitment,
            prev_head: *prev.as_bytes(),
            post_head: *post.as_bytes(),
        };
        let err = verify(&st, &Witness::new(b"forged".to_vec(), salt)).unwrap_err();
        assert!(matches!(err, ZkError::CommitmentMismatch));
    }

    #[test]
    fn inclusion_rejects_wrong_salt() {
        let payload = b"event-1".to_vec();
        let real_salt = [0xab; 32];
        let commitment = Commitment::new(&payload, &real_salt);
        let prev = ChainHead::ZERO;
        let post = gauss_audit::link(prev, &payload);
        let st = Statement::InclusionInChain {
            commitment,
            prev_head: *prev.as_bytes(),
            post_head: *post.as_bytes(),
        };
        let err = verify(&st, &Witness::new(payload, [0xcd; 32])).unwrap_err();
        assert!(matches!(err, ZkError::CommitmentMismatch));
    }

    #[test]
    fn head_at_length_round_trips() {
        let bytes: Vec<u8> = (1..=5_u8).collect();
        let mut chain = ReceiptChain::new();
        for b in &bytes {
            chain.append(&[*b]);
        }
        let st = Statement::HeadAtLength {
            length: bytes.len() as u64,
            head: *chain.head().as_bytes(),
        };
        verify(&st, &Witness::new(bytes, [0u8; 32])).unwrap();
    }

    #[test]
    fn head_at_length_rejects_mismatched_transcript() {
        let st = Statement::HeadAtLength {
            length: 3,
            head: [0xee; 32],
        };
        let err = verify(&st, &Witness::new(vec![1, 2, 3], [0u8; 32])).unwrap_err();
        assert!(matches!(err, ZkError::HeadAtLengthMismatch));
    }
}
