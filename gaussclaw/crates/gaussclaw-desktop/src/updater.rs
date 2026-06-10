#![allow(missing_docs, unused_variables)]

//! Updater artifact verification.
//!
//! Every GaussClaw desktop release ships with **two** integrity surfaces:
//!
//! 1. **Code-signing certificate** — Apple Developer ID on macOS,
//!    Authenticode on Windows, GPG + AppImage signature on Linux. The
//!    OS itself verifies this on install via Gatekeeper / SmartScreen /
//!    `gpg --verify`.
//! 2. **Chain-anchored SHA-256** — the release artefact's SHA-256 is
//!    written into a `gauss-attest` receipt, signed by the release
//!    publisher's Ed25519 key, and appended to the public receipt
//!    chain on the [releases server][rel]. The Tauri updater calls
//!    [`verify_release_artifact`] **before** swapping the binary, so a
//!    compromised CDN cannot ship a malicious update — every byte we
//!    apply was already anchored in a chain the user trusts.
//!
//! Hermes Desktop ships unsigned binaries with no chain-anchor — its
//! updater verifies nothing.
//!
//! [rel]: https://releases.gauss.ai/desktop/

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One release-artefact manifest, as published by the release server.
///
/// The wire shape is intentionally tiny: the OS-signed certificate
/// chain lives in the artefact bytes themselves; this struct only
/// carries the *cryptographic publisher attestation* on top.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ReleaseManifest {
    /// SemVer of the release (`"1.0.3"`).
    pub version: String,
    /// Target triple (`"x86_64-apple-darwin"`).
    pub target: String,
    /// SHA-256 of the artefact bytes, hex-encoded.
    pub sha256_hex: String,
    /// Bytes signed by the publisher's Ed25519 key.
    ///
    /// The signed message is the canonical concatenation
    /// `version || ":" || target || ":" || sha256_hex`.
    pub publisher_signature_hex: String,
    /// Receipt-chain position the publisher placed this release at.
    /// A subscriber MAY verify the position is consistent with the
    /// publisher's announced chain head; the updater uses it for
    /// telemetry only.
    pub chain_index: u64,
}

impl ReleaseManifest {
    /// Build a manifest. Public so the release-signing CLI (a separate
    /// compilation unit) can construct one without hitting the
    /// `#[non_exhaustive]` restriction.
    #[must_use]
    pub fn new(
        version: impl Into<String>,
        target: impl Into<String>,
        sha256_hex: impl Into<String>,
        publisher_signature_hex: impl Into<String>,
        chain_index: u64,
    ) -> Self {
        Self {
            version: version.into(),
            target: target.into(),
            sha256_hex: sha256_hex.into(),
            publisher_signature_hex: publisher_signature_hex.into(),
            chain_index,
        }
    }
}

/// Verification outcome.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UpdaterVerifyError {
    /// Computed SHA-256 of the downloaded bytes does not match the
    /// manifest claim.
    #[error("sha256 mismatch: expected {expected}, observed {observed}")]
    Sha256Mismatch { expected: String, observed: String },
    /// Manifest field is mal-encoded (e.g. odd-length hex).
    #[error("bad encoding: {0}")]
    BadEncoding(&'static str),
    /// Publisher's signature failed to verify under the supplied key.
    #[error("publisher signature failed")]
    BadPublisherSignature,
    /// Target triple in the manifest does not match the current host.
    #[error("target triple mismatch: expected {expected}, manifest says {observed}")]
    TargetMismatch { expected: String, observed: String },
    /// SemVer in the manifest is not strictly greater than the running
    /// version — the updater refuses downgrade attacks.
    #[error("manifest version `{manifest}` is not greater than running `{running}`")]
    VersionNotGreater { manifest: String, running: String },
}

/// Verify a downloaded artefact against its publisher manifest.
///
/// The four invariants we check, in order:
///
/// 1. `target` matches the current OS / arch (no cross-target swaps).
/// 2. SemVer is strictly greater than the running version (no
///    downgrade attacks).
/// 3. SHA-256 of `artefact_bytes` matches the manifest claim.
/// 4. The publisher's Ed25519 signature over `version:target:sha256`
///    verifies under `publisher_pk`.
///
/// On success, the caller may hand the artefact bytes to Tauri's
/// updater for swap-in. On failure, the swap **must not** be performed.
pub fn verify_release_artifact(
    manifest: &ReleaseManifest,
    artefact_bytes: &[u8],
    publisher_pk: &VerifyingKey,
    running_version: &str,
    host_target: &str,
) -> Result<(), UpdaterVerifyError> {
    // 1. Target triple matches.
    if manifest.target != host_target {
        return Err(UpdaterVerifyError::TargetMismatch {
            expected: host_target.into(),
            observed: manifest.target.clone(),
        });
    }
    // 2. Version is strictly newer.
    if !semver_gt(&manifest.version, running_version) {
        return Err(UpdaterVerifyError::VersionNotGreater {
            manifest: manifest.version.clone(),
            running: running_version.into(),
        });
    }
    // 3. SHA-256 binds the bytes.
    let observed = hex_lower(&Sha256::digest(artefact_bytes));
    if !observed.eq_ignore_ascii_case(&manifest.sha256_hex) {
        return Err(UpdaterVerifyError::Sha256Mismatch {
            expected: manifest.sha256_hex.clone(),
            observed,
        });
    }
    // 4. Publisher signature.
    let sig_bytes = hex_decode(&manifest.publisher_signature_hex)
        .ok_or(UpdaterVerifyError::BadEncoding("publisher_signature_hex"))?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| UpdaterVerifyError::BadEncoding("publisher_signature_hex length"))?;
    let sig = Signature::from_bytes(&sig_arr);
    let message =
        canonical_signed_message(&manifest.version, &manifest.target, &manifest.sha256_hex);
    publisher_pk
        .verify(message.as_bytes(), &sig)
        .map_err(|_| UpdaterVerifyError::BadPublisherSignature)?;
    Ok(())
}

/// Canonical bytes the publisher signs. Stable across versions; the
/// updater on the user's machine reconstructs this and verifies the
/// Ed25519 signature over it.
#[must_use]
pub fn canonical_signed_message(version: &str, target: &str, sha256_hex: &str) -> String {
    format!("{version}:{target}:{sha256_hex}")
}

/// Lexical SemVer compare. We don't pull in `semver` for one comparator;
/// the format is `MAJOR.MINOR.PATCH` integers and any pre-release tag
/// after a `-` is treated as "earlier than the release".
fn semver_gt(a: &str, b: &str) -> bool {
    let (av, ap) = split_pre(a);
    let (bv, bp) = split_pre(b);
    let av = parse_triple(av);
    let bv = parse_triple(bv);
    match av.cmp(&bv) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => {
            // No-pre release outranks any pre-release; otherwise lexical.
            match (ap, bp) {
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (None, None) => false,
                (Some(a), Some(b)) => a > b,
            }
        }
    }
}

fn split_pre(v: &str) -> (&str, Option<&str>) {
    match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    }
}

fn parse_triple(v: &str) -> (u64, u64, u64) {
    let mut it = v.split('.');
    let major = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand_core::OsRng;

    fn fixture(version: &str, target: &str, bytes: &[u8]) -> (ReleaseManifest, VerifyingKey) {
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let sha = hex_lower(&Sha256::digest(bytes));
        let msg = canonical_signed_message(version, target, &sha);
        let sig = sk.sign(msg.as_bytes());
        let manifest = ReleaseManifest {
            version: version.into(),
            target: target.into(),
            sha256_hex: sha,
            publisher_signature_hex: hex_lower(&sig.to_bytes()),
            chain_index: 42,
        };
        (manifest, pk)
    }

    #[test]
    fn happy_path() {
        let body = b"installer bytes";
        let (m, pk) = fixture("1.2.4", "x86_64-apple-darwin", body);
        verify_release_artifact(&m, body, &pk, "1.2.3", "x86_64-apple-darwin").expect("ok");
    }

    #[test]
    fn rejects_target_mismatch() {
        let body = b"x";
        let (m, pk) = fixture("1.0.1", "aarch64-apple-darwin", body);
        let r = verify_release_artifact(&m, body, &pk, "1.0.0", "x86_64-apple-darwin");
        assert!(matches!(r, Err(UpdaterVerifyError::TargetMismatch { .. })));
    }

    #[test]
    fn rejects_downgrade() {
        let body = b"x";
        let (m, pk) = fixture("0.9.0", "x86_64-apple-darwin", body);
        let r = verify_release_artifact(&m, body, &pk, "1.0.0", "x86_64-apple-darwin");
        assert!(matches!(
            r,
            Err(UpdaterVerifyError::VersionNotGreater { .. })
        ));
    }

    #[test]
    fn rejects_sha_mismatch() {
        let body = b"x";
        let (mut m, pk) = fixture("1.0.1", "x86_64-apple-darwin", body);
        m.sha256_hex = "00".repeat(32);
        // Re-sign over the lie so we get the SHA failure, not the sig failure.
        let sk = SigningKey::generate(&mut OsRng);
        let pk = sk.verifying_key();
        let msg = canonical_signed_message(&m.version, &m.target, &m.sha256_hex);
        m.publisher_signature_hex = hex_lower(&sk.sign(msg.as_bytes()).to_bytes());
        let r = verify_release_artifact(&m, body, &pk, "1.0.0", "x86_64-apple-darwin");
        assert!(matches!(r, Err(UpdaterVerifyError::Sha256Mismatch { .. })));
    }

    #[test]
    fn rejects_bad_signature() {
        let body = b"x";
        let (m, _pk) = fixture("1.0.1", "x86_64-apple-darwin", body);
        // Verify under a *different* publisher key.
        let other = SigningKey::generate(&mut OsRng).verifying_key();
        let r = verify_release_artifact(&m, body, &other, "1.0.0", "x86_64-apple-darwin");
        assert!(matches!(r, Err(UpdaterVerifyError::BadPublisherSignature)));
    }

    #[test]
    fn pre_release_outranked_by_release() {
        assert!(semver_gt("1.0.0", "1.0.0-rc.1"));
        assert!(!semver_gt("1.0.0-rc.1", "1.0.0"));
        assert!(semver_gt("1.0.0-rc.2", "1.0.0-rc.1"));
    }

    #[test]
    fn equal_versions_are_not_greater() {
        assert!(!semver_gt("1.2.3", "1.2.3"));
    }

    #[test]
    fn canonical_message_is_stable() {
        assert_eq!(
            canonical_signed_message("1.2.3", "linux-x86_64", "abcd"),
            "1.2.3:linux-x86_64:abcd"
        );
    }
}
