//! [`Workflow`] — the engine's dispatch trait.

use async_trait::async_trait;
use gauss_core::CapToken;
use serde::{Deserialize, Serialize};

use crate::engine::WorkflowId;
use crate::error::SopError;
use crate::event::TriggerEvent;

/// Per-execution scratchpad. Carries the live [`CapToken`] grant so a
/// workflow that wants to fan out into sub-steps can re-check caps
/// rather than trusting the engine's gate transitively.
#[non_exhaustive]
pub struct WorkflowCtx {
    /// The live kernel grant at the moment of dispatch.
    pub grant: CapToken,
    /// Whether the trigger source was untrusted. Workflows that touch
    /// cap-sensitive resources (e.g. `MEMORY_READ`) should refuse
    /// when `adversarial == true`.
    pub adversarial: bool,
}

impl WorkflowCtx {
    /// Build a context from the engine.
    #[must_use]
    pub const fn new(grant: CapToken, adversarial: bool) -> Self {
        Self { grant, adversarial }
    }
}

/// The result of a successful [`Workflow::execute`] call.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct WorkflowOutcome {
    /// Free-form JSON the engine records on the receipt. Workflows
    /// surface user-visible results here.
    pub output: serde_json::Value,
    /// Optional list of receipt ids the workflow produced internally
    /// (for example, by dispatching tool calls through HWCA). These
    /// chain into the [`crate::SopRunReceipt`].
    pub child_receipts: Vec<u64>,
}

impl WorkflowOutcome {
    /// Build an outcome with no child receipts.
    pub fn from_output(output: serde_json::Value) -> Self {
        Self {
            output,
            child_receipts: Vec::new(),
        }
    }

    /// Empty outcome — the workflow ran but produced no observable
    /// result. Useful for fire-and-forget side effects.
    #[must_use]
    pub fn empty() -> Self {
        Self::from_output(serde_json::Value::Null)
    }
}

/// A workflow the engine dispatches against an event.
#[async_trait]
pub trait Workflow: Send + Sync {
    /// Stable identifier the engine records on the receipt.
    fn id(&self) -> WorkflowId;

    /// Caps the workflow needs to run. The engine re-checks these
    /// against the live grant at dispatch time; a workflow whose caps
    /// the grant doesn't satisfy is refused with
    /// [`SopError::AdmitRefused`].
    fn required_caps(&self) -> CapToken;

    /// Run the workflow against an event. Workflows must not assume
    /// the cap-gate has been applied to their *internal* tool calls
    /// — they're responsible for routing through HWCA themselves.
    async fn execute(
        &self,
        ctx: &mut WorkflowCtx,
        event: &TriggerEvent,
    ) -> Result<WorkflowOutcome, SopError>;
}

/// Reference workflow that echoes the event payload into its outcome.
/// Useful for engine-level tests that need a deterministic, side-
/// effect-free workflow.
pub struct NoopWorkflow {
    id: WorkflowId,
    required_caps: CapToken,
}

impl NoopWorkflow {
    /// Build with the default `CapToken::BOTTOM` requirement (runs
    /// under any grant).
    pub fn new(id: WorkflowId) -> Self {
        Self {
            id,
            required_caps: CapToken::BOTTOM,
        }
    }

    /// Build with an explicit cap requirement.
    pub fn with_required(id: WorkflowId, required_caps: CapToken) -> Self {
        Self { id, required_caps }
    }
}

#[async_trait]
impl Workflow for NoopWorkflow {
    fn id(&self) -> WorkflowId {
        self.id.clone()
    }

    fn required_caps(&self) -> CapToken {
        self.required_caps
    }

    async fn execute(
        &self,
        _ctx: &mut WorkflowCtx,
        event: &TriggerEvent,
    ) -> Result<WorkflowOutcome, SopError> {
        Ok(WorkflowOutcome::from_output(event.payload.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_workflow_echoes_payload() {
        let wf = NoopWorkflow::new(WorkflowId::new("echo"));
        let mut ctx = WorkflowCtx::new(CapToken::TOP, false);
        let event = TriggerEvent::new("mem", serde_json::json!({ "x": 42 }));
        let outcome = wf.execute(&mut ctx, &event).await.unwrap();
        assert_eq!(outcome.output, serde_json::json!({ "x": 42 }));
        assert!(outcome.child_receipts.is_empty());
    }

    #[test]
    fn workflow_outcome_empty_is_null() {
        let o = WorkflowOutcome::empty();
        assert_eq!(o.output, serde_json::Value::Null);
        assert!(o.child_receipts.is_empty());
    }
}
