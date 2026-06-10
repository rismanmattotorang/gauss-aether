//! Approval queue + surface trait (paper §XI.C).
//!
//! A surface is the channel that asks the human for a decision. Phase 7
//! ships three deterministic test surfaces:
//!
//! * [`AutoApprove`] — every request approves immediately.
//! * [`AutoDeny`] — every request denies immediately.
//! * [`ChannelSurface`] — `tokio::sync::mpsc`-driven; tests push decisions
//!   in via the receiver clone.
//!
//! Production adapters (Telegram, Slack, Discord, Matrix, CLI/TUI, SSE)
//! ship in Phase 9 as additive impls — the trait surface here is the
//! stable contract.

use core::time::Duration;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolAction, TurnId};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;

use crate::risk::Risk;

/// Paper §XI.C deadline: 5 minutes (300 s) between the request and the
/// approver's decision; missing the deadline triggers
/// [`ApprovalDecision::Timeout`].
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(300);

/// A pending approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ApprovalRequest {
    /// Originating turn identifier.
    pub turn_id: TurnId,
    /// The action awaiting approval.
    pub action: ToolAction,
    /// Risk band the classifier assigned (always
    /// [`Risk::RequireApproval`] for live requests; [`Risk::Notify`] is
    /// surfaced as an asynchronous notification, not a blocking request).
    pub risk: Risk,
    /// Operator-readable reason / label from the matching rule.
    pub reason: String,
}

impl ApprovalRequest {
    /// Build a request.
    #[must_use]
    pub fn new(turn_id: TurnId, action: ToolAction, risk: Risk, reason: impl Into<String>) -> Self {
        Self {
            turn_id,
            action,
            risk,
            reason: reason.into(),
        }
    }
}

/// The user's decision (or the timer's verdict).
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "outcome")]
#[non_exhaustive]
pub enum ApprovalDecision {
    /// Approver said yes.
    Approved {
        /// Identifier of the human / system that approved.
        approver: String,
    },
    /// Approver said no.
    Denied {
        /// Identifier of the human / system that denied.
        approver: String,
        /// Optional operator-readable rationale.
        reason: Option<String>,
    },
    /// Deadline elapsed before any decision arrived. Deny-on-timeout per
    /// paper §XI.C.
    Timeout,
}

impl ApprovalDecision {
    /// True iff the action may proceed.
    #[must_use]
    pub const fn proceeds(&self) -> bool {
        matches!(self, Self::Approved { .. })
    }
}

/// Surface that requests a human decision.
///
/// Implementations are `async` and have an internal deadline plumbed through
/// `request_approval`'s `deadline` argument. The Phase-9 production surfaces
/// (Telegram et al.) implement this trait directly.
#[async_trait]
pub trait ApprovalSurface: Send + Sync {
    /// Submit `request` and await the decision, returning
    /// [`ApprovalDecision::Timeout`] if `deadline` elapses first.
    ///
    /// # Errors
    /// Surface-side transport / queue errors are wrapped in
    /// [`gauss_core::GaussError::Io`].
    async fn request_approval(
        &self,
        request: ApprovalRequest,
        deadline: Duration,
    ) -> GaussResult<ApprovalDecision>;
}

/// Test surface that approves every request immediately.
#[derive(Debug, Clone, Default)]
pub struct AutoApprove {
    /// Approver identity stamped onto the decision.
    pub approver: String,
}

impl AutoApprove {
    /// Build with a custom approver identity.
    #[must_use]
    pub fn new(approver: impl Into<String>) -> Self {
        Self {
            approver: approver.into(),
        }
    }
}

#[async_trait]
impl ApprovalSurface for AutoApprove {
    async fn request_approval(
        &self,
        _request: ApprovalRequest,
        _deadline: Duration,
    ) -> GaussResult<ApprovalDecision> {
        Ok(ApprovalDecision::Approved {
            approver: if self.approver.is_empty() {
                "auto".to_owned()
            } else {
                self.approver.clone()
            },
        })
    }
}

/// Test surface that denies every request immediately.
#[derive(Debug, Clone, Default)]
pub struct AutoDeny {
    /// Approver identity stamped onto the denial.
    pub approver: String,
    /// Optional rationale.
    pub reason: Option<String>,
}

impl AutoDeny {
    /// Build with a custom approver identity + reason.
    #[must_use]
    pub fn new(approver: impl Into<String>, reason: Option<String>) -> Self {
        Self {
            approver: approver.into(),
            reason,
        }
    }
}

#[async_trait]
impl ApprovalSurface for AutoDeny {
    async fn request_approval(
        &self,
        _request: ApprovalRequest,
        _deadline: Duration,
    ) -> GaussResult<ApprovalDecision> {
        Ok(ApprovalDecision::Denied {
            approver: if self.approver.is_empty() {
                "auto".to_owned()
            } else {
                self.approver.clone()
            },
            reason: self.reason.clone(),
        })
    }
}

/// A surface driven by an `mpsc` channel — used by tests and the in-process
/// CLI prompt. The caller pushes one decision per request via the cloned
/// [`ChannelSurface::sender`] handle.
#[derive(Debug, Clone)]
pub struct ChannelSurface {
    inbox: Arc<Mutex<mpsc::Receiver<ApprovalDecision>>>,
    outbox: mpsc::Sender<ApprovalRequest>,
    decision_sender: mpsc::Sender<ApprovalDecision>,
}

impl ChannelSurface {
    /// Build a channel surface with the given buffer size for both queues.
    #[must_use]
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<ApprovalRequest>) {
        let (req_tx, req_rx) = mpsc::channel(capacity);
        let (dec_tx, dec_rx) = mpsc::channel(capacity);
        let surface = Self {
            inbox: Arc::new(Mutex::new(dec_rx)),
            outbox: req_tx,
            decision_sender: dec_tx,
        };
        (surface, req_rx)
    }

    /// Sender handle to push decisions back into the surface.
    #[must_use]
    pub fn sender(&self) -> mpsc::Sender<ApprovalDecision> {
        self.decision_sender.clone()
    }
}

#[async_trait]
impl ApprovalSurface for ChannelSurface {
    async fn request_approval(
        &self,
        request: ApprovalRequest,
        deadline: Duration,
    ) -> GaussResult<ApprovalDecision> {
        // The deadline covers the whole round-trip. A bounded `send`
        // awaits queue capacity, so a stuffed queue (approver not
        // draining) must degrade to the same fail-closed `Timeout` as
        // an unanswered request — not block the turn indefinitely.
        let started = std::time::Instant::now();
        match timeout(deadline, self.outbox.send(request)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(GaussError::Io(format!("approval queue closed: {e}"))),
            Err(_) => return Ok(ApprovalDecision::Timeout),
        }
        // Drain one decision from the inbox under the remaining budget.
        let remaining = deadline.saturating_sub(started.elapsed());
        let mut inbox = self.inbox.lock().await;
        match timeout(remaining, inbox.recv()).await {
            Ok(Some(decision)) => Ok(decision),
            Ok(None) => Err(GaussError::Io("approval inbox channel closed".into())),
            Err(_) => Ok(ApprovalDecision::Timeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::{CapToken, ToolAction, ToolId};

    fn dummy_action() -> ToolAction {
        ToolAction::new(
            ToolId("dummy".into()),
            serde_json::Value::Null,
            CapToken::FILESYSTEM_READ,
            true,
        )
    }

    #[tokio::test]
    async fn auto_approve_proceeds() {
        let s = AutoApprove::new("operator");
        let req = ApprovalRequest::new(
            TurnId::new(1),
            dummy_action(),
            Risk::RequireApproval,
            "test",
        );
        let d = s
            .request_approval(req, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(d.proceeds());
        match d {
            ApprovalDecision::Approved { approver } => assert_eq!(approver, "operator"),
            _ => panic!("expected Approved"),
        }
    }

    #[tokio::test]
    async fn auto_deny_does_not_proceed() {
        let s = AutoDeny::new("operator", Some("policy".into()));
        let req = ApprovalRequest::new(
            TurnId::new(2),
            dummy_action(),
            Risk::RequireApproval,
            "test",
        );
        let d = s
            .request_approval(req, Duration::from_secs(1))
            .await
            .unwrap();
        assert!(!d.proceeds());
    }

    #[tokio::test]
    async fn channel_surface_delivers_decision() {
        let (surface, mut req_rx) = ChannelSurface::new(4);
        let sender = surface.sender();
        // Pretend operator: receive the request, push a decision back.
        let pretend = tokio::spawn(async move {
            let _ = req_rx.recv().await.unwrap();
            sender
                .send(ApprovalDecision::Approved {
                    approver: "alice".into(),
                })
                .await
                .unwrap();
        });
        let req = ApprovalRequest::new(
            TurnId::new(3),
            dummy_action(),
            Risk::RequireApproval,
            "test",
        );
        let d = surface
            .request_approval(req, Duration::from_secs(1))
            .await
            .unwrap();
        pretend.await.unwrap();
        assert!(d.proceeds());
    }

    #[tokio::test(start_paused = true)]
    async fn channel_surface_times_out() {
        let (surface, _req_rx) = ChannelSurface::new(4);
        let req = ApprovalRequest::new(
            TurnId::new(4),
            dummy_action(),
            Risk::RequireApproval,
            "test",
        );
        let fut = surface.request_approval(req, Duration::from_millis(50));
        // Advance virtual time past the deadline.
        tokio::time::advance(Duration::from_millis(100)).await;
        let d = fut.await.unwrap();
        assert_eq!(d, ApprovalDecision::Timeout);
    }

    #[tokio::test(start_paused = true)]
    async fn channel_surface_full_queue_times_out_instead_of_blocking() {
        // Capacity-1 surface with no approver draining: the first
        // request occupies the queue, so the second one's `send` can
        // never complete. The deadline must fail it closed (Timeout)
        // rather than letting the turn block forever.
        let (surface, _req_rx) = ChannelSurface::new(1);
        let stuffing = ApprovalRequest::new(
            TurnId::new(7),
            dummy_action(),
            Risk::RequireApproval,
            "stuffing",
        );
        surface.outbox.send(stuffing).await.unwrap();

        let req = ApprovalRequest::new(
            TurnId::new(8),
            dummy_action(),
            Risk::RequireApproval,
            "blocked",
        );
        let fut = surface.request_approval(req, Duration::from_millis(50));
        tokio::time::advance(Duration::from_millis(100)).await;
        let d = fut.await.unwrap();
        assert_eq!(d, ApprovalDecision::Timeout);
    }

    #[test]
    fn decision_serde_round_trips() {
        let a = ApprovalDecision::Approved {
            approver: "alice".into(),
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: ApprovalDecision = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }
}
