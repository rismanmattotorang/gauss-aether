//! `gauss-rsi` — the Gauss-Agent0 weight-frozen recursive self-improvement
//! engine.
//!
//! This crate is the deterministic, I/O-free mathematical core of the
//! Gauss-Agent0 framework (`Gauss-Agent0-PaperV1.0.pdf`, see
//! `/AGENT0_INTEGRATION.md`). Capability accrues not in model weights but in
//! an external, verifiable knowledge-and-skill state `x = (K, S)` composed
//! from a pool of frozen frontier LLMs reached through a mixture-of-experts
//! router. One improvement cycle is the operator Φ; the loop converges
//! geometrically to a verifiable composition closure.
//!
//! Following the workspace's "axioms before features" discipline, every later
//! phase wires real backends ([`gauss_memory`](https://docs.rs)-backed
//! `SurrealDB` state, `gaussclaw-providers-meta` routing, `gauss-poly` /
//! `gauss-exec` verification, `gauss-checkpoint` rollback) *behind*
//! already-proven algorithms. This crate ships those algorithms:
//!
//! ## Phase 0 — engine foundations
//!
//! * [`state`] — the state `x = (K, S)`, the gap metric `d(x, x′)` (Eq. 1),
//!   and the RSI operator Φ (Eq. 2).
//! * [`productivity`] — the Lemma 1 productivity factorization
//!   `ρ ≥ β·εₓ·r_L·p_g·c_v`.
//! * [`converge`] — Theorem 1: geometric gap forecast, the cycle bound `T(ε)`
//!   (Eq. 8), the online `ρ̂` estimator, and the patience-`k` convergence
//!   detector.
//! * [`gdi`] — the SAHOO Goal Drift Index (Eq. 17) and its `τ` gate.
//! * [`event`] — the [`event::CycleEvent`] bus (Appendix B).
//!
//! ## Phase 1 — routing + retrieval-fusion algorithms
//!
//! * [`router`] — Algorithm 3: the cost-aware LinUCB router (Theorem 3).
//! * [`fusion`] — Algorithm 2 fusion stage: reciprocal-rank fusion supplying
//!   the premise-recall factor `r_L`.
//!
//! Every public item is `#[non_exhaustive]` where future fields are expected,
//! the crate is `unsafe`-free (workspace `unsafe_code = forbid`), and all
//! algorithms are deterministic and unit-tested so the conformance suite can
//! drive them from fixed inputs.

// Math notation in the docs (`K°`, `r_L`, `εₓ`, `λ$`, `Aᴷ`, …) and small
// accessor methods trip these pedantic/nursery lints; the same allow block is
// used by sibling crates (`gauss-checkpoint`, `gaussclaw-providers-meta`).
#![allow(clippy::doc_markdown, clippy::missing_const_for_fn)]

pub mod converge;
pub mod event;
pub mod fusion;
pub mod gdi;
pub mod kg;
pub mod productivity;
pub mod router;
pub mod state;

pub use converge::{cycles_to_tolerance, expected_gap, ConvergenceDetector, RhoEstimator};
pub use event::CycleEvent;
pub use fusion::{pack_premises_first, reciprocal_rank_fusion, RankedList, DEFAULT_RRF_K};
pub use gdi::{DriftComponents, DriftGate, DriftVerdict, DriftWeights};
pub use kg::{
    AdmitBatch, Claim, ClaimStatus, Concept, ConceptId, InMemoryKnowledgeStore, KnowledgeStore,
    ModelId, ModelRec, Path, Provenance, Skill, SnapshotId, SCHEMA_SURREALQL,
};
pub use productivity::ProductivityFactors;
pub use router::{cost_adjusted_reward, routing_advantage, ArmWeight, Dispatch, LinUcbRouter};
pub use state::{ClaimId, CountingMeasure, Delta, Measure, SkillId, State};
