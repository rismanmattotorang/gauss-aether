//! [`SopEngine`] — registry + dispatcher.

use std::collections::HashMap;
use std::sync::Arc;

use gauss_core::CapToken;
use serde::{Deserialize, Serialize};

use crate::approval::{AlwaysApprove, ApprovalDecision, ApprovalGate};
use crate::cancel::CancelHandle;
use crate::error::SopError;
use crate::event::TriggerEvent;
use crate::receipt::{RunStatus, SopRunReceipt};
use crate::trigger::Trigger;
use crate::workflow::{Workflow, WorkflowCtx};

/// Stable identifier for an SOP definition. Operator-chosen at
/// registration; the engine uses it as a primary key.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SopId(String);

impl SopId {
    /// Build from an owned string.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// Borrow the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identifier for a workflow. Workflows declare their own id;
/// the engine carries it on receipts.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowId(String);

impl WorkflowId {
    /// Build from an owned string.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// Borrow the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One registered SOP. The engine holds these by [`SopId`].
pub struct SopDef {
    /// Stable identifier.
    pub id: SopId,
    /// Event source. Boxed because triggers are heterogeneous and the
    /// engine polls them via dynamic dispatch.
    pub trigger: Box<dyn Trigger>,
    /// Workflow to run on each event.
    pub workflow: Arc<dyn Workflow>,
    /// Optional approval gate. `None` is equivalent to
    /// [`AlwaysApprove`].
    pub gate: Option<Arc<dyn ApprovalGate>>,
    /// Caps the operator declared the SOP needs. The engine's
    /// [`SopEngine::register`] check uses this; per-dispatch checks
    /// also re-validate the workflow's `required_caps()` so an
    /// operator can't widen privileges by lying in the registration.
    pub caps: CapToken,
}

impl SopDef {
    /// Build with no approval gate (defaults to accept).
    pub fn new(
        id: SopId,
        trigger: Box<dyn Trigger>,
        workflow: Arc<dyn Workflow>,
        caps: CapToken,
    ) -> Self {
        Self {
            id,
            trigger,
            workflow,
            gate: None,
            caps,
        }
    }

    /// Builder: install an approval gate.
    #[must_use]
    pub fn with_gate(mut self, gate: Arc<dyn ApprovalGate>) -> Self {
        self.gate = Some(gate);
        self
    }
}

/// Registry + dispatcher.
///
/// Construct with [`Self::new`]; register SOPs against a live
/// `CapToken` grant; drive runs with [`Self::run_once`] (one event)
/// or [`Self::run_until_empty`] (drain the trigger queue).
pub struct SopEngine {
    /// Owning store of registered SOPs.
    sops: HashMap<SopId, SopDef>,
}

impl SopEngine {
    /// Fresh, empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sops: HashMap::new(),
        }
    }

    /// Register an SOP against the live grant. Refuses closed if
    /// `grant` doesn't satisfy `sop.caps` or if the workflow's own
    /// `required_caps()` aren't a subset of `sop.caps` (defence in
    /// depth: an operator can't smuggle privileges past the cap
    /// declaration). Refuses on duplicate id.
    ///
    /// # Errors
    /// - [`SopError::AdmitRefused`] when `grant` is missing bits.
    /// - [`SopError::Duplicate`] when `sop.id` already exists.
    pub fn register(&mut self, sop: SopDef, grant: CapToken) -> Result<(), SopError> {
        // SOP-level cap check.
        if !grant.contains(sop.caps) {
            return Err(SopError::AdmitRefused {
                required: sop.caps.bits(),
                grant: grant.bits(),
            });
        }
        // Workflow's declared caps must be ⊆ SOP's declared caps.
        let wf_caps = sop.workflow.required_caps();
        if !sop.caps.contains(wf_caps) {
            return Err(SopError::AdmitRefused {
                required: wf_caps.bits(),
                grant: sop.caps.bits(),
            });
        }
        if self.sops.contains_key(&sop.id) {
            return Err(SopError::Duplicate(sop.id.as_str().to_string()));
        }
        self.sops.insert(sop.id.clone(), sop);
        Ok(())
    }

    /// Number of registered SOPs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sops.len()
    }

    /// True iff no SOPs are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sops.is_empty()
    }

    /// Iterate registered SOP ids in insertion order is *not*
    /// guaranteed — callers that need ordering should sort.
    pub fn ids(&self) -> impl Iterator<Item = &SopId> {
        self.sops.keys()
    }

    /// Poll one event from the named SOP's trigger and dispatch it
    /// through the gate + workflow. Returns the receipt of the
    /// dispatch, or `Ok(None)` if the trigger had no event ready.
    ///
    /// # Errors
    /// - [`SopError::NotFound`] if no SOP is registered under
    ///   `sop_id`.
    pub async fn run_once(
        &mut self,
        sop_id: &SopId,
        grant: CapToken,
        cancel: CancelHandle,
    ) -> Result<Option<SopRunReceipt>, SopError> {
        let sop = self
            .sops
            .get_mut(sop_id)
            .ok_or_else(|| SopError::NotFound(sop_id.as_str().to_string()))?;
        let Some(event) = sop.trigger.next(cancel).await else {
            return Ok(None);
        };
        let receipt = dispatch_one(sop, &event, grant).await;
        Ok(Some(receipt))
    }

    /// Drain the named SOP's trigger queue, dispatching every event.
    /// Stops on the first `None` from the trigger or when `cancel`
    /// flips. Returns the receipts in fire order.
    ///
    /// # Errors
    /// - [`SopError::NotFound`] if no SOP is registered under
    ///   `sop_id`.
    pub async fn run_until_empty(
        &mut self,
        sop_id: &SopId,
        grant: CapToken,
        cancel: CancelHandle,
    ) -> Result<Vec<SopRunReceipt>, SopError> {
        let mut out = Vec::new();
        loop {
            if cancel.is_cancelled() {
                break;
            }
            match self.run_once(sop_id, grant, cancel.clone()).await? {
                Some(r) => out.push(r),
                None => break,
            }
        }
        Ok(out)
    }

    /// Dispatch one explicit, operator-supplied event against the
    /// named SOP — bypassing the trigger. Models the `gaussclaw sop
    /// run --event-json` CLI verb.
    ///
    /// # Errors
    /// - [`SopError::NotFound`] if no SOP is registered under
    ///   `sop_id`.
    pub async fn fire_manual(
        &mut self,
        sop_id: &SopId,
        event: TriggerEvent,
        grant: CapToken,
    ) -> Result<SopRunReceipt, SopError> {
        let sop = self
            .sops
            .get_mut(sop_id)
            .ok_or_else(|| SopError::NotFound(sop_id.as_str().to_string()))?;
        Ok(dispatch_one(sop, &event, grant).await)
    }
}

impl Default for SopEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply gate + cap-recheck + workflow execution. Always produces a
/// receipt; refusals are recorded, not thrown.
async fn dispatch_one(sop: &SopDef, event: &TriggerEvent, grant: CapToken) -> SopRunReceipt {
    let trigger_event_digest = event.digest();
    let chain_head_before = [0u8; 32];
    let chain_head_after = [0u8; 32];

    // 1. Cap-gate at dispatch time. The SOP's declared caps might be
    //    a superset of the workflow's; we check the *workflow's*
    //    requirement against the *live* grant, which is the stricter
    //    of the two policies.
    let required = sop.workflow.required_caps();
    if !grant.contains(required) {
        return SopRunReceipt {
            sop_id: sop.id.clone(),
            workflow_id: sop.workflow.id(),
            trigger_event_digest,
            status: RunStatus::AdmitRefused {
                required: required.bits(),
                grant: grant.bits(),
            },
            chain_head_before,
            chain_head_after,
            signature: None,
        };
    }

    // 2. Approval gate. `None` => `AlwaysApprove`.
    let default_gate = AlwaysApprove;
    let gate: &dyn ApprovalGate = sop
        .gate
        .as_deref()
        .map_or(&default_gate as &dyn ApprovalGate, |g| g);
    let decision = gate.approve(sop, event).await;
    let status = match decision {
        ApprovalDecision::Refuse => RunStatus::Refused,
        ApprovalDecision::Defer => RunStatus::Deferred,
        ApprovalDecision::Accept => {
            let mut ctx = WorkflowCtx::new(grant, event.adversarial);
            match sop.workflow.execute(&mut ctx, event).await {
                Ok(outcome) => RunStatus::Completed { outcome },
                Err(e) => RunStatus::Failed {
                    reason: e.to_string(),
                },
            }
        }
    };

    SopRunReceipt {
        sop_id: sop.id.clone(),
        workflow_id: sop.workflow.id(),
        trigger_event_digest,
        status,
        chain_head_before,
        chain_head_after,
        signature: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{AlwaysDefer, AlwaysRefuse};
    use crate::trigger::MemoryTrigger;
    use crate::workflow::NoopWorkflow;
    use async_trait::async_trait;

    fn sop_with_noop(id: &str, caps: CapToken, workflow_caps: CapToken) -> SopDef {
        let trigger = MemoryTrigger::new(
            id,
            [TriggerEvent::new(id, serde_json::json!({ "n": 1 }))],
        );
        let workflow = Arc::new(NoopWorkflow::with_required(
            WorkflowId::new(format!("{id}-wf")),
            workflow_caps,
        ));
        SopDef::new(SopId::new(id), Box::new(trigger), workflow, caps)
    }

    #[tokio::test]
    async fn register_succeeds_when_grant_covers_caps() {
        let mut engine = SopEngine::new();
        let sop = sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM);
        engine
            .register(sop, CapToken::SOP_DEFINE | CapToken::SOP_TRIGGER)
            .unwrap();
        assert_eq!(engine.len(), 1);
    }

    #[tokio::test]
    async fn register_refuses_when_grant_misses_caps() {
        let mut engine = SopEngine::new();
        let sop = sop_with_noop("alpha", CapToken::NETWORK_GET, CapToken::BOTTOM);
        let err = engine.register(sop, CapToken::BOTTOM).unwrap_err();
        assert!(matches!(err, SopError::AdmitRefused { .. }));
        assert!(engine.is_empty());
    }

    #[tokio::test]
    async fn register_refuses_when_workflow_caps_exceed_sop_caps() {
        let mut engine = SopEngine::new();
        // SOP declares only SOP_DEFINE, but workflow demands NETWORK_GET.
        let sop = sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::NETWORK_GET);
        let err = engine.register(sop, CapToken::TOP).unwrap_err();
        match err {
            SopError::AdmitRefused { required, grant } => {
                assert_eq!(required, CapToken::NETWORK_GET.bits());
                assert_eq!(grant, CapToken::SOP_DEFINE.bits());
            }
            other => panic!("expected AdmitRefused, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_refuses_duplicate_id() {
        let mut engine = SopEngine::new();
        engine
            .register(
                sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM),
                CapToken::TOP,
            )
            .unwrap();
        let err = engine
            .register(
                sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM),
                CapToken::TOP,
            )
            .unwrap_err();
        assert!(matches!(err, SopError::Duplicate(_)));
    }

    #[tokio::test]
    async fn run_once_dispatches_one_event_and_records_outcome() {
        let mut engine = SopEngine::new();
        engine
            .register(
                sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM),
                CapToken::TOP,
            )
            .unwrap();
        let receipt = engine
            .run_once(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap()
            .expect("event was queued");
        match receipt.status {
            RunStatus::Completed { outcome } => {
                assert_eq!(outcome.output, serde_json::json!({ "n": 1 }));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(receipt.sop_id.as_str(), "alpha");
        assert_eq!(receipt.workflow_id.as_str(), "alpha-wf");
    }

    #[tokio::test]
    async fn run_once_returns_none_when_trigger_drained() {
        let mut engine = SopEngine::new();
        engine
            .register(
                sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM),
                CapToken::TOP,
            )
            .unwrap();
        // First call drains the single queued event.
        let _ = engine
            .run_once(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap();
        // Second call sees an empty queue.
        let r = engine
            .run_once(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap();
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn run_once_refuses_when_workflow_caps_exceed_live_grant() {
        let mut engine = SopEngine::new();
        engine
            .register(
                sop_with_noop(
                    "alpha",
                    CapToken::SOP_DEFINE | CapToken::NETWORK_GET,
                    CapToken::NETWORK_GET,
                ),
                CapToken::TOP,
            )
            .unwrap();
        // Live grant is *missing* NETWORK_GET — dispatch must refuse.
        let receipt = engine
            .run_once(
                &SopId::new("alpha"),
                CapToken::SOP_DEFINE,
                CancelHandle::new(),
            )
            .await
            .unwrap()
            .expect("event queued");
        match receipt.status {
            RunStatus::AdmitRefused { required, grant } => {
                assert_eq!(required, CapToken::NETWORK_GET.bits());
                assert_eq!(grant, CapToken::SOP_DEFINE.bits());
            }
            other => panic!("expected AdmitRefused, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn approval_gate_accept_runs_workflow() {
        let mut engine = SopEngine::new();
        let sop = sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM)
            .with_gate(Arc::new(AlwaysApprove));
        engine.register(sop, CapToken::TOP).unwrap();
        let receipt = engine
            .run_once(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(receipt.status, RunStatus::Completed { .. }));
    }

    #[tokio::test]
    async fn approval_gate_refuse_skips_workflow() {
        let mut engine = SopEngine::new();
        let sop = sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM)
            .with_gate(Arc::new(AlwaysRefuse));
        engine.register(sop, CapToken::TOP).unwrap();
        let receipt = engine
            .run_once(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(receipt.status, RunStatus::Refused));
    }

    #[tokio::test]
    async fn approval_gate_defer_pauses_workflow() {
        let mut engine = SopEngine::new();
        let sop = sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM)
            .with_gate(Arc::new(AlwaysDefer));
        engine.register(sop, CapToken::TOP).unwrap();
        let receipt = engine
            .run_once(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(receipt.status, RunStatus::Deferred));
    }

    #[tokio::test]
    async fn fire_manual_dispatches_supplied_event() {
        let mut engine = SopEngine::new();
        engine
            .register(
                sop_with_noop("alpha", CapToken::SOP_DEFINE, CapToken::BOTTOM),
                CapToken::TOP,
            )
            .unwrap();
        let event = TriggerEvent::new("manual", serde_json::json!({ "synthetic": true }));
        let receipt = engine
            .fire_manual(&SopId::new("alpha"), event, CapToken::TOP)
            .await
            .unwrap();
        match receipt.status {
            RunStatus::Completed { outcome } => {
                assert_eq!(outcome.output, serde_json::json!({ "synthetic": true }));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_until_empty_drains_multi_event_queue_in_order() {
        let mut engine = SopEngine::new();
        let trigger = MemoryTrigger::new(
            "alpha",
            [
                TriggerEvent::new("alpha", serde_json::json!({ "n": 1 })),
                TriggerEvent::new("alpha", serde_json::json!({ "n": 2 })),
                TriggerEvent::new("alpha", serde_json::json!({ "n": 3 })),
            ],
        );
        let workflow = Arc::new(NoopWorkflow::new(WorkflowId::new("alpha-wf")));
        let sop = SopDef::new(
            SopId::new("alpha"),
            Box::new(trigger),
            workflow,
            CapToken::SOP_DEFINE,
        );
        engine.register(sop, CapToken::TOP).unwrap();
        let receipts = engine
            .run_until_empty(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap();
        assert_eq!(receipts.len(), 3);
        for (i, r) in receipts.iter().enumerate() {
            let expected = serde_json::json!({ "n": i as u64 + 1 });
            match &r.status {
                RunStatus::Completed { outcome } => assert_eq!(outcome.output, expected),
                other => panic!("expected Completed at {i}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn run_until_empty_stops_when_cancel_flips() {
        let mut engine = SopEngine::new();
        let trigger = MemoryTrigger::new(
            "alpha",
            [
                TriggerEvent::new("alpha", serde_json::json!({ "n": 1 })),
                TriggerEvent::new("alpha", serde_json::json!({ "n": 2 })),
                TriggerEvent::new("alpha", serde_json::json!({ "n": 3 })),
            ],
        );
        let workflow = Arc::new(NoopWorkflow::new(WorkflowId::new("alpha-wf")));
        let sop = SopDef::new(
            SopId::new("alpha"),
            Box::new(trigger),
            workflow,
            CapToken::SOP_DEFINE,
        );
        engine.register(sop, CapToken::TOP).unwrap();
        let cancel = CancelHandle::new();
        cancel.request_cancel();
        let receipts = engine
            .run_until_empty(&SopId::new("alpha"), CapToken::TOP, cancel)
            .await
            .unwrap();
        assert!(receipts.is_empty());
    }

    #[tokio::test]
    async fn run_once_returns_not_found_for_unknown_sop() {
        let mut engine = SopEngine::new();
        let err = engine
            .run_once(&SopId::new("missing"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap_err();
        assert!(matches!(err, SopError::NotFound(_)));
    }

    /// Workflow that always returns an internal error — exercises
    /// the [`RunStatus::Failed`] path.
    struct FailingWorkflow {
        id: WorkflowId,
    }

    #[async_trait]
    impl Workflow for FailingWorkflow {
        fn id(&self) -> WorkflowId {
            self.id.clone()
        }
        fn required_caps(&self) -> CapToken {
            CapToken::BOTTOM
        }
        async fn execute(
            &self,
            _ctx: &mut WorkflowCtx,
            _event: &TriggerEvent,
        ) -> Result<crate::WorkflowOutcome, SopError> {
            Err(SopError::Workflow("synthetic failure".into()))
        }
    }

    #[tokio::test]
    async fn workflow_error_lands_as_failed_status() {
        let mut engine = SopEngine::new();
        let trigger = MemoryTrigger::new(
            "alpha",
            [TriggerEvent::new("alpha", serde_json::Value::Null)],
        );
        let workflow = Arc::new(FailingWorkflow {
            id: WorkflowId::new("fail"),
        });
        let sop = SopDef::new(
            SopId::new("alpha"),
            Box::new(trigger),
            workflow,
            CapToken::SOP_DEFINE,
        );
        engine.register(sop, CapToken::TOP).unwrap();
        let receipt = engine
            .run_once(&SopId::new("alpha"), CapToken::TOP, CancelHandle::new())
            .await
            .unwrap()
            .unwrap();
        match receipt.status {
            RunStatus::Failed { reason } => assert!(reason.contains("synthetic failure")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
