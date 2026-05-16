//! `gauss-turn` — Differential Turn Engine (DTE).
//!
//! Phase 2 ships:
//!
//! * A real [`TurnEngine`] generic over a [`Kernel`](gauss_traits::Kernel), a
//!   [`MemoryBackend`](gauss_traits::MemoryBackend), and a
//!   [`Provider`](gauss_traits::Provider).
//! * Algorithm 1 of the paper, minus HWCA isolation (Phase 4) and signed
//!   receipts (Phase 5). The WAL-before-effect barrier (Axiom A1) is wired
//!   in now and exercised by the conformance suite.
//! * The Phase-0 type-state shell ([`Turn`], [`run_turn`]) is retained as a
//!   compile-time guard so the lifecycle ordering is unambiguous.
//!
//! See `SPECS.md` §5 for the normative description.

pub mod engine;

pub use engine::{DynSigningBackend, TurnEngine, TurnOutcome, TurnSummary};

use gauss_core::{GaussError, GaussResult, Observation, TurnId};

/// Input to a turn — the observation plus the session it belongs to.
#[derive(Debug, Clone)]
pub struct TurnInput {
    /// Turn identifier (Phase 2 callers assign these explicitly; Phase 6 ULID
    /// generation lands with the snapshot subsystem).
    pub id: TurnId,
    /// Triggering observation.
    pub obs: Observation,
}

/// State markers for the type-state DTE.
pub mod state {
    /// Newly constructed; observation is ingested but the policy has not run.
    #[derive(Debug)]
    pub struct Ingest;
    /// Provider has produced one or more actions; the WAL has not yet been
    /// appended.
    #[derive(Debug)]
    pub struct Generate;
    /// WAL appended; external effects are firing.
    #[derive(Debug)]
    pub struct Commit;
}

/// Turn under construction, parameterised by phase.
#[derive(Debug)]
pub struct Turn<S> {
    input: TurnInput,
    _state: core::marker::PhantomData<fn() -> S>,
}

impl Turn<state::Ingest> {
    /// Start a new turn in the `Ingest` phase.
    #[must_use]
    pub const fn new(input: TurnInput) -> Self {
        Self {
            input,
            _state: core::marker::PhantomData,
        }
    }

    /// Advance to `Generate`.
    pub fn generate(self) -> Turn<state::Generate> {
        Turn {
            input: self.input,
            _state: core::marker::PhantomData,
        }
    }
}

impl Turn<state::Generate> {
    /// Advance to `Commit` without invoking a provider — Phase-0 compat shim.
    ///
    /// # Errors
    /// Currently infallible; kept as `Result` for symmetry with the real
    /// [`TurnEngine::run_turn`] path.
    pub fn commit(self) -> GaussResult<TurnSummary> {
        tracing::trace!(turn_id = ?self.input.id, "phase-0 commit (no-op)");
        Ok(TurnSummary {
            id: self.input.id,
            action_count: 0,
            chain_head: gauss_traits::ChainHeadSnapshot::GENESIS,
            receipt: None,
        })
    }
}

/// Drive a fresh turn end-to-end **without** invoking the real engine.
/// Convenience wrapper retained for Phase-0 conformance; use
/// [`TurnEngine::run_turn`] for the real Algorithm 1 path.
pub fn run_turn(input: TurnInput) -> GaussResult<TurnSummary> {
    Turn::<state::Ingest>::new(input).generate().commit()
}

/// Type-state guard: the engine MUST be polled from the `Ingest` start state.
/// Calling `commit` directly on an `Ingest`-state `Turn` is a compile error.
#[doc(hidden)]
pub fn typestate_proof() -> GaussResult<()> {
    let t = Turn::<state::Ingest>::new(TurnInput {
        id: TurnId::new(0),
        obs: dummy_observation(),
    });
    let t = t.generate();
    let _ = t.commit()?;
    Ok(())
}

fn dummy_observation() -> Observation {
    Observation::new(
        gauss_core::ObservationSource::User {
            channel: String::from("test"),
        },
        gauss_core::TaintLabel::User,
        serde_json::Value::Null,
    )
}

/// Re-export the error type so callers don't need a dep on `gauss-core` just
/// to handle DTE errors.
pub type Error = GaussError;

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::TurnId;

    #[test]
    fn typestate_proof_holds() {
        typestate_proof().unwrap();
    }

    #[test]
    fn shim_run_turn_returns_zero_actions() {
        let summary = run_turn(TurnInput {
            id: TurnId::new(123),
            obs: dummy_observation(),
        })
        .unwrap();
        assert_eq!(summary.id, TurnId::new(123));
        assert_eq!(summary.action_count, 0);
    }
}
