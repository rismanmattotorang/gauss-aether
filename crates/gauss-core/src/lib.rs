//! `gauss-core` — shared types for Gauss-Aether.
//!
//! This crate is the foundation every other Gauss-Aether crate depends on. It
//! is intentionally tiny: it carries identifier newtypes, the `Action` /
//! `Observation` envelopes, taint and capability *placeholder* types, and the
//! unified [`GaussError`] enum. It does not depend on the kernel, the audit
//! chain, or any I/O.
//!
//! Design constraints:
//!
//! * No `unsafe`.
//! * No I/O — `gauss-core` is pure data.
//! * Public types are `#[non_exhaustive]` where future variants are expected,
//!   so adding fields/variants stays semver-minor.
//! * All identifiers are newtypes — never bare `u64` / `String` — to keep the
//!   privilege boundaries in the type system rather than in code reviews.
//!
//! See `SPECS.md` §3 for the normative description.
//!
//! Note: a `no_std` build is on the v2 roadmap; Phase 0 targets std only so
//! that `String`/`Vec` are available without a feature flag.

pub mod action;
pub mod error;
pub mod ids;
pub mod observation;
pub mod taint;

pub use action::{Action, TextAction, ToolAction};
pub use error::{GaussError, RefusalReason};
pub use ids::{AgentId, SessionId, ToolId, TurnId, WorkerId};
pub use observation::{Observation, ObservationSource};
pub use taint::TaintLabel;

/// Convenience alias used throughout the workspace.
pub type GaussResult<T> = core::result::Result<T, GaussError>;
