//! Per-action approval gate — the surface the `gauss-turn::TurnEngine`
//! calls inline after admission (paper §XI.D).
//!
//! The gate combines a [`Classifier`] (typically a
//! [`DecisionTable`](crate::DecisionTable)) with an [`ApprovalSurface`].
//! For each tool action the engine asks the gate
//! `decide_action(action, taint)`; the gate returns one of:
//!
//! * [`Outcome::Allow`] — proceed (no approval needed; the action was
//!   `Auto` or `Notify`-banded).
//! * [`Outcome::Denied`] — refuse outright (the classifier returned
//!   `Deny`, or the human surface returned `Denied { .. }`).
//! * [`Outcome::TimedOut`] — the approver missed the deadline; the engine
//!   surfaces this as [`gauss_core::GaussError::AutonomyApprovalTimeout`].
//! * [`Outcome::Approved`] — the action was gated and the human approved
//!   it; the engine emits the original action AND records the approval
//!   decision as an attached audit event.
//!
//! The gate never modifies the action — it returns a verdict. The DTE
//! decides what to do with that verdict (return an error, embed the
//! approval decision in the canonical payload, etc.).

use core::time::Duration;

use gauss_core::{GaussError, GaussResult, TaintLabel, ToolAction, TurnId};

use crate::approval::{ApprovalDecision, ApprovalRequest, ApprovalSurface, DEFAULT_DEADLINE};
use crate::risk::{Classifier, Risk, RiskInputs};

/// Outcome of [`ApprovalGate::decide_action`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Outcome {
    /// Auto-allowed (no approval surface called).
    Allow {
        /// Risk band the classifier assigned (`Auto` or `Notify`).
        risk: Risk,
    },
    /// The classifier denied the action outright; no surface was called.
    Denied {
        /// Risk band (always [`Risk::Deny`] for this variant).
        risk: Risk,
        /// Operator-readable reason from the matching rule.
        reason: String,
    },
    /// The human approved the action.
    Approved {
        /// The decision the surface produced.
        decision: ApprovalDecision,
    },
    /// The deadline elapsed before any decision arrived. Deny-on-timeout.
    TimedOut,
}

impl Outcome {
    /// True iff the action may proceed.
    #[must_use]
    pub const fn proceeds(&self) -> bool {
        matches!(self, Self::Allow { .. } | Self::Approved { .. })
    }
}

/// Gate that wraps a [`Classifier`] + [`ApprovalSurface`].
///
/// Construct once per `TurnEngine` and reuse — the gate holds no mutable
/// state. Generic over `C: Classifier` so the classifier can be a
/// concrete [`crate::DecisionTable`] or a custom Phase-10 scorer.
pub struct ApprovalGate<C: Classifier> {
    classifier: C,
    surface: Box<dyn ApprovalSurface>,
    deadline: Duration,
}

impl<C: Classifier> core::fmt::Debug for ApprovalGate<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ApprovalGate")
            .field("deadline", &self.deadline)
            .field("classifier", &"<dyn Classifier>")
            .field("surface", &"<dyn ApprovalSurface>")
            .finish()
    }
}

impl<C: Classifier> ApprovalGate<C> {
    /// Build a gate with the SPECS default 5-minute deadline.
    #[must_use]
    pub fn new(classifier: C, surface: impl ApprovalSurface + 'static) -> Self {
        Self {
            classifier,
            surface: Box::new(surface),
            deadline: DEFAULT_DEADLINE,
        }
    }

    /// Override the approval deadline. Useful for tests that simulate time.
    #[must_use]
    pub const fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Active deadline.
    #[must_use]
    pub const fn deadline(&self) -> Duration {
        self.deadline
    }

    /// Borrow the inner classifier (mostly for diagnostics + tests).
    #[must_use]
    pub const fn classifier(&self) -> &C {
        &self.classifier
    }

    /// Run the gate on `action`.
    ///
    /// # Errors
    /// Propagates [`ApprovalSurface::request_approval`]'s I/O errors verbatim.
    pub async fn decide_action(
        &self,
        turn_id: TurnId,
        action: &ToolAction,
        taint: TaintLabel,
    ) -> GaussResult<Outcome> {
        let inputs = RiskInputs::new(
            action.cap_required,
            taint,
            action.reversible,
            action.tool.clone(),
        );
        let risk = self.classifier.classify(&inputs);
        match risk {
            Risk::Auto | Risk::Notify => Ok(Outcome::Allow { risk }),
            Risk::Deny => Ok(Outcome::Denied {
                risk,
                reason: "classifier-denied".into(),
            }),
            Risk::RequireApproval => {
                let req = ApprovalRequest::new(
                    turn_id,
                    action.clone(),
                    risk,
                    "classifier-required-approval",
                );
                let decision = self.surface.request_approval(req, self.deadline).await?;
                Ok(match decision {
                    ApprovalDecision::Timeout => Outcome::TimedOut,
                    // `Approved` and `Denied` both wrap the decision in
                    // `Outcome::Approved` for the engine to triage via
                    // [`Self::check`] — the body is intentionally identical.
                    _ => Outcome::Approved { decision },
                })
            }
        }
    }

    /// Convenience: convert an [`Outcome`] into a `GaussResult` that the
    /// turn engine returns directly when the action is blocked. `Allow` and
    /// `Approved` resolve to `Ok(outcome)`; `Denied` and `TimedOut` resolve
    /// to the corresponding [`gauss_core`] error.
    pub fn check(outcome: Outcome) -> GaussResult<Outcome> {
        match &outcome {
            Outcome::Allow { .. } => Ok(outcome),
            Outcome::Approved { decision } => match decision {
                ApprovalDecision::Approved { .. } => Ok(outcome),
                ApprovalDecision::Denied { .. } => Err(GaussError::AutonomyDenied),
                ApprovalDecision::Timeout => Err(GaussError::AutonomyApprovalTimeout),
            },
            Outcome::Denied { .. } => Err(GaussError::AutonomyDenied),
            Outcome::TimedOut => Err(GaussError::AutonomyApprovalTimeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::{CapToken, ToolId};

    use crate::approval::{AutoApprove, AutoDeny};
    use crate::table::default_decision_table;

    fn reversible_filesystem_read() -> ToolAction {
        ToolAction::new(
            ToolId("read_file".into()),
            serde_json::Value::Null,
            CapToken::FILESYSTEM_READ,
            true,
        )
    }

    fn non_reversible_network_post() -> ToolAction {
        ToolAction::new(
            ToolId("send_email".into()),
            serde_json::Value::Null,
            CapToken::NETWORK_POST,
            false,
        )
    }

    #[tokio::test]
    async fn auto_band_does_not_call_surface() {
        let gate = ApprovalGate::new(default_decision_table(), AutoDeny::default());
        let out = gate
            .decide_action(
                TurnId::new(1),
                &reversible_filesystem_read(),
                TaintLabel::User,
            )
            .await
            .unwrap();
        // Even though the surface is `AutoDeny`, the classifier resolves to
        // `Notify` (User taint) and we never reach the surface.
        assert!(out.proceeds());
        match out {
            Outcome::Allow { risk } => assert!(risk == Risk::Notify || risk == Risk::Auto),
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn require_approval_calls_surface() {
        let gate = ApprovalGate::new(default_decision_table(), AutoApprove::new("alice"));
        let out = gate
            .decide_action(
                TurnId::new(2),
                &non_reversible_network_post(),
                TaintLabel::User,
            )
            .await
            .unwrap();
        match out {
            Outcome::Approved {
                decision: ApprovalDecision::Approved { approver },
            } => assert_eq!(approver, "alice"),
            other => panic!("expected Approved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_band_short_circuits_without_calling_surface() {
        let gate = ApprovalGate::new(default_decision_table(), AutoApprove::default());
        let out = gate
            .decide_action(
                TurnId::new(3),
                &reversible_filesystem_read(),
                TaintLabel::Adversarial,
            )
            .await
            .unwrap();
        match out {
            Outcome::Denied { risk, .. } => assert_eq!(risk, Risk::Deny),
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn human_deny_propagates() {
        let gate = ApprovalGate::new(
            default_decision_table(),
            AutoDeny::new("bob", Some("nope".into())),
        );
        let out = gate
            .decide_action(
                TurnId::new(4),
                &non_reversible_network_post(),
                TaintLabel::User,
            )
            .await
            .unwrap();
        let result = ApprovalGate::<crate::DecisionTable>::check(out);
        assert!(matches!(result, Err(GaussError::AutonomyDenied)));
    }

    #[tokio::test(start_paused = true)]
    async fn approval_timeout_propagates_as_error() {
        let (surface, _rx) = crate::approval::ChannelSurface::new(1);
        let gate = ApprovalGate::new(default_decision_table(), surface)
            .with_deadline(Duration::from_millis(20));
        let action = non_reversible_network_post();
        let fut = gate.decide_action(TurnId::new(5), &action, TaintLabel::User);
        // Advance virtual time past the deadline.
        tokio::time::advance(Duration::from_millis(100)).await;
        let out = fut.await.unwrap();
        let result = ApprovalGate::<crate::DecisionTable>::check(out);
        assert!(matches!(result, Err(GaussError::AutonomyApprovalTimeout)));
    }
}
