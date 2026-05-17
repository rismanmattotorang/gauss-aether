//! `gauss-provider` — pluggable LLM provider adapters.
//!
//! Phase 2 ships [`ToyProvider`], a deterministic in-process provider used by
//! the differential turn engine's end-to-end tests and the conformance suite.
//! Real provider adapters (Anthropic, `OpenAI`, Google, `OpenRouter`,
//! local-Llama) land in Phase 8 alongside the polyhedral-equivalence verifier.
//!
//! See `SPECS.md` §11 for the long-term plugin surface.

pub mod toy;

pub use toy::ToyProvider;
