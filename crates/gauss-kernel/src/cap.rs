//! Capability lattice `K` — paper Axiom 2, Theorem T2.
//!
//! Phase 0 ships a small, finite cap namespace encoded as a bitmask. The full
//! `CapLattice` over arbitrary `CapNode` instances is Phase 1; for the
//! workspace to compile and exercise the meet/join laws we need a concrete
//! representative now.
//!
//! ## Invariants
//!
//! * `meet` is the lattice infimum (bitwise AND on the mask).
//! * `join` is the lattice supremum (bitwise OR), but Phase 1's full kernel
//!   will gate `join` behind an out-of-band admin operation — for now we
//!   expose it as a pure function and the kernel will wrap it.
//! * `leq` is reflexive, antisymmetric, transitive.

use core::ops::{BitAnd, BitOr};
use serde::{Deserialize, Serialize};

/// A finite capability token, encoded as a bitmask over a fixed namespace.
///
/// The namespace covers the cap classes called out in `SPECS.md` §4.1; future
/// phases may swap this for a richer poset, but the public method signatures
/// stay the same.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapToken(u64);

impl CapToken {
    /// The bottom element `⊥` — no capabilities.
    pub const BOTTOM: Self = Self(0);
    /// The top element `⊤` — every capability in the namespace.
    pub const TOP: Self = Self(u64::MAX);

    // Canonical capability bits. The exact assignments are not stable across
    // versions but the constants themselves are.

    /// Read from a scoped filesystem path.
    pub const FILESYSTEM_READ: Self = Self(1 << 0);
    /// Write to a scoped filesystem path.
    pub const FILESYSTEM_WRITE: Self = Self(1 << 1);
    /// Perform an HTTP GET to an allow-listed origin.
    pub const NETWORK_GET: Self = Self(1 << 2);
    /// Perform an HTTP POST to an allow-listed origin.
    pub const NETWORK_POST: Self = Self(1 << 3);
    /// Spawn a child process (sandboxed).
    pub const SUBPROCESS_SPAWN: Self = Self(1 << 4);
    /// Use a long-lived crypto key for signing.
    pub const CRYPTO_SIGN: Self = Self(1 << 5);
    /// Render to the A2UI Live Canvas.
    pub const CANVAS_RENDER: Self = Self(1 << 6);
    /// Embed an iframe from an allow-listed origin.
    pub const CANVAS_EMBED: Self = Self(1 << 7);
    /// Push a file to the user's downloads.
    pub const CANVAS_FILE_WRITE: Self = Self(1 << 8);

    /// Construct from a raw bitmask.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Return the raw bitmask.
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Lattice meet (`⊓`).
    #[inline]
    #[must_use]
    pub const fn meet(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }

    /// Lattice join (`⊔`).
    ///
    /// Note: in Phase 1 this becomes admin-gated; the bare function survives
    /// as the algebraic primitive that the kernel wraps.
    #[inline]
    #[must_use]
    pub const fn join(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }

    /// True iff `self ⪯ rhs`.
    #[inline]
    #[must_use]
    pub const fn leq(self, rhs: Self) -> bool {
        (self.0 & rhs.0) == self.0
    }

    /// True iff this token contains every bit in `rhs`.
    #[inline]
    #[must_use]
    pub const fn contains(self, rhs: Self) -> bool {
        (self.0 & rhs.0) == rhs.0
    }
}

impl BitAnd for CapToken {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        self.meet(rhs)
    }
}

impl BitOr for CapToken {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        self.join(rhs)
    }
}

/// Trait alias for things that behave like a capability. Phase 1 promotes
/// this to the kernel's public sealed-trait surface.
pub trait Capability: Copy + Eq {
    /// Lattice meet.
    #[must_use]
    fn meet(self, rhs: Self) -> Self;
    /// Lattice join.
    #[must_use]
    fn join(self, rhs: Self) -> Self;
    /// Order: `self ⪯ rhs`.
    fn leq(self, rhs: Self) -> bool;
}

impl Capability for CapToken {
    #[inline]
    fn meet(self, rhs: Self) -> Self {
        Self::meet(self, rhs)
    }
    #[inline]
    fn join(self, rhs: Self) -> Self {
        Self::join(self, rhs)
    }
    #[inline]
    fn leq(self, rhs: Self) -> bool {
        Self::leq(self, rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn meet_with_bottom_is_bottom() {
        let cap = CapToken::NETWORK_GET | CapToken::FILESYSTEM_READ;
        assert_eq!(cap.meet(CapToken::BOTTOM), CapToken::BOTTOM);
    }

    #[test]
    fn join_with_top_is_top() {
        let cap = CapToken::NETWORK_GET;
        assert_eq!(cap.join(CapToken::TOP), CapToken::TOP);
    }

    #[test]
    fn leq_is_reflexive_and_antisymmetric() {
        let a = CapToken::NETWORK_GET | CapToken::CRYPTO_SIGN;
        assert!(a.leq(a));
        let b = CapToken::NETWORK_GET;
        assert!(b.leq(a));
        assert!(!a.leq(b));
    }

    proptest! {
        #[test]
        fn meet_is_commutative(a in any::<u64>(), b in any::<u64>()) {
            let a = CapToken::from_bits(a);
            let b = CapToken::from_bits(b);
            prop_assert_eq!(a.meet(b), b.meet(a));
        }

        #[test]
        fn meet_is_associative(a in any::<u64>(), b in any::<u64>(), c in any::<u64>()) {
            let a = CapToken::from_bits(a);
            let b = CapToken::from_bits(b);
            let c = CapToken::from_bits(c);
            prop_assert_eq!(a.meet(b).meet(c), a.meet(b.meet(c)));
        }

        #[test]
        fn join_is_commutative(a in any::<u64>(), b in any::<u64>()) {
            let a = CapToken::from_bits(a);
            let b = CapToken::from_bits(b);
            prop_assert_eq!(a.join(b), b.join(a));
        }

        #[test]
        fn absorption_holds(a in any::<u64>(), b in any::<u64>()) {
            let a = CapToken::from_bits(a);
            let b = CapToken::from_bits(b);
            prop_assert_eq!(a.meet(a.join(b)), a);
            prop_assert_eq!(a.join(a.meet(b)), a);
        }

        #[test]
        fn leq_transitive(a in any::<u64>(), b in any::<u64>(), c in any::<u64>()) {
            let a = CapToken::from_bits(a);
            let b = CapToken::from_bits(b);
            let c = CapToken::from_bits(c);
            if a.leq(b) && b.leq(c) {
                prop_assert!(a.leq(c));
            }
        }
    }
}
