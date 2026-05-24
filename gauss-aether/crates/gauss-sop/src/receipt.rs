//! [`SopRunReceipt`] — one record per dispatch.
//!
//! Sprint 14 §1 ships the receipt shape and a stable digest; Sprint
//! 14 §2 chains receipts into the existing `gauss-audit` Merkle log,
//! at which point [`Self::chain_head_before`] and
//! [`Self::chain_head_after`] become real chain links rather than
//! caller-supplied placeholders. The digest layer is stable now.

use blake3::Hasher;
use serde::{Deserialize, Serialize};

use crate::engine::{SopId, WorkflowId};
use crate::workflow::WorkflowOutcome;

/// One terminal state of an SOP run.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunStatus {
    /// Workflow ran to completion and returned an outcome.
    Completed {
        /// The workflow's terminal outcome.
        outcome: WorkflowOutcome,
    },
    /// Approval gate refused; no workflow ran.
    Refused,
    /// Approval gate deferred; the engine paused. A follow-on event
    /// fire is required to resume.
    Deferred,
    /// Cap-gate refused at dispatch time. The workflow did not run.
    AdmitRefused {
        /// Cap bits the workflow required.
        required: u64,
        /// Cap bits the live grant exposed.
        grant: u64,
    },
    /// Workflow ran but returned an error.
    Failed {
        /// Human-readable diagnostic from the workflow.
        reason: String,
    },
}

/// One run's receipt. Carries every input the engine observed and
/// the terminal status, in a shape that BLAKE3's over canonical
/// bytes for tamper-evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SopRunReceipt {
    /// Which SOP was dispatched.
    pub sop_id: SopId,
    /// Which workflow ran (or would have run, if refused).
    pub workflow_id: WorkflowId,
    /// 32-byte BLAKE3 digest of the input [`crate::TriggerEvent`].
    pub trigger_event_digest: [u8; 32],
    /// Terminal status.
    pub status: RunStatus,
    /// Chain head observed before the run started. Placeholder
    /// `[0; 32]` until Sprint 14 §2 wires this into `gauss-audit`.
    pub chain_head_before: [u8; 32],
    /// Chain head after the receipt is appended. Placeholder
    /// `[0; 32]` until Sprint 14 §2.
    pub chain_head_after: [u8; 32],
    /// Optional Ed25519 signature. `None` until Sprint 14 §2 wires
    /// a [`gauss_audit`] `ReceiptSigner`.
    pub signature: Option<Vec<u8>>,
}

impl SopRunReceipt {
    /// BLAKE3 over the canonical receipt bytes:
    ///
    /// ```text
    /// sop_id.as_bytes()
    ///  || 0x00
    ///  || workflow_id.as_bytes()
    ///  || 0x00
    ///  || trigger_event_digest
    ///  || 0x00
    ///  || status_canonical_json
    ///  || 0x00
    ///  || chain_head_before
    ///  || chain_head_after
    /// ```
    ///
    /// Signature is *not* part of the digest — a receipt re-signed
    /// with a new key keeps the same identifier.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut h = Hasher::new();
        h.update(self.sop_id.as_str().as_bytes());
        h.update(&[0u8]);
        h.update(self.workflow_id.as_str().as_bytes());
        h.update(&[0u8]);
        h.update(&self.trigger_event_digest);
        h.update(&[0u8]);
        let status_bytes = serde_json::to_vec(&self.status).unwrap_or_default();
        h.update(&status_bytes);
        h.update(&[0u8]);
        h.update(&self.chain_head_before);
        h.update(&self.chain_head_after);
        *h.finalize().as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SopId, WorkflowId};
    use crate::workflow::WorkflowOutcome;

    fn sample_receipt() -> SopRunReceipt {
        SopRunReceipt {
            sop_id: SopId::new("alpha"),
            workflow_id: WorkflowId::new("echo"),
            trigger_event_digest: [1; 32],
            status: RunStatus::Completed {
                outcome: WorkflowOutcome::from_output(serde_json::json!({ "ok": true })),
            },
            chain_head_before: [0; 32],
            chain_head_after: [2; 32],
            signature: None,
        }
    }

    #[test]
    fn digest_is_stable_across_equal_receipts() {
        let a = sample_receipt();
        let b = sample_receipt();
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn digest_changes_when_status_changes() {
        let a = sample_receipt();
        let mut b = sample_receipt();
        b.status = RunStatus::Refused;
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn signature_does_not_affect_digest() {
        let a = sample_receipt();
        let mut b = sample_receipt();
        b.signature = Some(vec![0xAB; 64]);
        assert_eq!(a.digest(), b.digest());
    }
}
