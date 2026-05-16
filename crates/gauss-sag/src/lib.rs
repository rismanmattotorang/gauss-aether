//! `gauss-sag` — Supervised Autonomy Gradient (Axiom A8, paper §XI).
//!
//! Phase 7 ships:
//!
//! * [`risk`] — the four-band [`Risk`] outcome lattice
//!   (`Auto ≤ Notify ≤ RequireApproval ≤ Deny`), [`RiskInputs`] capturing
//!   the cap-required / taint / reversibility / tool-id quadruple a
//!   classifier needs, and the [`Classifier`] trait.
//! * [`table`] — a serialisable [`DecisionTable`] of [`Rule`]s with a
//!   build-time [`verify_monotonicity`] check.
//!   The default table (`default_decision_table()`) encodes paper §XI.B:
//!   trusted reversible actions auto-fire; non-reversible `NETWORK_POST` or
//!   `SUBPROCESS_SPAWN` require approval; `CRYPTO_SIGN` always requires
//!   approval; adversarial-tainted actions are denied outright.
//! * [`approval`] — the [`ApprovalSurface`] trait + three test surfaces
//!   ([`AutoApprove`], [`AutoDeny`], [`ChannelSurface`]) so the DTE can
//!   drive approval round-trips deterministically. Production adapters
//!   (Telegram, Slack, Discord, Matrix, CLI/TUI, SSE) ship in Phase 9 as
//!   additive impls.
//! * [`gate::ApprovalGate`] — wraps a `Classifier` + `ApprovalSurface` into
//!   a single `decide_action(action) -> Outcome` helper that the
//!   `gauss-turn::TurnEngine` calls inline. The gate honours a
//!   configurable [`DEFAULT_DEADLINE`] (5 minutes per SPECS §XI.C).
//!
//! The crate is `unsafe`-free (workspace lint `unsafe_code = forbid`) and
//! its public surface is `#[non_exhaustive]` throughout for semver-minor
//! evolution.

pub mod approval;
pub mod gate;
pub mod risk;
pub mod table;

pub use approval::{
    ApprovalDecision, ApprovalRequest, ApprovalSurface, AutoApprove, AutoDeny, ChannelSurface,
    DEFAULT_DEADLINE,
};
pub use gate::{ApprovalGate, Outcome};
pub use risk::{Classifier, Risk, RiskInputs};
pub use table::{
    default_decision_table, verify_monotonicity, DecisionTable, MonotonicityError, Predicate, Rule,
};
