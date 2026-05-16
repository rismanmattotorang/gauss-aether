//! `gauss-turn` — Differential Turn Engine (DTE).
//!
//! Phase 0 ships the type-level state machine for a turn (`Ingest` →
//! `Generate` → `Commit`). The actual provider streaming, WAL barrier, and
//! receipt signing are filled in across Phases 2/4/5.
//!
//! The state machine is encoded with the type-state pattern so that calling
//! `commit` before `generate` is a compile error.

use gauss_core::{GaussError, GaussResult, Observation, TurnId};

/// Input to a turn — the observation plus the session it belongs to.
#[derive(Debug)]
pub struct TurnInput {
    /// Turn identifier (Phase 2 assigns via ULID).
    pub id: TurnId,
    /// Triggering observation.
    pub obs: Observation,
}

/// The outcome of a committed turn. Phase 5 attaches the signed receipt;
/// Phase 2 attaches the record body and post-state.
#[derive(Debug)]
pub struct TurnOutcome {
    /// The turn that was committed.
    pub id: TurnId,
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
    pub fn new(input: TurnInput) -> Self {
        Self {
            input,
            _state: core::marker::PhantomData,
        }
    }

    /// Advance to `Generate`. Phase 2 will run the policy here; Phase 0
    /// simply moves the type-state.
    pub fn generate(self) -> Turn<state::Generate> {
        Turn {
            input: self.input,
            _state: core::marker::PhantomData,
        }
    }
}

impl Turn<state::Generate> {
    /// Advance to `Commit`. Phase 2 will append the WAL entry; Phase 0
    /// returns the outcome directly.
    pub fn commit(self) -> GaussResult<TurnOutcome> {
        tracing::trace!(turn_id = ?self.input.id, "phase-0 commit (no-op)");
        Ok(TurnOutcome { id: self.input.id })
    }
}

/// Drive a fresh turn end-to-end. Convenience wrapper; the real
/// `TurnEngine::run_turn` lands in Phase 2.
pub fn run_turn(input: TurnInput) -> GaussResult<TurnOutcome> {
    Turn::<state::Ingest>::new(input).generate().commit()
}

/// Type-state guard: the engine MUST be polled from the `Ingest` start state.
/// This function is here purely to assert the `Generate`-to-`Commit` ordering
/// at the type level; calling `commit` directly on an `Ingest`-state `Turn`
/// is a compile error, which is the invariant we want.
#[doc(hidden)]
pub fn typestate_proof() -> GaussResult<()> {
    // The lines below would fail to compile if the type-state were broken.
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

    #[test]
    fn turn_drives_to_completion() {
        let input = TurnInput {
            id: TurnId::new(7),
            obs: dummy_observation(),
        };
        let outcome = run_turn(input).expect("phase-0 commit always succeeds");
        assert_eq!(outcome.id, TurnId::new(7));
    }

    #[test]
    fn typestate_proof_holds() {
        typestate_proof().unwrap();
    }
}
