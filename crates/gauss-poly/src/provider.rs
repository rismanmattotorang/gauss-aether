//! Polyhedral equivalence for [`gauss_traits::Provider`].
//!
//! Theorem T7 (paper §XII.B) says the provider surface forms a
//! contravariant adjunction: a deployment that swaps one Provider for
//! another (e.g. `Anthropic` ↔ `OpenAI`) produces semantically-equivalent
//! transcripts when both providers are polyhedrally equivalent.
//!
//! This module mechanises the equivalence check. Given two `Provider`
//! impls and a finite probe set of observations, the verifier:
//!
//! 1. Calls `p.generate(obs)` and `q.generate(obs)` for each probe.
//! 2. Canonicalises both action vectors through `serde_json::to_vec`.
//! 3. Compares the canonical bytes.
//!
//! Any divergence is reported as a [`SwapEquivalenceError`] with the
//! probe name, the canonical bytes from each side, and the first probe
//! index that diverged. The verifier short-circuits at the first
//! divergence.

use gauss_core::{Action, Observation};
use gauss_traits::Provider;
use thiserror::Error;

use crate::probe::PolyhedralProbeSet;

/// First-divergence report.
#[derive(Debug, Clone, Error)]
#[error(
    "provider polyhedral equivalence failed at probe {probe_index} ({probe_name}): \
     p emitted {p_canonical_len} bytes, q emitted {q_canonical_len} bytes"
)]
#[non_exhaustive]
pub struct SwapEquivalenceError {
    /// 0-based position of the diverging probe in the probe set.
    pub probe_index: usize,
    /// Human-readable probe name.
    pub probe_name: String,
    /// Canonical JSON bytes produced by the first provider.
    pub p_canonical: Vec<u8>,
    /// Length of the first provider's canonical bytes.
    pub p_canonical_len: usize,
    /// Canonical JSON bytes produced by the second provider.
    pub q_canonical: Vec<u8>,
    /// Length of the second provider's canonical bytes.
    pub q_canonical_len: usize,
}

impl SwapEquivalenceError {
    fn new(probe_index: usize, probe_name: impl Into<String>, p: Vec<u8>, q: Vec<u8>) -> Self {
        Self {
            probe_index,
            probe_name: probe_name.into(),
            p_canonical_len: p.len(),
            q_canonical_len: q.len(),
            p_canonical: p,
            q_canonical: q,
        }
    }
}

/// Successful run report (probe count + behavioural divergence histogram).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ProviderEquivalenceReport {
    /// Number of probes that passed without divergence.
    pub passed: usize,
    /// Total probes attempted.
    pub total: usize,
}

impl ProviderEquivalenceReport {
    /// All probes passed.
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.passed == self.total
    }

    /// Behavioural-divergence ratio in `[0, 1]`. `0.0` means perfect
    /// equivalence; the SPECS Phase-8 exit gate pins this `<= 0.05` for
    /// the toy-provider compatibility suite.
    ///
    /// Returns `0.0` for an empty probe set so trivially-empty runs
    /// don't claim divergence.
    #[must_use]
    pub fn divergence(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let missed = self.total.saturating_sub(self.passed);
        // Counts are bounded by `total <= probe set size`, typically <
        // 10^4; saturating to `u32::MAX` here is safety paint.
        let missed_u32 = u32::try_from(missed).unwrap_or(u32::MAX);
        let total_u32 = u32::try_from(self.total).unwrap_or(u32::MAX);
        f64::from(missed_u32) / f64::from(total_u32)
    }
}

/// Verify that two `Provider` impls are polyhedrally equivalent on the
/// given probe set.
///
/// Each probe contributes one observation to feed both providers; the
/// `expected` field of [`crate::Probe`] is a canonical
/// `Vec<Action>` against which BOTH providers must match. Returns
/// [`ProviderEquivalenceReport`] when all probes agree, or
/// [`SwapEquivalenceError`] at the first divergence.
///
/// # Errors
/// Returns [`SwapEquivalenceError`] on the first probe where `p` and `q`
/// emit different canonical bytes.
pub async fn verify_provider_equivalence<P, Q>(
    p: &P,
    q: &Q,
    probes: &PolyhedralProbeSet<Observation, Vec<Action>>,
) -> Result<ProviderEquivalenceReport, SwapEquivalenceError>
where
    P: Provider,
    Q: Provider,
{
    let total = probes.len();
    let mut passed = 0_usize;
    for (i, probe) in probes.probes.iter().enumerate() {
        let p_out = p.generate(&probe.input).await.map_err(|e| {
            SwapEquivalenceError::new(
                i,
                &probe.name,
                format!("provider-p error: {e}").into_bytes(),
                Vec::new(),
            )
        })?;
        let q_out = q.generate(&probe.input).await.map_err(|e| {
            SwapEquivalenceError::new(
                i,
                &probe.name,
                Vec::new(),
                format!("provider-q error: {e}").into_bytes(),
            )
        })?;
        let p_canonical = serde_json::to_vec(&p_out).unwrap_or_default();
        let q_canonical = serde_json::to_vec(&q_out).unwrap_or_default();
        let expected_canonical = serde_json::to_vec(&probe.expected).unwrap_or_default();
        if p_canonical != q_canonical {
            return Err(SwapEquivalenceError::new(
                i,
                &probe.name,
                p_canonical,
                q_canonical,
            ));
        }
        if p_canonical != expected_canonical {
            return Err(SwapEquivalenceError::new(
                i,
                format!("{}/expected", probe.name),
                p_canonical,
                expected_canonical,
            ));
        }
        passed = passed.saturating_add(1);
    }
    Ok(ProviderEquivalenceReport { passed, total })
}

#[cfg(test)]
mod tests {
    use super::*;

    use gauss_core::{Action, ObservationSource, TaintLabel, TextAction};
    use gauss_provider::ToyProvider;

    use crate::probe::Probe;

    fn obs(channel: &str) -> Observation {
        Observation::new(
            ObservationSource::User {
                channel: channel.into(),
            },
            TaintLabel::User,
            serde_json::Value::Null,
        )
    }

    fn echo_actions(body: &str) -> Vec<Action> {
        vec![Action::Text(TextAction::new(body))]
    }

    fn echo_probes() -> PolyhedralProbeSet<Observation, Vec<Action>> {
        let mut set = PolyhedralProbeSet::default();
        set.push(Probe::new("hi", obs("a"), echo_actions("hi")));
        set.push(Probe::new("hi", obs("b"), echo_actions("hi")));
        set.push(Probe::new("hi", obs("c"), echo_actions("hi")));
        set
    }

    #[tokio::test]
    async fn two_always_text_providers_are_equivalent() {
        let p = ToyProvider::always_text("hi");
        let q = ToyProvider::always_text("hi");
        let report = verify_provider_equivalence(&p, &q, &echo_probes())
            .await
            .unwrap();
        assert!(report.ok());
        assert_eq!(report.passed, 3);
        assert!((report.divergence() - 0.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn two_diverging_providers_report_first_divergence() {
        let p = ToyProvider::always_text("hi");
        let q = ToyProvider::always_text("bye");
        let err = verify_provider_equivalence(&p, &q, &echo_probes())
            .await
            .expect_err("divergence at probe 0");
        assert_eq!(err.probe_index, 0);
        assert!(err.p_canonical_len > 0);
        assert!(err.q_canonical_len > 0);
        assert_ne!(err.p_canonical, err.q_canonical);
    }

    #[tokio::test]
    async fn report_divergence_ratio_is_zero_for_empty_probe_set() {
        let p = ToyProvider::always_text("hi");
        let q = ToyProvider::always_text("hi");
        let report = verify_provider_equivalence(&p, &q, &PolyhedralProbeSet::default())
            .await
            .unwrap();
        assert!(report.ok());
        assert!((report.divergence() - 0.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn matching_provider_disagreeing_with_expected_fails() {
        let p = ToyProvider::always_text("hi");
        let q = ToyProvider::always_text("hi");
        // Probe expects "different" — both providers say "hi" — they
        // agree with each other but not with the spec.
        let mut probes = PolyhedralProbeSet::default();
        probes.push(Probe::new("mismatch", obs("a"), echo_actions("different")));
        let err = verify_provider_equivalence(&p, &q, &probes)
            .await
            .expect_err("spec-mismatch must fail");
        assert!(err.probe_name.contains("/expected"));
    }
}
