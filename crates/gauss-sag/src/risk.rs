//! Risk lattice + classifier trait (paper §XI.A).
//!
//! The four-band lattice mirrors the paper: `Auto ≤ Notify ≤ RequireApproval
//! ≤ Deny`. Monotonicity (paper §XI.B Theorem A8) says: if input `b` is
//! "more risky" than `a` (higher cap depth, stronger taint, non-reversible
//! where `a` was reversible), then `classify(b) ≥ classify(a)` in this
//! order. The build-time verifier in [`crate::table`] exercises this on a
//! canonical input grid.

use gauss_core::{CapToken, TaintLabel, ToolId};
use serde::{Deserialize, Serialize};

/// Risk band the classifier assigns to an action.
///
/// The variants are ordered most-permissive to most-restrictive; `Ord` is
/// derived from the declaration order, so `risk_a <= risk_b` iff `risk_a`
/// is at most as restrictive as `risk_b`.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Risk {
    /// Auto-execute. No user-facing notification beyond the normal turn
    /// transcript.
    #[default]
    Auto,
    /// Auto-execute, but emit a notification receipt so the human can
    /// audit it asynchronously.
    Notify,
    /// Suspend the turn pending a synchronous user decision.
    RequireApproval,
    /// Refuse the action outright. The DTE returns
    /// [`gauss_core::GaussError::AutonomyDenied`].
    Deny,
}

impl Risk {
    /// True iff this risk band requires a human-in-the-loop decision before
    /// the action may execute.
    #[must_use]
    pub const fn blocks(self) -> bool {
        matches!(self, Self::RequireApproval | Self::Deny)
    }

    /// The lattice join: the more-restrictive of the two outcomes.
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        if (self as u8) >= (other as u8) {
            self
        } else {
            other
        }
    }
}

/// Inputs to a [`Classifier`].
///
/// The struct is `#[non_exhaustive]` so Phase-8+ can add fields (e.g. a
/// `requesting_user`, `prior_denial_count`, etc.) without breaking
/// downstream classifier impls.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RiskInputs {
    /// Capability the action requires (from the tool manifest).
    pub cap: CapToken,
    /// Information-flow taint of the underlying observation chain.
    pub taint: TaintLabel,
    /// Reversibility flag from the tool manifest.
    pub reversible: bool,
    /// Tool identifier — surfaces tool-specific overrides.
    pub tool: ToolId,
}

impl RiskInputs {
    /// Construct an input record.
    #[must_use]
    pub const fn new(cap: CapToken, taint: TaintLabel, reversible: bool, tool: ToolId) -> Self {
        Self {
            cap,
            taint,
            reversible,
            tool,
        }
    }
}

/// The classifier trait. Concrete impls are typically [`crate::DecisionTable`]
/// (rule-driven) or, in Phase 10, a learnt scorer that wraps the table.
pub trait Classifier: Send + Sync {
    /// Classify the action defined by `inputs` into one of the four
    /// [`Risk`] bands.
    fn classify(&self, inputs: &RiskInputs) -> Risk;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_ordering_is_monotone() {
        assert!(Risk::Auto < Risk::Notify);
        assert!(Risk::Notify < Risk::RequireApproval);
        assert!(Risk::RequireApproval < Risk::Deny);
    }

    #[test]
    fn join_is_idempotent_and_commutative() {
        for a in [Risk::Auto, Risk::Notify, Risk::RequireApproval, Risk::Deny] {
            assert_eq!(a.join(a), a, "{a:?} ∨ {a:?}");
            for b in [Risk::Auto, Risk::Notify, Risk::RequireApproval, Risk::Deny] {
                assert_eq!(a.join(b), b.join(a), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn blocks_only_for_approval_or_deny() {
        assert!(!Risk::Auto.blocks());
        assert!(!Risk::Notify.blocks());
        assert!(Risk::RequireApproval.blocks());
        assert!(Risk::Deny.blocks());
    }

    #[test]
    fn inputs_round_trip_through_serde() {
        let r = RiskInputs::new(
            CapToken::NETWORK_GET | CapToken::FILESYSTEM_READ,
            TaintLabel::User,
            true,
            ToolId("fetch".into()),
        );
        let s = serde_json::to_string(&r).unwrap();
        let back: RiskInputs = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }
}
