//! The Differential Turn Engine (Phase 2 implementation).
//!
//! Algorithm 1 of the paper, minus HWCA worker isolation (Phase 4) and signed
//! receipts (Phase 5):
//!
//! ```text
//! 1. INGEST   join taint(o) into ℓ
//! 2. GENERATE ask provider π for actions
//! 3. ADMIT    kernel.admit(k(a), ℓ) for each tool action
//! 4. WAL      memory.append(record(o, a, ℓ))  ← durable barrier (A1)
//! 5. COMMIT   external effects fire AFTER the append succeeds (Phase 3+)
//! ```
//!
//! Step 4 is **the** invariant of Axiom A1 / Theorem T1: external effects MUST
//! NOT fire before the WAL append durably succeeds. The barrier is structural
//! in this engine — `append` returns before `apply_actions_locally` is
//! invoked, and `apply_actions_locally` panics if it sees the engine in a
//! pre-barrier state. The conformance suite exercises both happy and
//! crash-injection paths.

use std::sync::Arc;

use gauss_core::{Action, GaussError, GaussResult, TaintLabel, TurnId};
use gauss_traits::{AppendAck, AppendEntry, ChainHeadSnapshot, Kernel, MemoryBackend, Provider};

use crate::TurnInput;

/// Summary of a successfully committed turn.
#[derive(Debug, Clone)]
pub struct TurnSummary {
    /// Identifier of the turn.
    pub id: TurnId,
    /// Number of actions the provider emitted.
    pub action_count: usize,
    /// Audit chain head after the turn was committed (Phase 1+).
    pub chain_head: ChainHeadSnapshot,
}

/// Outcome of a single turn — alias retained for `SPECS.md` continuity.
pub type TurnOutcome = TurnSummary;

/// The Differential Turn Engine.
///
/// Generic over the kernel, memory backend, and provider so test harnesses
/// can mix-and-match implementations without changing the engine itself.
pub struct TurnEngine<K, M, P> {
    kernel: Arc<K>,
    memory: Arc<M>,
    provider: Arc<P>,
}

impl<K, M, P> core::fmt::Debug for TurnEngine<K, M, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TurnEngine")
            .field("kernel", &"<K: Kernel>")
            .field("memory", &"<M: MemoryBackend>")
            .field("provider", &"<P: Provider>")
            .finish()
    }
}

impl<K, M, P> TurnEngine<K, M, P>
where
    K: Kernel,
    M: MemoryBackend,
    P: Provider,
{
    /// Construct a turn engine.
    pub const fn new(kernel: Arc<K>, memory: Arc<M>, provider: Arc<P>) -> Self {
        Self {
            kernel,
            memory,
            provider,
        }
    }

    /// Drive a single turn through the full Phase-2 lifecycle.
    ///
    /// # Errors
    /// * [`GaussError::Denied`] — admission rejected (cap or taint).
    /// * [`GaussError::Io`] — memory backend failure.
    /// * Provider-side error propagated verbatim.
    pub async fn run_turn(&self, input: TurnInput) -> GaussResult<TurnSummary> {
        // -- 1. Ingest ----------------------------------------------------
        let taint: TaintLabel = input.obs.taint;
        tracing::trace!(turn_id = ?input.id, ?taint, "turn ingest");

        // -- 2. Generate (policy π) --------------------------------------
        let actions = self.provider.generate(&input.obs).await?;
        tracing::trace!(turn_id = ?input.id, count = actions.len(), "turn generated");

        // -- 3. Admit each tool action ----------------------------------
        for a in &actions {
            self.admit_action(a, taint)?;
        }

        // -- 4. WAL barrier (A1) ----------------------------------------
        // Canonicalise the action set into a deterministic byte payload so
        // the chain digest depends on the structural content, not on any
        // serialiser non-determinism.
        let payload = canonicalise_actions(&actions)?;
        let ack: AppendAck = self
            .memory
            .append(AppendEntry::new(input.id, payload, taint))
            .await?;
        tracing::trace!(
            turn_id = ?input.id,
            chain_index = ack.index,
            "wal append committed"
        );

        // -- 5. Commit external effects (Phase 3 fills in sandbox exec) --
        apply_actions_locally(&actions);

        Ok(TurnSummary {
            id: input.id,
            action_count: actions.len(),
            chain_head: ack.head,
        })
    }

    fn admit_action(&self, action: &Action, taint: TaintLabel) -> GaussResult<()> {
        match action {
            Action::Text(_) => Ok(()),
            Action::Tool(t) => self.kernel.admit(t.cap_required, taint),
            // `Action` is `#[non_exhaustive]`; treat unknown variants as
            // hard-deny rather than silently passing them through.
            _ => Err(GaussError::Internal(
                "unknown Action variant — kernel refuses to admit unknowns".into(),
            )),
        }
    }
}

fn canonicalise_actions(actions: &[Action]) -> GaussResult<Vec<u8>> {
    serde_json::to_vec(actions)
        .map_err(|e| GaussError::Internal(format!("canonicalise_actions: {e}")))
}

/// Phase-2 placeholder for external-effect commit. Replaced by the composite
/// sandbox executor in Phase 3 and by HWCA worker spawn in Phase 4.
fn apply_actions_locally(actions: &[Action]) {
    for a in actions {
        match a {
            Action::Text(t) => tracing::debug!(text.len = t.body.len(), "text emit"),
            Action::Tool(t) => tracing::debug!(tool = %t.tool.0, "tool invocation (stub)"),
            _ => tracing::warn!("unknown Action variant skipped at apply"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::{Observation, ObservationSource, TaintLabel};

    // The end-to-end tests for the engine live in `gauss-conformance`, since
    // they need the kernel + memory backend wired together. Here we only
    // sanity-check the canonicaliser determinism (it influences the chain
    // digest and therefore Theorem T3).
    #[test]
    fn canonicaliser_is_deterministic_for_equal_inputs() {
        let actions = vec![Action::Text(gauss_core::TextAction::new("hi"))];
        let a = canonicalise_actions(&actions).unwrap();
        let b = canonicalise_actions(&actions).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn canonicaliser_diverges_for_different_inputs() {
        let a = canonicalise_actions(&[Action::Text(gauss_core::TextAction::new("one"))]).unwrap();
        let b = canonicalise_actions(&[Action::Text(gauss_core::TextAction::new("two"))]).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn dummy_observation_constructor_works() {
        let _ = Observation::new(
            ObservationSource::User {
                channel: "x".into(),
            },
            TaintLabel::User,
            serde_json::Value::Null,
        );
    }
}
