//! `gauss-audit` — Cryptographic Receipt Chain (skeleton).
//!
//! Phase 0 ships the SHA-256 chain primitive `c_i = H(c_{i-1} ‖ ρ_i_bytes)`
//! without any signing — the EUF-CMA receipts arrive in Phase 5. Even at
//! Phase 0 the chain is tamper-evident: any modification to a previously
//! appended payload changes the head with overwhelming probability.

use sha2::{Digest, Sha256};

/// A 32-byte chain head digest.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ChainHead([u8; 32]);

impl ChainHead {
    /// The zero head — the genesis chain anchor.
    pub const ZERO: Self = Self([0u8; 32]);

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
            // write! into a `String` is infallible; the result is ignored.
            let _ = write!(s, "{byte:02x}");
        }
        s
    }
}

/// An append-only Merkle-ish chain.
///
/// At Phase 0 the chain stores nothing but the running head and the count of
/// appended entries; Phase 5 attaches the full receipt body to each link.
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
        let mut hasher = Sha256::new();
        hasher.update(self.head.0);
        hasher.update(payload);
        let digest = hasher.finalize();
        let mut next = [0u8; 32];
        next.copy_from_slice(&digest);
        self.head = ChainHead(next);
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
}

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

    proptest! {
        #[test]
        fn modifying_any_payload_changes_the_final_head(payloads in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..32), 1..16), idx in 0usize..16, mutation in any::<u8>()) {
            // Run-through: chain over the original payloads.
            let mut original = ReceiptChain::new();
            for p in &payloads {
                original.append(p);
            }

            // Build a mutated copy where one payload byte differs (or an
            // entry is inserted at the end if idx >= len).
            let mut mutated_payloads = payloads.clone();
            // proptest guarantees mutated_payloads.len() >= 1 from the
            // generator (1..16); modulo is well-defined and side-effect free.
            #[allow(clippy::arithmetic_side_effects)]
            let target = idx % mutated_payloads.len();
            mutated_payloads[target].push(mutation);

            let mut mutated = ReceiptChain::new();
            for p in &mutated_payloads {
                mutated.append(p);
            }
            prop_assert_ne!(original.head(), mutated.head(),
                "any payload modification must change the head");
        }
    }
}
