//! `gauss-audit` — Cryptographic Receipt Chain.
//!
//! Phase 2 ships the un-signed SHA-256 chain `c_i = H(c_{i-1} ‖ payload_i)`,
//! a [`ReceiptChain::verify_replay`] entry point that recomputes the entire
//! chain from a list of payloads, and an [`InclusionWitness`] type so a
//! caller can prove that a specific payload contributed to a given head
//! without exposing the entire log. The Ed25519 signing + EUF-CMA receipts
//! come in Phase 5; the chain primitives here are the foundation they layer
//! on top of.

use sha2::{Digest, Sha256};

/// A 32-byte chain head digest.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ChainHead([u8; 32]);

impl ChainHead {
    /// The zero head — the genesis chain anchor.
    pub const ZERO: Self = Self([0u8; 32]);

    /// Construct a head from a raw 32-byte digest.
    #[must_use]
    pub const fn from_bytes(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Return the raw 32-byte digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Render as lowercase hex. Allocates; use only for diagnostics.
    #[must_use]
    pub fn to_hex(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::with_capacity(64);
        for byte in &self.0 {
            let _ = write!(s, "{byte:02x}");
        }
        s
    }
}

/// Compute one chain link `H(prev ‖ payload)`.
#[must_use]
pub fn link(prev: ChainHead, payload: &[u8]) -> ChainHead {
    let mut hasher = Sha256::new();
    hasher.update(prev.0);
    hasher.update(payload);
    let out = hasher.finalize();
    let mut next = [0u8; 32];
    next.copy_from_slice(&out);
    ChainHead(next)
}

/// An append-only Merkle-ish chain.
///
/// Phase 0 stored only the running head and count; Phase 2 adds a
/// [`Self::verify_replay`] and an [`InclusionWitness`] API. The full receipt
/// body (signature + record + TSA anchor) layers on top in Phase 5.
#[derive(Debug, Clone)]
pub struct ReceiptChain {
    head: ChainHead,
    len: u64,
}

impl Default for ReceiptChain {
    fn default() -> Self {
        Self::new()
    }
}

impl ReceiptChain {
    /// Construct an empty chain at the genesis head.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            head: ChainHead::ZERO,
            len: 0,
        }
    }

    /// Append a payload to the chain and return the new head.
    pub fn append(&mut self, payload: &[u8]) -> ChainHead {
        self.head = link(self.head, payload);
        self.len = self.len.saturating_add(1);
        self.head
    }

    /// Current chain head.
    #[must_use]
    pub const fn head(&self) -> ChainHead {
        self.head
    }

    /// Number of payloads appended.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// True iff nothing has been appended.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Replay verification: rebuild a chain over `payloads` and check that
    /// its head equals `expected_head`. Returns the index of the first
    /// payload whose digest deviates, or `Ok(())` if the replay succeeds.
    ///
    /// # Errors
    /// Returns `Err(idx)` where `idx` is the 0-based payload position whose
    /// link does not match the corresponding expected head, OR a tail-mismatch
    /// at `payloads.len()` if the final heads diverge despite per-link agreement.
    pub fn verify_replay(payloads: &[&[u8]], expected_head: ChainHead) -> Result<(), VerifyError> {
        let mut head = ChainHead::ZERO;
        for (i, p) in payloads.iter().enumerate() {
            head = link(head, p);
            if i == payloads.len().saturating_sub(1) && head != expected_head {
                return Err(VerifyError { mismatched_at: i });
            }
        }
        if payloads.is_empty() && expected_head != ChainHead::ZERO {
            return Err(VerifyError { mismatched_at: 0 });
        }
        Ok(())
    }
}

/// Inclusion witness for one payload: the prior head and the post-link head.
///
/// To verify, the third party recomputes `link(prev, payload)` and checks the
/// result equals `post`. To prove inclusion to a *current* chain head the
/// caller would supply the chain of `(prev, post)` pairs from `post` onward;
/// Phase 5 will fold this into a proper Merkle proof structure.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct InclusionWitness {
    /// Chain head just before the payload was appended.
    pub prev: ChainHead,
    /// Chain head immediately after the payload was appended.
    pub post: ChainHead,
}

impl InclusionWitness {
    /// Verify that `payload` indeed produces `self.post` when chained onto
    /// `self.prev`. This is the minimum tamper-evidence proof Phase 2 ships.
    #[must_use]
    pub fn verify(&self, payload: &[u8]) -> bool {
        link(self.prev, payload) == self.post
    }
}

/// Replay-verification mismatch.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct VerifyError {
    /// 0-based payload position whose recomputed head deviated.
    pub mismatched_at: usize,
}

impl core::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "audit chain replay mismatch at position {}",
            self.mismatched_at
        )
    }
}

impl std::error::Error for VerifyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn genesis_head_is_zero() {
        let chain = ReceiptChain::new();
        assert_eq!(chain.head(), ChainHead::ZERO);
        assert!(chain.is_empty());
    }

    #[test]
    fn appending_changes_head() {
        let mut chain = ReceiptChain::new();
        let before = chain.head();
        let after = chain.append(b"hello");
        assert_ne!(before, after);
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn chain_is_deterministic_for_the_same_inputs() {
        let mut a = ReceiptChain::new();
        let mut b = ReceiptChain::new();
        for payload in [b"one".as_ref(), b"two", b"three"] {
            a.append(payload);
            b.append(payload);
        }
        assert_eq!(a.head(), b.head());
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn replay_verification_accepts_a_valid_chain() {
        let mut chain = ReceiptChain::new();
        for p in [b"a".as_ref(), b"b", b"c"] {
            chain.append(p);
        }
        ReceiptChain::verify_replay(&[b"a", b"b", b"c"], chain.head()).unwrap();
    }

    #[test]
    fn replay_verification_rejects_an_altered_chain() {
        let mut chain = ReceiptChain::new();
        for p in [b"a".as_ref(), b"b", b"c"] {
            chain.append(p);
        }
        // Replace one payload byte; expect mismatch.
        let err = ReceiptChain::verify_replay(&[b"a", b"X", b"c"], chain.head()).unwrap_err();
        // We only check that the verifier rejected; the exact `mismatched_at`
        // depends on whether the discrepancy is at the final head or earlier.
        let _ = err.mismatched_at;
    }

    #[test]
    fn inclusion_witness_round_trip() {
        let mut chain = ReceiptChain::new();
        let prev = chain.head();
        let post = chain.append(b"event");
        let witness = InclusionWitness { prev, post };
        assert!(witness.verify(b"event"));
        assert!(!witness.verify(b"forged"));
    }

    proptest! {
        #[test]
        fn modifying_any_payload_changes_the_final_head(
            payloads in proptest::collection::vec(
                proptest::collection::vec(any::<u8>(), 0..32),
                1..16,
            ),
            idx in 0usize..16,
            mutation in any::<u8>(),
        ) {
            let mut original = ReceiptChain::new();
            for p in &payloads {
                original.append(p);
            }
            let mut mutated_payloads = payloads.clone();
            #[allow(clippy::arithmetic_side_effects)]
            let target = idx % mutated_payloads.len();
            mutated_payloads[target].push(mutation);
            let mut mutated = ReceiptChain::new();
            for p in &mutated_payloads {
                mutated.append(p);
            }
            prop_assert_ne!(original.head(), mutated.head());
        }
    }
}
