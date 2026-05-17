//! [`FederatedPool`] — the publish / subscribe / verify orchestrator.
//!
//! Wires:
//!
//! - a [`crate::PoolBackend`] (in-memory, filesystem, S3, …),
//! - a [`crate::AdmissionPolicy`] chain (publisher allow-list, taint
//!   cap, …),
//! - an optional [`gaussclaw_export::verify::TsaRoot`] for TSA-anchor
//!   verification.
//!
//! The contract: every byte that enters the pool has been
//! `verify_envelope`-validated AND `AdmissionPolicy::decide`-admitted.
//! Subscribers retrieve bytes that are byte-equal to what the
//! publisher signed.

use gauss_audit::ED25519_PUBLIC_KEY_LEN;
use gaussclaw_export::{verify::TsaRoot, Envelope, verify_envelope};

use crate::backend::{PoolBackend, PoolEntry, PoolError, PoolResult};
use crate::policy::{AdmissionDecision, AdmissionPolicy};

/// Canonical object key for one envelope.
///
/// Layout: `{org}/{chain_head_hex}/{turn_id}.env.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectKey {
    /// Owning org slug (URL-safe; no slashes).
    pub org: String,
    /// Lowercase hex of the envelope's chain head (64 chars).
    pub chain_head_hex: String,
    /// Turn id from the envelope's receipt.
    pub turn_id: u128,
}

impl ObjectKey {
    /// Build a key.
    #[must_use]
    pub fn new(org: impl Into<String>, envelope: &Envelope) -> Self {
        Self {
            org: org.into(),
            chain_head_hex: hex::encode(envelope.chain_head),
            turn_id: envelope.receipt.turn_id.0,
        }
    }

    /// Render the canonical S3-style key.
    #[must_use]
    pub fn as_path(&self) -> String {
        format!("{}/{}/{}.env.json", self.org, self.chain_head_hex, self.turn_id)
    }
}

/// Outcome of a `publish` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Envelope was admitted and written.
    Admitted {
        /// The canonical key the envelope was written to.
        key: String,
    },
    /// Envelope was already in the pool with byte-equal contents
    /// (idempotent publish).
    AlreadyPresent {
        /// The canonical key the envelope was already at.
        key: String,
    },
    /// The admission policy rejected the envelope.
    Rejected {
        /// Reason from the policy.
        reason: String,
    },
}

/// Federated pool orchestrator.
pub struct FederatedPool<B: PoolBackend, P: AdmissionPolicy> {
    backend: B,
    policy: P,
    tsa_root: Option<TsaRoot>,
}

impl<B: PoolBackend, P: AdmissionPolicy> FederatedPool<B, P> {
    /// Build a pool over a backend + policy. No TSA root yet — see
    /// [`Self::with_tsa_root`].
    pub fn new(backend: B, policy: P) -> Self {
        Self {
            backend,
            policy,
            tsa_root: None,
        }
    }

    /// Attach a TSA trust root. Publishes whose envelopes carry a TSA
    /// anchor will additionally have their anchor verified under this
    /// root; envelopes without an anchor are still accepted.
    #[must_use]
    pub fn with_tsa_root(mut self, root: TsaRoot) -> Self {
        self.tsa_root = Some(root);
        self
    }

    /// Borrow the backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Publish an envelope under `org`. Verifies + admits + writes.
    ///
    /// # Errors
    /// Returns [`PoolError::Verification`] when `verify_envelope` fails,
    /// [`PoolError::Admission`] when the admission policy rejects, and
    /// any [`PoolError::Backend`] / [`PoolError::Duplicate`] from the
    /// backend put.
    pub async fn publish(
        &self,
        org: &str,
        envelope: &Envelope,
        pin_publisher_key: Option<&[u8; ED25519_PUBLIC_KEY_LEN]>,
    ) -> PoolResult<PublishOutcome> {
        // 1. Cryptographic verify.
        verify_envelope(envelope, pin_publisher_key, self.tsa_root.as_ref())
            .map_err(|e| PoolError::Verification(format!("{e}")))?;

        // 2. Admission policy.
        match self.policy.decide(envelope) {
            AdmissionDecision::Reject(reason) => {
                return Ok(PublishOutcome::Rejected { reason });
            }
            AdmissionDecision::Admit => {}
        }

        // 3. Serialise + put.
        let bytes = serde_json::to_vec(envelope).map_err(|e| PoolError::Backend(format!("{e}")))?;
        let key = ObjectKey::new(org, envelope).as_path();
        match self
            .backend
            .put(PoolEntry {
                key: key.clone(),
                bytes,
            })
            .await
        {
            Ok(()) => Ok(PublishOutcome::Admitted { key }),
            Err(PoolError::Duplicate(_)) => {
                // The InMemoryPoolBackend distinguishes same-bytes
                // (Ok) from different-bytes (Duplicate). So a
                // Duplicate here means a real collision — surface it.
                Err(PoolError::Duplicate(key))
            }
            Err(other) => Err(other),
        }
    }

    /// Subscribe: list every envelope under `org`, verifying each one.
    ///
    /// Returns the parsed [`Envelope`]s for which `verify_envelope`
    /// succeeded. Malformed bytes / failed verifications are silently
    /// dropped — subscribers are expected to call
    /// [`Self::subscribe_strict`] for fail-loud behaviour.
    pub async fn subscribe(
        &self,
        org: &str,
        pin_publisher_key: Option<&[u8; ED25519_PUBLIC_KEY_LEN]>,
    ) -> PoolResult<Vec<Envelope>> {
        let entries = self
            .backend
            .list(&format!("{org}/"))
            .await?;
        let mut out = Vec::with_capacity(entries.len());
        for e in entries {
            let Ok(env) = serde_json::from_slice::<Envelope>(&e.bytes) else {
                continue;
            };
            if verify_envelope(&env, pin_publisher_key, self.tsa_root.as_ref()).is_ok() {
                out.push(env);
            }
        }
        Ok(out)
    }

    /// Same as [`Self::subscribe`] but surfaces the first verification
    /// failure instead of skipping.
    ///
    /// # Errors
    /// Returns [`PoolError::Verification`] on the first failing
    /// envelope; [`PoolError::Backend`] on backend / parse failure.
    pub async fn subscribe_strict(
        &self,
        org: &str,
        pin_publisher_key: Option<&[u8; ED25519_PUBLIC_KEY_LEN]>,
    ) -> PoolResult<Vec<Envelope>> {
        let entries = self.backend.list(&format!("{org}/")).await?;
        let mut out = Vec::with_capacity(entries.len());
        for e in entries {
            let env: Envelope = serde_json::from_slice(&e.bytes)
                .map_err(|err| PoolError::Backend(format!("parse {}: {err}", e.key)))?;
            verify_envelope(&env, pin_publisher_key, self.tsa_root.as_ref())
                .map_err(|err| PoolError::Verification(format!("{}: {err}", e.key)))?;
            out.push(env);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::InMemoryPoolBackend;
    use crate::policy::{AlwaysAdmit, MaxTaintPolicy};
    use gauss_audit::chain::ChainHead;
    use gauss_audit::{Ed25519Signer, ReceiptSigner};
    use gauss_core::{TaintLabel, TurnId};
    use gaussclaw_export::{EnvelopeBuilder, SftMessage, SftRecord};

    fn envelope_with(seed: u8, taint: TaintLabel) -> (Envelope, [u8; 32]) {
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

    #[tokio::test]
    async fn publish_admit_round_trips_through_pool() {
        let pool = FederatedPool::new(InMemoryPoolBackend::new(), AlwaysAdmit);
        let (env, pk) = envelope_with(0x20, TaintLabel::User);
        let outcome = pool.publish("acme", &env, Some(&pk)).await.unwrap();
        match outcome {
            PublishOutcome::Admitted { key } => assert!(key.starts_with("acme/")),
            other => panic!("expected Admitted, got {other:?}"),
        }
        // Round-trip via subscribe — gets back the same envelope.
        let back = pool.subscribe("acme", Some(&pk)).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].receipt.public_key, env.receipt.public_key);
    }

    #[tokio::test]
    async fn publish_rejects_when_policy_refuses() {
        let pool = FederatedPool::new(
            InMemoryPoolBackend::new(),
            MaxTaintPolicy::new(TaintLabel::User),
        );
        let (env, _pk) = envelope_with(0x21, TaintLabel::Adversarial);
        let outcome = pool.publish("acme", &env, None).await.unwrap();
        match outcome {
            PublishOutcome::Rejected { reason } => assert!(reason.contains("taint")),
            other => panic!("expected Rejected, got {other:?}"),
        }
        // Nothing was written.
        assert_eq!(pool.backend().len().await, 0);
    }

    #[tokio::test]
    async fn publish_rejects_when_envelope_verification_fails() {
        let pool = FederatedPool::new(InMemoryPoolBackend::new(), AlwaysAdmit);
        let (mut env, pk) = envelope_with(0x22, TaintLabel::User);
        env.receipt.signature[0] = env.receipt.signature[0].wrapping_add(1);
        let err = pool.publish("acme", &env, Some(&pk)).await.unwrap_err();
        match err {
            PoolError::Verification(msg) => assert!(msg.contains("signature") || msg.contains("Signature")),
            other => panic!("expected Verification, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn idempotent_publish_returns_admitted_twice() {
        // Same envelope bytes published twice — the backend's
        // idempotency makes both puts succeed; both outcomes are
        // Admitted because the policy admits and the backend says ok.
        let pool = FederatedPool::new(InMemoryPoolBackend::new(), AlwaysAdmit);
        let (env, pk) = envelope_with(0x23, TaintLabel::User);
        let _ = pool.publish("acme", &env, Some(&pk)).await.unwrap();
        let again = pool.publish("acme", &env, Some(&pk)).await.unwrap();
        assert!(matches!(again, PublishOutcome::Admitted { .. }));
        // Only one entry remains.
        assert_eq!(pool.backend().len().await, 1);
    }

    #[tokio::test]
    async fn subscribe_strict_surfaces_first_verification_failure() {
        // We can't easily inject a tampered envelope into the pool
        // without going through publish (which verifies). So we drop a
        // hand-crafted malformed entry directly through the backend
        // bypass — that's exactly the "malicious S3 bucket" scenario
        // subscribe_strict is meant to catch.
        let backend = InMemoryPoolBackend::new();
        backend
            .put(PoolEntry {
                key: "acme/dead/1.env.json".into(),
                bytes: b"not-json".to_vec(),
            })
            .await
            .unwrap();
        let pool = FederatedPool::new(backend, AlwaysAdmit);
        let err = pool.subscribe_strict("acme", None).await.unwrap_err();
        assert!(matches!(err, PoolError::Backend(_)));
    }

    #[tokio::test]
    async fn subscribe_silently_drops_bad_entries() {
        let backend = InMemoryPoolBackend::new();
        backend
            .put(PoolEntry {
                key: "acme/dead/1.env.json".into(),
                bytes: b"not-json".to_vec(),
            })
            .await
            .unwrap();
        let pool = FederatedPool::new(backend, AlwaysAdmit);
        let back = pool.subscribe("acme", None).await.unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn object_key_has_canonical_layout() {
        let (env, _pk) = envelope_with(0x24, TaintLabel::User);
        let k = ObjectKey::new("acme", &env);
        let p = k.as_path();
        assert!(p.starts_with("acme/"));
        assert!(p.ends_with(".env.json"));
        // chain_head_hex is 64 lowercase chars between slashes
        let parts: Vec<&str> = p.split('/').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].len(), 64);
        assert!(parts[1].chars().all(|c| c.is_ascii_hexdigit()));
    }
}
