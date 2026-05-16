//! Information-flow taint label `L` (paper Axiom 6).
//!
//! Phase 0 ships the total chain `Trusted ≤ User ≤ Web ≤ Adversarial`.
//! The lattice gains its `declass` map and full antitone check in Phase 4.
//!
//! The semantics enforced here are:
//!
//! * `join` is the lattice supremum.
//! * `join` is associative and commutative; absorption holds.
//! * `Trusted` is the bottom and `Adversarial` is the top.

use serde::{Deserialize, Serialize};

/// Total-chain taint lattice.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum TaintLabel {
    /// Bottom — kernel-internal or operator-trusted data.
    #[default]
    Trusted,
    /// User-supplied data on the conversation channel.
    User,
    /// Data fetched from the open web or an untrusted external API.
    Web,
    /// Explicitly adversarial — auto-quarantined input.
    Adversarial,
}

impl TaintLabel {
    /// Lattice join (supremum). Higher taint dominates.
    #[inline]
    #[must_use]
    pub const fn join(self, rhs: Self) -> Self {
        // PartialOrd on the enum is derived from declaration order.
        // We can't call `max` in `const fn`, so we open-code it.
        if (self as u8) >= (rhs as u8) {
            self
        } else {
            rhs
        }
    }

    /// True iff `self ≤ rhs` in the lattice.
    #[inline]
    #[must_use]
    pub const fn leq(self, rhs: Self) -> bool {
        (self as u8) <= (rhs as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_is_commutative() {
        for a in [
            TaintLabel::Trusted,
            TaintLabel::User,
            TaintLabel::Web,
            TaintLabel::Adversarial,
        ] {
            for b in [
                TaintLabel::Trusted,
                TaintLabel::User,
                TaintLabel::Web,
                TaintLabel::Adversarial,
            ] {
                assert_eq!(a.join(b), b.join(a), "join not commutative for {a:?},{b:?}");
            }
        }
    }

    #[test]
    fn join_is_associative() {
        let labels = [
            TaintLabel::Trusted,
            TaintLabel::User,
            TaintLabel::Web,
            TaintLabel::Adversarial,
        ];
        for a in labels {
            for b in labels {
                for c in labels {
                    assert_eq!(a.join(b).join(c), a.join(b.join(c)));
                }
            }
        }
    }

    #[test]
    fn bottom_and_top_are_correct() {
        assert!(TaintLabel::Trusted.leq(TaintLabel::Adversarial));
        assert!(!TaintLabel::Adversarial.leq(TaintLabel::Trusted));
        assert_eq!(
            TaintLabel::Trusted.join(TaintLabel::Adversarial),
            TaintLabel::Adversarial,
            "Adversarial is the top",
        );
    }
}
