//! Admission policy combinators.
//!
//! An admission policy is a pure function over an envelope (and its
//! publisher metadata) returning [`AdmissionDecision::Admit`] or
//! [`AdmissionDecision::Reject`] with a reason. Policies compose: a
//! pool can chain a publisher-allow-list policy then a max-taint
//! policy then a custom record-content filter, and the first rejection
//! short-circuits.

use std::sync::Arc;

use gauss_audit::ED25519_PUBLIC_KEY_LEN;
use gauss_core::TaintLabel;
use gaussclaw_export::Envelope;

/// Decision returned by [`AdmissionPolicy::decide`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Admit the envelope into the pool.
    Admit,
    /// Reject with a human-readable reason.
    Reject(String),
}

/// Trait surface for an admission policy.
pub trait AdmissionPolicy: Send + Sync {
    /// Make a decision for `envelope`. The publisher's public key is
    /// derived from `envelope.receipt.public_key` and is also passed
    /// explicitly to policies that gate on publisher identity.
    fn decide(&self, envelope: &Envelope) -> AdmissionDecision;
}

/// Publisher allow-list policy. Admit iff the envelope's signer is
/// in the explicit allow-list.
pub struct PublisherAllowList {
    keys: Vec<[u8; ED25519_PUBLIC_KEY_LEN]>,
}

impl PublisherAllowList {
    /// Build an allow list from a set of publisher public keys.
    #[must_use]
    pub fn new(keys: Vec<[u8; ED25519_PUBLIC_KEY_LEN]>) -> Self {
        Self { keys }
    }

    /// Number of publishers in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True iff the list is empty (no admits).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl AdmissionPolicy for PublisherAllowList {
    fn decide(&self, envelope: &Envelope) -> AdmissionDecision {
        if self.keys.iter().any(|k| k == &envelope.receipt.public_key) {
            AdmissionDecision::Admit
        } else {
            AdmissionDecision::Reject("publisher not in allow list".into())
        }
    }
}

/// Max-taint policy. Admit iff the envelope's receipt taint ⪯ `max`.
///
/// Combine with [`crate::pool::FederatedPool`]'s per-publish taint
/// filter to get the "subscriber-side" guarantee that no record with
/// taint above `max` enters the pool.
pub struct MaxTaintPolicy {
    max: TaintLabel,
}

impl MaxTaintPolicy {
    /// Cap at the given taint.
    #[must_use]
    pub fn new(max: TaintLabel) -> Self {
        Self { max }
    }
}

impl AdmissionPolicy for MaxTaintPolicy {
    fn decide(&self, envelope: &Envelope) -> AdmissionDecision {
        if envelope.receipt.taint.leq(self.max) {
            AdmissionDecision::Admit
        } else {
            AdmissionDecision::Reject(format!(
                "taint {:?} exceeds max {:?}",
                envelope.receipt.taint, self.max
            ))
        }
    }
}

/// Compose any number of policies into one. The first rejection
/// short-circuits.
pub struct ChainedPolicy {
    inner: Vec<Arc<dyn AdmissionPolicy>>,
}

impl ChainedPolicy {
    /// Build a chain.
    #[must_use]
    pub fn new(inner: Vec<Arc<dyn AdmissionPolicy>>) -> Self {
        Self { inner }
    }
}

impl AdmissionPolicy for ChainedPolicy {
    fn decide(&self, envelope: &Envelope) -> AdmissionDecision {
        for p in &self.inner {
            if let AdmissionDecision::Reject(why) = p.decide(envelope) {
                return AdmissionDecision::Reject(why);
            }
        }
        AdmissionDecision::Admit
    }
}

/// Always-admit policy. Used by the `permissive` deployment mode.
#[derive(Debug, Default)]
pub struct AlwaysAdmit;

impl AdmissionPolicy for AlwaysAdmit {
    fn decide(&self, _envelope: &Envelope) -> AdmissionDecision {
        AdmissionDecision::Admit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_audit::chain::ChainHead;
    use gauss_audit::{Ed25519Signer, ReceiptSigner};
    use gauss_core::TurnId;
    use gaussclaw_export::{EnvelopeBuilder, SftMessage, SftRecord};

    fn envelope_with_seed_and_taint(seed: u8, taint: TaintLabel) -> (Envelope, [u8; 32]) {
        let signer = ReceiptSigner::new(Ed25519Signer::from_seed([seed; 32]));
        let body = SftRecord::from_messages(vec![SftMessage::new("assistant", "x")]);
        let body_bytes =
            serde_json::to_vec(&gaussclaw_export::envelope::EnvelopeBody::Sft(body.clone()))
                .unwrap();
        let prev = ChainHead::from_bytes([0u8; 32]);
        let receipt = signer
            .sign_append(TurnId::new(1), 0, prev, &body_bytes, taint, 0)
            .unwrap();
        let pk = *signer.backend().public_key();
        let env = EnvelopeBuilder::for_sft(body, receipt).build().unwrap();
        (env, pk)
    }

    #[test]
    fn allow_list_admits_listed_publisher() {
        let (env, pk) = envelope_with_seed_and_taint(0x10, TaintLabel::User);
        let p = PublisherAllowList::new(vec![pk]);
        assert_eq!(p.decide(&env), AdmissionDecision::Admit);
    }

    #[test]
    fn allow_list_rejects_unlisted_publisher() {
        let (env, _pk) = envelope_with_seed_and_taint(0x11, TaintLabel::User);
        let p = PublisherAllowList::new(vec![[0xffu8; ED25519_PUBLIC_KEY_LEN]]);
        assert!(matches!(p.decide(&env), AdmissionDecision::Reject(_)));
    }

    #[test]
    fn max_taint_admits_below_cap() {
        let (env, _) = envelope_with_seed_and_taint(0x12, TaintLabel::User);
        let p = MaxTaintPolicy::new(TaintLabel::Web);
        assert_eq!(p.decide(&env), AdmissionDecision::Admit);
    }

    #[test]
    fn max_taint_rejects_above_cap() {
        let (env, _) = envelope_with_seed_and_taint(0x13, TaintLabel::Adversarial);
        let p = MaxTaintPolicy::new(TaintLabel::User);
        assert!(matches!(p.decide(&env), AdmissionDecision::Reject(_)));
    }

    #[test]
    fn chain_short_circuits_on_first_reject() {
        let (env, _pk) = envelope_with_seed_and_taint(0x14, TaintLabel::Adversarial);
        let chained = ChainedPolicy::new(vec![
            Arc::new(MaxTaintPolicy::new(TaintLabel::User)),
            Arc::new(AlwaysAdmit),
        ]);
        // Taint policy rejects first → chain returns that reject; the
        // AlwaysAdmit later in the chain is irrelevant.
        match chained.decide(&env) {
            AdmissionDecision::Reject(why) => assert!(why.contains("taint")),
            AdmissionDecision::Admit => panic!("expected reject"),
        }
    }

    #[test]
    fn chain_admits_when_every_member_admits() {
        let (env, pk) = envelope_with_seed_and_taint(0x15, TaintLabel::User);
        let chained = ChainedPolicy::new(vec![
            Arc::new(PublisherAllowList::new(vec![pk])),
            Arc::new(MaxTaintPolicy::new(TaintLabel::Web)),
        ]);
        assert_eq!(chained.decide(&env), AdmissionDecision::Admit);
    }
}
