//! Capability lattice — re-exported from `gauss-core`.
//!
//! Phase 0 originally held [`CapToken`] here; Phase 2 moves the canonical
//! definition into `gauss-core` so that `Action`, the kernel, the memory
//! backend, and plugin crates can all reference it without a circular
//! dependency. The proptest lattice-law coverage stays here so the kernel
//! crate continues to own its conformance.

pub use gauss_core::{CapToken, Capability};

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
