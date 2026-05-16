//! `gauss-conformance` — axiomatic conformance test harness.
//!
//! Each test in this crate corresponds to one of the nine axioms (A1–A9) or
//! twelve theorems (T1–T12) named in the source paper. Phase 0 wires up the
//! harness shape; later phases tighten the assertions.
//!
//! The current state per axiom / theorem:
//!
//! | ID | Status Phase 0                                        | Phase that closes |
//! |----|-------------------------------------------------------|--------------------|
//! | A2 | basic capability monotonicity over `CapToken`         | Phase 1            |
//! | A3 | chain tamper detection via SHA-256                    | Phase 2            |
//! | A4 | cross-plane starvation independence (smoke)           | Phase 1            |
//! | A6 | taint join commutativity / associativity              | Phase 4            |
//! | T1 | type-state DTE drives Ingest → Generate → Commit      | Phase 2            |
//! | T3 | head changes on payload mutation                      | Phase 2            |
//! | T4 | `worst_case_wait = B / ρ`                             | Phase 1            |

// This crate is intentionally test-driven. The library itself just re-exports
// the workspace surface so other crates can pull in the conformance scenarios
// programmatically if they want.

pub use gauss_audit::ReceiptChain;
pub use gauss_core::{TaintLabel, TurnId};
pub use gauss_kernel::{CapToken, Plane, Planes};

#[cfg(test)]
mod axiom_a2_capability_monotonicity {
    use gauss_kernel::CapToken;

    #[test]
    fn meet_reduces_below_both_arguments() {
        let a = CapToken::NETWORK_GET | CapToken::FILESYSTEM_READ;
        let b = CapToken::FILESYSTEM_READ | CapToken::CRYPTO_SIGN;
        let m = a.meet(b);
        assert!(m.leq(a));
        assert!(m.leq(b));
    }
}

#[cfg(test)]
mod axiom_a4_fairness_separation {
    use gauss_kernel::{Plane, Planes};

    #[test]
    fn draining_daemon_does_not_starve_conversation() {
        let planes = Planes::with_defaults();
        let now = std::time::Instant::now();
        while planes.pool(Plane::Daemon).try_acquire_at(now) {}
        // Daemon drained; conversation still serves.
        assert!(planes.pool(Plane::Conversation).try_acquire_at(now));
    }
}

#[cfg(test)]
mod axiom_a6_taint_lattice {
    use gauss_core::TaintLabel;

    #[test]
    fn join_is_commutative_for_named_labels() {
        assert_eq!(
            TaintLabel::User.join(TaintLabel::Web),
            TaintLabel::Web.join(TaintLabel::User),
        );
    }
}

#[cfg(test)]
mod theorem_t3_merkle_tamper_evidence {
    use gauss_audit::ReceiptChain;

    #[test]
    fn appending_distinct_bytes_diverges_head() {
        let mut a = ReceiptChain::new();
        let mut b = ReceiptChain::new();
        a.append(b"x");
        b.append(b"y");
        assert_ne!(a.head(), b.head());
    }
}

#[cfg(test)]
mod theorem_t4_starvation_bound {
    use gauss_kernel::PlanePool;
    use std::time::Duration;

    #[test]
    fn worst_case_wait_matches_b_over_rho() {
        let pool = PlanePool::new(20.0, 5.0);
        assert_eq!(pool.worst_case_wait(), Duration::from_secs(4));
    }
}

#[cfg(test)]
mod theorem_t1_typestate_dte {
    use gauss_core::{Observation, ObservationSource, TaintLabel, TurnId};
    use gauss_turn::{run_turn, TurnInput};

    #[test]
    fn run_turn_completes() {
        let outcome = run_turn(TurnInput {
            id: TurnId::new(99),
            obs: Observation::new(
                ObservationSource::User {
                    channel: "stub".into(),
                },
                TaintLabel::User,
                serde_json::Value::Null,
            ),
        })
        .unwrap();
        assert_eq!(outcome.id, TurnId::new(99));
    }
}
