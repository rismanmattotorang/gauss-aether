//! `gauss-poly` — polyhedral trait-equivalence verifier (Theorem T7).
//!
//! Two implementations of a Gauss-Aether trait are **polyhedrally
//! equivalent** when they produce structurally-equal outputs on every
//! input in a finite probe set (paper §XII.A). The verifier is the
//! mechanical check the build-time `cargo gauss-verify` driver runs over
//! plugin crates; the trait surface is generic enough that callers can
//! verify any pair of `Provider`s, `ApprovalSurface`s, or `ToolTrait`s.
//!
//! Phase 8 ships:
//!
//! * [`Probe`] — one input/expected pair.
//! * [`PolyhedralProbeSet`] — a deterministic collection of probes.
//! * [`verify_provider_equivalence`] — the canonical check for the
//!   `Provider` trait (the only fully-stable plugin trait in Phase 8;
//!   other traits follow the same shape and are wired by the next
//!   helper as new plugin surfaces stabilise).
//! * [`SwapEquivalenceError`] — first-divergence detail report.
//!
//! The polyhedral check is **structural**, not behavioural-up-to-bisim:
//! providers that produce semantically-equivalent JSON but different
//! field orderings will diverge, and that's intentional — the canonical
//! serde form is the contract.

pub mod canonical;
pub mod probe;
pub mod provider;

pub use canonical::{canonical, SNAPSHOT_BYTES};
pub use probe::{PolyhedralProbeSet, Probe};
pub use provider::{verify_provider_equivalence, ProviderEquivalenceReport, SwapEquivalenceError};
