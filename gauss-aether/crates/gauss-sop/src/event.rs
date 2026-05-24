//! [`TriggerEvent`] — the canonical envelope a [`Trigger`](crate::Trigger)
//! emits and a [`Workflow`](crate::Workflow) consumes.
//!
//! The event carries opaque `payload` JSON (whatever the trigger
//! source extracts from the underlying transport — webhook body, MQTT
//! message, cron tick metadata, etc.) plus a stable identifier and
//! the trigger-side taint label.
//!
//! The [`Self::digest`] is BLAKE3 over the canonical encoding
//! `name || 0x00 || serde_json::to_vec(payload)`. The double-NUL-byte
//! separator prevents collision between events whose `name` is a
//! prefix of another's name + payload bytes.

use blake3::Hasher;
use serde::{Deserialize, Serialize};

/// One event emitted by a [`Trigger`](crate::Trigger).
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct TriggerEvent {
    /// Stable identifier for the trigger that produced the event.
    /// Used in the [`crate::SopRunReceipt`] to identify which SOP
    /// processed which trigger.
    pub trigger_name: String,
    /// Free-form payload. Workflows are responsible for validating
    /// the shape they expect.
    pub payload: serde_json::Value,
    /// True iff the event was received over a transport that admits
    /// untrusted senders (webhook ingress, MQTT broker, etc.).
    /// Defaults to `true`; trusted-source triggers downgrade
    /// explicitly. The engine surfaces this to workflows so they can
    /// refuse cap-sensitive steps under adversarial taint, matching
    /// `MEMORY_READ`'s default declass behaviour.
    pub adversarial: bool,
}

impl TriggerEvent {
    /// Build an adversarial-by-default event.
    pub fn new(trigger_name: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            trigger_name: trigger_name.into(),
            payload,
            adversarial: true,
        }
    }

    /// Mark the event as coming from a trusted source. Use for
    /// internal cron ticks and operator-initiated `sop run` calls;
    /// never for transport-level ingress.
    #[must_use]
    pub fn trusted(mut self) -> Self {
        self.adversarial = false;
        self
    }

    /// BLAKE3 digest over the canonical bytes:
    /// `trigger_name.as_bytes() || 0x00 || serde_json::to_vec(payload)`.
    ///
    /// Stable across `serde_json` versions only because
    /// `serde_json::to_vec` emits keys in insertion order; callers
    /// that need cross-version stability should pre-canonicalise.
    /// Adversarial taint is not part of the digest — the same event
    /// over two transports produces the same identifier.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut h = Hasher::new();
        h.update(self.trigger_name.as_bytes());
        h.update(&[0u8]);
        let body = serde_json::to_vec(&self.payload).unwrap_or_default();
        h.update(&body);
        *h.finalize().as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_for_equal_events() {
        let a = TriggerEvent::new("webhook", serde_json::json!({ "x": 1 }));
        let b = TriggerEvent::new("webhook", serde_json::json!({ "x": 1 }));
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn digest_differs_across_payload_changes() {
        let a = TriggerEvent::new("webhook", serde_json::json!({ "x": 1 }));
        let b = TriggerEvent::new("webhook", serde_json::json!({ "x": 2 }));
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn digest_differs_across_trigger_name_changes() {
        let a = TriggerEvent::new("webhook", serde_json::json!({}));
        let b = TriggerEvent::new("mqtt", serde_json::json!({}));
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn trusted_helper_flips_adversarial_default() {
        let e = TriggerEvent::new("cron", serde_json::Value::Null).trusted();
        assert!(!e.adversarial);
    }

    #[test]
    fn adversarial_flag_does_not_affect_digest() {
        let a = TriggerEvent::new("webhook", serde_json::json!({ "x": 1 }));
        let b = TriggerEvent::new("webhook", serde_json::json!({ "x": 1 })).trusted();
        assert_eq!(a.digest(), b.digest());
    }
}
