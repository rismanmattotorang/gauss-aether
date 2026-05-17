//! `gauss-attest` — TEE attestation surface (paper §IX.E, Theorem T10
//! Layer 4).
//!
//! Phase 10 ships the **trait + canonical wire format** plus a
//! deterministic Ed25519-backed software simulator. Production
//! attestors (AMD SEV-SNP, Intel TDX, ARM CCA) ship in additive plugin
//! crates that wrap the same trait; the conformance suite uses the
//! simulator so the build stays hardware-free and offline.
//!
//! The attestation payload is `(measurement, nonce)`; the signed
//! `AttestationReport` carries the bytes a verifier needs to confirm
//! the workload ran inside a trusted environment with the claimed
//! measurement.

use core::fmt;

use async_trait::async_trait;
use ed25519_dalek::{Signer as _, SigningKey, Verifier as _, VerifyingKey};
use gauss_core::GaussError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::Zeroize;

/// Attestor backend kind. New variants are semver-minor (the enum is
/// `#[non_exhaustive]`).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AttestKind {
    /// AMD SEV-SNP — production hardware backend (Phase-10 follow-up
    /// plugin crate).
    SevSnp,
    /// Intel TDX — production hardware backend (Phase-10 follow-up
    /// plugin crate).
    TdxIntel,
    /// ARM Confidential Compute Architecture — production hardware
    /// backend (Phase-10 follow-up plugin crate).
    ArmCca,
    /// Software simulator, signed by an Ed25519 CA key. Used by tests
    /// and the conformance suite; production verifiers refuse this kind
    /// unless explicitly configured to trust the simulator public key.
    Simulator,
}

/// Operator-readable claims a workload makes about itself.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Default)]
#[non_exhaustive]
pub struct AttestClaims {
    /// Identifier of the workload (e.g. `"gauss-aether/turn-engine"`).
    pub workload: String,
    /// Workload version (e.g. `"0.0.1"`).
    pub version: String,
    /// Optional cluster / node identifier.
    pub node: Option<String>,
}

impl AttestClaims {
    /// Construct.
    #[must_use]
    pub fn new(workload: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            workload: workload.into(),
            version: version.into(),
            node: None,
        }
    }
}

/// A signed attestation report.
///
/// The verifier's job is to confirm:
///
/// 1. The signature verifies against the trusted backend key.
/// 2. The `measurement` matches what the workload SHOULD be.
/// 3. The `nonce` matches what the verifier expected (replay defence).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AttestationReport {
    /// Backend kind that produced this report.
    pub kind: AttestKind,
    /// Workload-supplied claims.
    pub claims: AttestClaims,
    /// SHA-256 measurement of the workload (32 bytes).
    pub measurement: [u8; 32],
    /// 32-byte nonce supplied by the verifier (replay defence).
    pub nonce: [u8; 32],
    /// UTC ms since UNIX epoch when the report was produced.
    pub generated_at_ms: u64,
    /// Public key bytes (32) — Ed25519 verification key.
    pub public_key: [u8; 32],
    /// Signature bytes (64) — over the canonical pre-image (see
    /// [`canonical_bytes`]).
    pub signature: Vec<u8>,
}

impl AttestationReport {
    /// Render the measurement as lowercase hex (diagnostics only).
    #[must_use]
    pub fn measurement_hex(&self) -> String {
        hex::encode(self.measurement)
    }
}

/// Construct the canonical pre-image a backend signs.
///
/// Layout (little-endian integers):
/// `kind (1) ‖ measurement (32) ‖ nonce (32) ‖ generated_at_ms (8) ‖
///  workload_len (4) ‖ workload (utf8) ‖ version_len (4) ‖ version (utf8)
///  ‖ node_present (1) ‖ node_len (4) ‖ node (utf8)`
#[must_use]
pub fn canonical_bytes(
    kind: AttestKind,
    measurement: &[u8; 32],
    nonce: &[u8; 32],
    generated_at_ms: u64,
    claims: &AttestClaims,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    buf.push(kind_byte(kind));
    buf.extend_from_slice(measurement);
    buf.extend_from_slice(nonce);
    buf.extend_from_slice(&generated_at_ms.to_le_bytes());
    push_len_prefixed(&mut buf, claims.workload.as_bytes());
    push_len_prefixed(&mut buf, claims.version.as_bytes());
    if let Some(node) = claims.node.as_deref() {
        buf.push(1);
        push_len_prefixed(&mut buf, node.as_bytes());
    } else {
        buf.push(0);
    }
    buf
}

const fn kind_byte(k: AttestKind) -> u8 {
    match k {
        AttestKind::SevSnp => 0x53,
        AttestKind::TdxIntel => 0x54,
        AttestKind::ArmCca => 0x41,
        AttestKind::Simulator => 0x73,
    }
}

fn push_len_prefixed(buf: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(bytes);
}

/// Attestation verification error.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum AttestError {
    /// Backend transport / I/O failure.
    #[error("attest backend i/o: {0}")]
    Io(String),
    /// Signature did not verify against the report's public key.
    #[error("attestation signature did not verify: {0}")]
    SignatureInvalid(String),
    /// The nonce in the report did not match the expected nonce.
    #[error("attestation nonce mismatch")]
    NonceMismatch,
    /// The measurement in the report did not match the trusted baseline.
    #[error("attestation measurement does not match the trusted baseline")]
    MeasurementMismatch,
    /// The backend kind in the report did not match the verifier's
    /// configured trust roots.
    #[error("attestation kind {0:?} is not in the trusted set")]
    UntrustedKind(AttestKind),
}

impl From<AttestError> for GaussError {
    fn from(e: AttestError) -> Self {
        Self::Internal(format!("attest: {e}"))
    }
}

/// Pluggable attestor backend.
#[async_trait]
pub trait Attestor: Send + Sync {
    /// Backend kind.
    fn kind(&self) -> AttestKind;

    /// Produce an [`AttestationReport`] for `claims` + `nonce`. The
    /// workload's measurement is computed by the backend (in hardware
    /// backends this is read from the SEV-SNP / TDX / CCA register set;
    /// in the simulator it's computed via SHA-256 over the workload
    /// binary or a caller-supplied digest).
    ///
    /// # Errors
    /// Backend-specific failures are wrapped in
    /// [`AttestError::Io`].
    async fn attest(
        &self,
        claims: AttestClaims,
        nonce: [u8; 32],
    ) -> Result<AttestationReport, AttestError>;
}

/// Deterministic Ed25519-backed software attestor for tests + conformance.
pub struct SoftwareSimAttestor {
    inner: SigningKey,
    public_key: [u8; 32],
    /// Trusted baseline measurement; the simulator returns this verbatim
    /// so verifiers can pin it.
    baseline_measurement: [u8; 32],
    /// Pinned clock — `None` falls through to wall clock.
    fixed_clock_ms: Option<u64>,
}

impl fmt::Debug for SoftwareSimAttestor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoftwareSimAttestor")
            .field("public_key", &hex::encode(self.public_key))
            .field(
                "baseline_measurement",
                &hex::encode(self.baseline_measurement),
            )
            .finish_non_exhaustive()
    }
}

impl SoftwareSimAttestor {
    /// Build from a 32-byte seed + workload measurement.
    #[must_use]
    pub fn from_seed(seed: [u8; 32], baseline_measurement: [u8; 32]) -> Self {
        let inner = SigningKey::from_bytes(&seed);
        let public_key = inner.verifying_key().to_bytes();
        let mut tmp = seed;
        tmp.zeroize();
        Self {
            inner,
            public_key,
            baseline_measurement,
            fixed_clock_ms: None,
        }
    }

    /// Pin the wall clock for deterministic tests.
    #[must_use]
    pub const fn with_fixed_clock(mut self, ms: u64) -> Self {
        self.fixed_clock_ms = Some(ms);
        self
    }

    /// Read the simulator's public key (verifiers pin this).
    #[must_use]
    pub const fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    /// Read the baseline measurement.
    #[must_use]
    pub const fn baseline_measurement(&self) -> &[u8; 32] {
        &self.baseline_measurement
    }

    fn now_ms(&self) -> u64 {
        if let Some(ms) = self.fixed_clock_ms {
            return ms;
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| u64::try_from(d.as_millis()).ok())
            .unwrap_or(0)
    }
}

#[async_trait]
impl Attestor for SoftwareSimAttestor {
    fn kind(&self) -> AttestKind {
        AttestKind::Simulator
    }

    async fn attest(
        &self,
        claims: AttestClaims,
        nonce: [u8; 32],
    ) -> Result<AttestationReport, AttestError> {
        let generated_at_ms = self.now_ms();
        let pre = canonical_bytes(
            AttestKind::Simulator,
            &self.baseline_measurement,
            &nonce,
            generated_at_ms,
            &claims,
        );
        let signature = self.inner.sign(&pre).to_bytes().to_vec();
        Ok(AttestationReport {
            kind: AttestKind::Simulator,
            claims,
            measurement: self.baseline_measurement,
            nonce,
            generated_at_ms,
            public_key: self.public_key,
            signature,
        })
    }
}

/// Verify an attestation report.
///
/// `expected_nonce` is the nonce the verifier originally supplied.
/// `trusted_keys` is the set of acceptable public keys (one per trust
/// root); pass an empty slice to skip the public-key membership check
/// (only safe when the verifier already pinned the key via some other
/// channel).
///
/// # Errors
/// First failure short-circuits with the matching [`AttestError`].
pub fn verify_report(
    report: &AttestationReport,
    expected_nonce: &[u8; 32],
    trusted_keys: &[[u8; 32]],
    trusted_baseline: &[u8; 32],
) -> Result<(), AttestError> {
    if &report.nonce != expected_nonce {
        return Err(AttestError::NonceMismatch);
    }
    if &report.measurement != trusted_baseline {
        return Err(AttestError::MeasurementMismatch);
    }
    if !trusted_keys.is_empty() && !trusted_keys.iter().any(|k| k == &report.public_key) {
        return Err(AttestError::UntrustedKind(report.kind));
    }
    let pre = canonical_bytes(
        report.kind,
        &report.measurement,
        &report.nonce,
        report.generated_at_ms,
        &report.claims,
    );
    let vk = VerifyingKey::from_bytes(&report.public_key)
        .map_err(|e| AttestError::SignatureInvalid(format!("public key: {e}")))?;
    let sig_bytes: [u8; 64] = report
        .signature
        .clone()
        .try_into()
        .map_err(|_| AttestError::SignatureInvalid("signature length != 64".into()))?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);
    vk.verify(&pre, &signature)
        .map_err(|e| AttestError::SignatureInvalid(format!("verify: {e}")))
}

/// Helper: SHA-256 a workload binary blob into a measurement.
#[must_use]
pub fn measure_workload(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut m = [0u8; 32];
    m.copy_from_slice(&out);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det_attestor() -> SoftwareSimAttestor {
        SoftwareSimAttestor::from_seed([3u8; 32], measure_workload(b"workload-binary"))
            .with_fixed_clock(1_700_000_000_000)
    }

    #[tokio::test]
    async fn simulator_attest_round_trips_through_verifier() {
        let a = det_attestor();
        let claims = AttestClaims::new("gauss-aether/turn", "0.0.1");
        let nonce = [0x42; 32];
        let report = a.attest(claims, nonce).await.unwrap();
        let baseline = *a.baseline_measurement();
        verify_report(&report, &nonce, &[*a.public_key()], &baseline).unwrap();
    }

    #[tokio::test]
    async fn verifier_rejects_nonce_replay() {
        let a = det_attestor();
        let report = a
            .attest(AttestClaims::new("workload", "0.0.1"), [0x10; 32])
            .await
            .unwrap();
        let baseline = *a.baseline_measurement();
        let err = verify_report(&report, &[0x20; 32], &[*a.public_key()], &baseline).unwrap_err();
        assert!(matches!(err, AttestError::NonceMismatch));
    }

    #[tokio::test]
    async fn verifier_rejects_wrong_measurement() {
        let a = det_attestor();
        let report = a
            .attest(AttestClaims::new("workload", "0.0.1"), [0x11; 32])
            .await
            .unwrap();
        let wrong_baseline = [0xee; 32];
        let err =
            verify_report(&report, &[0x11; 32], &[*a.public_key()], &wrong_baseline).unwrap_err();
        assert!(matches!(err, AttestError::MeasurementMismatch));
    }

    #[tokio::test]
    async fn verifier_rejects_untrusted_key() {
        let a = det_attestor();
        let report = a
            .attest(AttestClaims::new("workload", "0.0.1"), [0x12; 32])
            .await
            .unwrap();
        let baseline = *a.baseline_measurement();
        let err = verify_report(&report, &[0x12; 32], &[[0u8; 32]], &baseline).unwrap_err();
        assert!(matches!(err, AttestError::UntrustedKind(_)));
    }

    #[tokio::test]
    async fn verifier_rejects_tampered_signature() {
        let a = det_attestor();
        let mut report = a
            .attest(AttestClaims::new("workload", "0.0.1"), [0x13; 32])
            .await
            .unwrap();
        // Flip a bit.
        if let Some(b) = report.signature.first_mut() {
            *b ^= 0x01;
        }
        let baseline = *a.baseline_measurement();
        let err = verify_report(&report, &[0x13; 32], &[*a.public_key()], &baseline).unwrap_err();
        assert!(matches!(err, AttestError::SignatureInvalid(_)));
    }

    #[test]
    fn canonical_bytes_are_stable() {
        let claims = AttestClaims::new("w", "1");
        let a = canonical_bytes(AttestKind::Simulator, &[0u8; 32], &[0u8; 32], 0, &claims);
        let b = canonical_bytes(AttestKind::Simulator, &[0u8; 32], &[0u8; 32], 0, &claims);
        assert_eq!(a, b);
    }

    #[test]
    fn measure_workload_is_deterministic() {
        let a = measure_workload(b"hello world");
        let b = measure_workload(b"hello world");
        assert_eq!(a, b);
        assert_ne!(a, measure_workload(b"hello world!"));
    }
}
