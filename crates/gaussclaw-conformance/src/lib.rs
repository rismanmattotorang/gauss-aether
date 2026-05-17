//! `gaussclaw-conformance` — Hermes-parity test suite.
//!
//! Six test classes, each gated on every PR (see `GAUSSCLAW_ROADMAP.md`
//! § "Conformance suite"):
//!
//! 1. **CLI parity** ([`cli_parity`]) — `gaussclaw --help` and per-subcommand
//!    `--help` snapshots locked under `insta`. Phase 1.
//! 2. **TUI snapshot** — Ratatui screen states (Phase 1).
//! 3. **Web e2e** — Playwright suite over Axum backend (Phase 1).
//! 4. **Desktop e2e** — `webdriverio + tauri-driver` (Phase 1).
//! 5. **Hermes-replay** — 1,000-turn corpus byte-equal (Phase 2 onwards).
//! 6. **OpenAI SDK parity** — official SDK suite (Phase 4).
//!
//! Phase 1 ships #1 in this commit; the rest are TBD per phase.

#![allow(clippy::doc_markdown)]

pub mod cli_parity;
pub mod oai_sdk_parity;
pub mod polyhedral_provider;
pub mod replay_corpus;

pub use polyhedral_provider::{
    HandleEquivalenceError, HandleEquivalenceReport, ProviderProbe, verify_handle_equivalence,
};

#[cfg(test)]
mod tests {
    /// Marker test: the conformance crate compiles and links every subcrate
    /// it covers. Real coverage lives in each submodule's `#[test]` blocks.
    #[test]
    fn conformance_crate_links() {
        assert_eq!(2 + 2, 4);
    }
}
