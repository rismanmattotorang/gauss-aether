//! `gauss-hwca` — Hierarchical Worker-Context Architecture (paper §X).
//!
//! Phase 4 ships the three pillars of Axiom A7 + Theorem T9:
//!
//! 1. **Worker isolation** — every tool invocation runs in a freshly-spawned
//!    [`Worker`]. The raw tool output, intermediate reasoning, and retrieved
//!    content stay in the worker and are dropped at turn boundary.
//! 2. **Schema gate** — only the [`ValidatedValue`](gauss_traits::ValidatedValue)
//!    that conforms to the tool's `OutputSchema` (JSON Schema 2020-12 +
//!    length caps + instruction-substring filter) crosses back.
//! 3. **Recursion-depth bound** — a parent worker MAY spawn sub-workers up to
//!    `WorkerSpawner::max_depth` (default 8). Beyond the bound the spawner
//!    returns [`gauss_core::GaussError::WorkerDepthExceeded`].
//!
//! The Phase-3 sandbox layers run *inside* the worker so Landlock / seccomp /
//! bwrap apply per-tool rather than to the host kernel thread.

#![allow(clippy::module_name_repetitions)]

pub mod corpus;
pub mod filter;
pub mod schema_gate;
pub mod worker;

pub use corpus::{IpiAttempt, IpiCorpus, IpiOutcome};
pub use filter::{contains_instruction_substring, INSTRUCTION_SUBSTRINGS};
pub use schema_gate::{SchemaGate, SchemaGateError};
pub use worker::{Worker, WorkerSpawner};
