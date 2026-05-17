//! Receipt-chain anchoring — RFC 3161 TSA + `OpenTimestamps` fallback.
//!
//! Phase 5 ships the *trait surface* and a deterministic in-process simulator
//! that exercises the anchoring path end-to-end. The real RFC 3161 HTTP
//! client and `OpenTimestamps` Bitcoin-anchor client are additive feature
//! crates that wrap [`TsaClient`] for production deployments; both keep the
//! conformance suite offline.
//!
//! Anchor semantics:
//!
//! * An [`Anchor`] binds a specific chain head to an externally-witnessable
//!   token (RFC 3161 reply OR an `OpenTimestamps` `.ots` proof).
//! * Tokens are opaque bytes here; production verifiers decode them against
//!   the upstream PKI / timestamp ledger.
//! * The [`TsaClient`] trait is async-friendly to accommodate HTTP backends.

use core::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ed25519_dalek::{Signer as _, SigningKey, Verifier as _, VerifyingKey};
use gauss_core::{GaussError, GaussResult};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::chain::ChainHead;
use crate::sign::{ED25519_PUBLIC_KEY_LEN, ED25519_SECRET_KEY_LEN, ED25519_SIGNATURE_LEN};

/// External-trust kind for an anchor token. Distinct kinds verify against
/// different upstream authorities.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnchorKind {
    /// RFC 3161 Time-Stamp Protocol reply (DER-encoded).
    Rfc3161,
    /// `OpenTimestamps` `.ots` proof anchoring into Bitcoin / Calendars.
    OpenTimestamps,
    /// Offline simulator that signs `(head, timestamp_ms)` with an Ed25519
    /// CA key. Used by tests and the deterministic in-process anchoring
    /// path; production verifiers refuse this kind unless configured to
    /// trust the simulator key.
    Simulator,
}

/// One anchor over a chain head.
///
/// The chain index is stored alongside the head so verifiers can locate the
/// covered append without re-hashing the entire log; the timestamp is in UTC
/// milliseconds since the UNIX epoch.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct Anchor {
    /// Anchoring authority kind.
    pub kind: AnchorKind,
    /// 0-based index of the chain head that was anchored.
    pub anchored_at_index: u64,
    /// The chain head digest covered by this anchor.
    pub head: [u8; 32],
    /// UTC milliseconds since UNIX epoch when the anchor was produced.
    pub anchored_at_ms: u64,
    /// Opaque token bytes — decoded by the upstream authority.
    pub token: Vec<u8>,
}

impl Anchor {
    /// Render the [`Anchor::head`] as lowercase hex (diagnostics only).
    #[must_use]
    pub fn head_hex(&self) -> String {
        hex::encode(self.head)
    }
}

/// Pluggable timestamping client. Production deployments wire RFC 3161 (HTTP)
/// or `OpenTimestamps` (Calendar HTTP); the conformance suite uses
/// [`SimulatorTsaClient`].
#[async_trait]
pub trait TsaClient: Send + Sync {
    /// Anchoring authority kind this client produces.
    fn kind(&self) -> AnchorKind;

    /// Anchor `head` (covering `index` records) and return the anchor.
    ///
    /// # Errors
    /// Backend-specific failures are wrapped in
    /// [`GaussError::AnchorFailed`].
    async fn anchor(&self, head: ChainHead, index: u64) -> GaussResult<Anchor>;
}

/// Deterministic Ed25519-backed TSA simulator. The CA key signs
/// `kind ‖ index ‖ head ‖ ts_ms`; verifiers reconstruct that buffer and check
/// the signature with the simulator's public key.
///
/// The simulator is `Zeroize`-on-drop for the secret key so even test
/// processes don't leak it.
pub struct SimulatorTsaClient {
    inner: SigningKey,
    public_key: [u8; ED25519_PUBLIC_KEY_LEN],
    /// Monotone clock for deterministic tests; if `None`, the wall clock is
    /// used.
    fixed_clock_ms: Option<u64>,
}

impl fmt::Debug for SimulatorTsaClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SimulatorTsaClient")
            .field("public_key", &hex::encode(self.public_key))
            .field("fixed_clock_ms", &self.fixed_clock_ms)
            .finish_non_exhaustive()
    }
}

impl SimulatorTsaClient {
    /// Build a simulator from a 32-byte deterministic seed (test vectors).
    #[must_use]
    pub fn from_seed(seed: [u8; ED25519_SECRET_KEY_LEN]) -> Self {
        let inner = SigningKey::from_bytes(&seed);
        let public_key = inner.verifying_key().to_bytes();
        let mut tmp = seed;
        tmp.zeroize();
        Self {
            inner,
            public_key,
            fixed_clock_ms: None,
        }
    }

    /// Pin the wall clock to a deterministic value (used by tests).
    #[must_use]
    pub const fn with_fixed_clock(mut self, ms: u64) -> Self {
        self.fixed_clock_ms = Some(ms);
        self
    }

    /// Public key for verifier configuration.
    #[must_use]
    pub const fn public_key(&self) -> &[u8; ED25519_PUBLIC_KEY_LEN] {
        &self.public_key
    }

    /// Verify an anchor produced by this simulator (offline path).
    ///
    /// # Errors
    /// Returns [`GaussError::AnchorFailed`] on token-length, kind, or
    /// signature mismatch.
    pub fn verify(&self, anchor: &Anchor) -> GaussResult<()> {
        if anchor.kind != AnchorKind::Simulator {
            return Err(GaussError::AnchorFailed(format!(
                "simulator verifier called on non-simulator anchor ({:?})",
                anchor.kind
            )));
        }
        if anchor.token.len() != ED25519_SIGNATURE_LEN {
            return Err(GaussError::AnchorFailed(format!(
                "simulator anchor token length {} != {}",
                anchor.token.len(),
                ED25519_SIGNATURE_LEN
            )));
        }
        let canonical = canonical_anchor_bytes(
            anchor.kind,
            anchor.anchored_at_index,
            &anchor.head,
            anchor.anchored_at_ms,
        );
        let vk = VerifyingKey::from_bytes(&self.public_key)
            .map_err(|e| GaussError::AnchorFailed(format!("simulator public key: {e}")))?;
        let mut sig = [0u8; ED25519_SIGNATURE_LEN];
        sig.copy_from_slice(&anchor.token);
        vk.verify(&canonical, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|e| GaussError::AnchorFailed(format!("simulator anchor verify: {e}")))
    }
}

#[async_trait]
impl TsaClient for SimulatorTsaClient {
    fn kind(&self) -> AnchorKind {
        AnchorKind::Simulator
    }

    async fn anchor(&self, head: ChainHead, index: u64) -> GaussResult<Anchor> {
        let anchored_at_ms = self.fixed_clock_ms.unwrap_or_else(now_ms);
        let canonical = canonical_anchor_bytes(
            AnchorKind::Simulator,
            index,
            head.as_bytes(),
            anchored_at_ms,
        );
        let sig = self.inner.sign(&canonical).to_bytes();
        Ok(Anchor {
            kind: AnchorKind::Simulator,
            anchored_at_index: index,
            head: *head.as_bytes(),
            anchored_at_ms,
            token: sig.to_vec(),
        })
    }
}

/// Canonical pre-image used by [`SimulatorTsaClient::verify`] and
/// [`SimulatorTsaClient::anchor`]. Public so cross-language verifiers can
/// replicate it.
#[must_use]
pub fn canonical_anchor_bytes(
    kind: AnchorKind,
    index: u64,
    head: &[u8; 32],
    ts_ms: u64,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + 8 + 32 + 8);
    buf.push(anchor_kind_to_byte(kind));
    buf.extend_from_slice(&index.to_le_bytes());
    buf.extend_from_slice(head);
    buf.extend_from_slice(&ts_ms.to_le_bytes());
    buf
}

const fn anchor_kind_to_byte(kind: AnchorKind) -> u8 {
    match kind {
        // ASCII '1' — distinguishes the RFC 3161 family.
        AnchorKind::Rfc3161 => 0x31,
        // ASCII 'O' — `OpenTimestamps`.
        AnchorKind::OpenTimestamps => 0x4F,
        // ASCII 'S' — Simulator.
        AnchorKind::Simulator => 0x53,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det_client() -> SimulatorTsaClient {
        SimulatorTsaClient::from_seed([5u8; 32]).with_fixed_clock(1_700_000_000_000)
    }

    #[tokio::test]
    async fn simulator_anchor_round_trips_through_its_verifier() {
        let client = det_client();
        let head = ChainHead::from_bytes([0x42; 32]);
        let anchor = client.anchor(head, 5).await.unwrap();
        client.verify(&anchor).unwrap();
        assert_eq!(anchor.kind, AnchorKind::Simulator);
        assert_eq!(anchor.anchored_at_index, 5);
        assert_eq!(anchor.anchored_at_ms, 1_700_000_000_000);
    }

    #[tokio::test]
    async fn anchor_is_deterministic_for_fixed_inputs() {
        let client_a = det_client();
        let client_b = det_client();
        let head = ChainHead::from_bytes([0x99; 32]);
        let a = client_a.anchor(head, 1).await.unwrap();
        let b = client_b.anchor(head, 1).await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn tampered_anchor_token_is_rejected() {
        let client = det_client();
        let head = ChainHead::from_bytes([0x11; 32]);
        let mut anchor = client.anchor(head, 0).await.unwrap();
        anchor.token[0] ^= 0x01;
        let err = client.verify(&anchor).unwrap_err();
        assert!(format!("{err}").contains("anchor verify"));
    }

    #[tokio::test]
    async fn anchor_with_wrong_index_does_not_verify() {
        let client = det_client();
        let head = ChainHead::from_bytes([0x22; 32]);
        let mut anchor = client.anchor(head, 0).await.unwrap();
        anchor.anchored_at_index = 999;
        let err = client.verify(&anchor).unwrap_err();
        assert!(format!("{err}").contains("anchor verify"));
    }

    #[test]
    fn canonical_anchor_bytes_are_stable() {
        let a = canonical_anchor_bytes(AnchorKind::Simulator, 1, &[0u8; 32], 0);
        let b = canonical_anchor_bytes(AnchorKind::Simulator, 1, &[0u8; 32], 0);
        assert_eq!(a, b);
        assert_eq!(a.len(), 1 + 8 + 32 + 8);
    }

    #[tokio::test]
    async fn verifier_rejects_non_simulator_kind() {
        let client = det_client();
        let anchor = Anchor {
            kind: AnchorKind::Rfc3161,
            anchored_at_index: 0,
            head: [0u8; 32],
            anchored_at_ms: 0,
            token: vec![0u8; ED25519_SIGNATURE_LEN],
        };
        let err = client.verify(&anchor).unwrap_err();
        assert!(format!("{err}").contains("non-simulator anchor"));
    }
}
