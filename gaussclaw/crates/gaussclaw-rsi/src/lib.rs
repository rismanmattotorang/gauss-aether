//! `gaussclaw-rsi` — live-backend wiring for the Gauss-Agent0 RSI engine.
//!
//! The `gauss-rsi` crate is the deterministic, I/O-free engine core. This
//! crate connects it to the live backends the agent already ships:
//!
//! * [`store::SurrealKnowledgeStore`] — the KnowledgeGraph state on an embedded
//!   SurrealDB instance (claims, skills, `derived_from` edges, provenance,
//!   per-cycle snapshots), implementing [`gauss_rsi::AsyncKnowledgeStore`].
//! * [`expert::ProviderExpert`] — wraps any
//!   [`gaussclaw_agent::ProviderHandle`] (OpenRouter / vendor drivers) as a
//!   frozen frontier [`gauss_rsi::AsyncExpert`], parsing model output into
//!   verifier-gated candidate claims and skills.
//! * [`strategy::LinUcbStrategy`] — the cost-aware LinUCB router (Algorithm 3)
//!   as a `gaussclaw-providers-meta` `SelectionStrategy`, so the live agent's
//!   `NotDiamondProvider` routes by verifier-measured utility.
//!
//! Together these let [`gauss_rsi::AsyncRsiEngine`] run the full
//! self-improvement loop against real I/O. See `AGENT0_INTEGRATION.md`.

#![allow(clippy::doc_markdown)]

pub mod expert;
pub mod store;
pub mod strategy;

pub use expert::ProviderExpert;
pub use store::{add_concept, StoreError, SurrealKnowledgeStore};
pub use strategy::LinUcbStrategy;
