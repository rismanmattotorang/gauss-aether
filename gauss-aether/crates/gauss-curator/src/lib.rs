//! `gauss-curator` — cross-session memory map + background skill consolidation
//! + per-turn autosave review.
//!
//! Sprint 5 §5, §6, and §7 of `/ROADMAP.md`. Hermes ships three loosely
//! coupled subsystems for "what happens between turns / between sessions":
//!
//! - `hermes_cli/honcho/` — a per-user memory map with peer / identity /
//!   mode dimensions, surviving session resets.
//! - `agent/curator.py` (~1 500 LOC) — background skill consolidation:
//!   merges narrow skills into umbrella skills, archives stale skills.
//! - `agent/background_review.py` (~550 LOC) — fork a memory-only loop
//!   after each turn to autosave learned skills / memories.
//!
//! This crate ships **structural** parity for all three:
//!
//! 1. **[`cross_session`]** — `PeerId` / `Namespace` / `MemoryRecord`
//!    types, `CrossSessionStore` trait, in-process `InMemoryStore`
//!    reference impl with TTL + last-touched bookkeeping.
//! 2. **[`curator`]** — `Curator::scan_stale(now, max_age)` walks the
//!    store and returns the set of stale records. The actual
//!    "consolidate narrow skills into umbrella via LLM summary" step
//!    is a thin trait the agent loop wires in (the LLM call itself
//!    lives in `gaussclaw-agent`).
//! 3. **[`review`]** — `BackgroundReviewer::record_turn(...)` is the
//!    per-turn autosave hook. Pure data — the host plumbs the hook
//!    into the agent loop's `LoopSink`.
//!
//! ## Hermes-superiority axes
//!
//! - **Cap-gated by construction.** Every write goes through the
//!   `CrossSessionStore` trait, which the host wraps in a kernel
//!   admit gate against [`gauss_core::CapToken::MEMORY_READ`] (read)
//!   and a new `MEMORY_WRITE` cap (Sprint 6). Hermes writes raw.
//! - **Stable schema.** `MemoryRecord` is `serde`-tagged so its
//!   exported form is identical across versions; Hermes's Honcho
//!   uses pickle.
//! - **Deterministic curator.** `Curator::scan_stale` takes an
//!   explicit `now` rather than reading the wall clock, so the
//!   conformance suite drives it from a fixed clock.

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::option_if_let_else,
    clippy::significant_drop_tightening,
    clippy::too_many_lines,
    clippy::too_long_first_doc_paragraph,
    clippy::missing_docs_in_private_items,
    clippy::redundant_clone,
    clippy::missing_errors_doc,
    missing_docs
)]
#![allow(rustdoc::broken_intra_doc_links)]

pub mod cross_session;
pub mod curator;
pub mod review;

pub use cross_session::{
    CrossSessionStore, InMemoryStore, MemoryError, MemoryRecord, MemoryResult, Namespace, PeerId,
};
pub use curator::{ArchiveOutcome, Curator, ScanReport, StaleRecord};
pub use review::{BackgroundReviewer, ReviewEntry, ReviewError};
