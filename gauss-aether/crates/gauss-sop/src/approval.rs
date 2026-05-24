//! [`ApprovalGate`] — pre-dispatch consent surface.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::engine::SopDef;
use crate::event::TriggerEvent;

/// What an [`ApprovalGate`] decides for one event.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    /// Run the workflow.
    Accept,
    /// Skip the workflow; the engine records the refusal on the
    /// receipt.
    Refuse,
    /// Pause the run. The engine surfaces a `Deferred` outcome; the
    /// caller is expected to re-fire the event once the gate's
    /// underlying state changes (operator approval, etc.). Sprint 14
    /// §2 wires resume into `SessionStore`.
    Defer,
}

/// A gate that decides whether an [`crate::SopDef`]'s workflow runs
/// for a given event.
#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// Decide. Implementations must complete in finite time (or
    /// return [`ApprovalDecision::Defer`]); the engine does not apply
    /// its own timeout in this slice.
    async fn approve(&self, sop: &SopDef, event: &TriggerEvent) -> ApprovalDecision;
}

/// Reference gate that always accepts. Default behaviour for SOPs
/// registered without an explicit gate.
pub struct AlwaysApprove;

#[async_trait]
impl ApprovalGate for AlwaysApprove {
    async fn approve(&self, _sop: &SopDef, _event: &TriggerEvent) -> ApprovalDecision {
        ApprovalDecision::Accept
    }
}

/// Reference gate that always refuses. Useful as a deny-by-default
/// safety net while an operator wires the real approval surface.
pub struct AlwaysRefuse;

#[async_trait]
impl ApprovalGate for AlwaysRefuse {
    async fn approve(&self, _sop: &SopDef, _event: &TriggerEvent) -> ApprovalDecision {
        ApprovalDecision::Refuse
    }
}

/// Reference gate that always defers. Useful for testing the
/// pause path.
pub struct AlwaysDefer;

#[async_trait]
impl ApprovalGate for AlwaysDefer {
    async fn approve(&self, _sop: &SopDef, _event: &TriggerEvent) -> ApprovalDecision {
        ApprovalDecision::Defer
    }
}
