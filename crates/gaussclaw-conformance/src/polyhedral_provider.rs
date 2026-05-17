//! Polyhedral equivalence verifier for [`gaussclaw_agent::ProviderHandle`].
//!
//! Phase 4 slice 8 — the **CI gate** for the post-Hermes provider plane.
//!
//! Two [`ProviderHandle`] impls are **polyhedrally equivalent** when
//! they produce structurally-equal [`Completion`]s on every prompt in a
//! finite probe set (paper §XII.A, Theorem T7). The verifier mechanises
//! that check so CI can flag any swap-incompatible regression *before*
//! it reaches a deployment.
//!
//! Two cousin verifiers already exist:
//!
//! - [`gauss_poly::verify_provider_equivalence`] covers the older
//!   `gauss_traits::Provider` (action-vector) shape.
//! - [`gaussclaw_providers::router::check_transparency`] covers the
//!   single-call router-transparency contract on
//!   [`gaussclaw_providers::RoutedCompletion`].
//!
//! This verifier extends both to the post-Hermes `ProviderHandle` trait
//! every vendor driver in `gaussclaw-providers` implements, so a single
//! probe set can audit Anthropic ↔ OpenAI ↔ Ollama ↔ Cohere ↔ Google ↔
//! HuggingFace ↔ Replicate ↔ llama.cpp ↔ each OpenAI-compat vendor in
//! one CI step. Hermes upstream has no equivalent gate.
//!
//! ## Contract
//!
//! For each [`ProviderProbe`] in the input set, the verifier:
//!
//! 1. Calls `p.complete(&probe.prompt).await` and
//!    `q.complete(&probe.prompt).await`.
//! 2. Serialises both [`Completion`]s through `serde_json::to_vec`
//!    (canonical structural form — field ordering is fixed by serde).
//! 3. Compares the canonical bytes.
//!
//! Any divergence short-circuits with a [`HandleEquivalenceError`]
//! identifying the probe and carrying both canonical byte vectors for
//! diff display.
//!
//! The probe's optional `expected` field, when set, additionally
//! enforces that BOTH providers match a spec-supplied completion —
//! catching the "two providers agree with each other but neither
//! matches the contract" failure mode.

use gaussclaw_agent::{Completion, Prompt, ProviderHandle};
use serde_json::Value;

/// One input/expected probe for the equivalence check.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ProviderProbe {
    /// Human-readable probe name (surfaces in failure diagnostics).
    pub name: String,
    /// The prompt fed to both providers.
    pub prompt: Prompt,
    /// Optional spec-supplied completion: when present, BOTH providers
    /// must match its canonical bytes. When `None`, the verifier only
    /// requires the two providers to agree with each other.
    pub expected: Option<Completion>,
}

impl ProviderProbe {
    /// Build a new probe.
    #[must_use]
    pub fn new(name: impl Into<String>, prompt: Prompt) -> Self {
        Self {
            name: name.into(),
            prompt,
            expected: None,
        }
    }

    /// Attach a spec-supplied expected completion.
    #[must_use]
    pub fn with_expected(mut self, expected: Completion) -> Self {
        self.expected = Some(expected);
        self
    }
}

/// First-divergence report for a swap-equivalence failure.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "provider-handle polyhedral equivalence failed at probe {probe_index} ({probe_name}): \
     reason={reason}"
)]
#[non_exhaustive]
pub struct HandleEquivalenceError {
    /// 0-based position of the diverging probe.
    pub probe_index: usize,
    /// Human-readable probe name.
    pub probe_name: String,
    /// Why the probe failed (`"p-vs-q"`, `"p-vs-expected"`, or
    /// `"q-vs-expected"`, or a provider error string).
    pub reason: String,
    /// Canonical bytes from the first provider (or empty on transport error).
    pub p_canonical: Vec<u8>,
    /// Canonical bytes from the second provider (or empty on transport error).
    pub q_canonical: Vec<u8>,
}

/// Successful run report.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HandleEquivalenceReport {
    /// Number of probes that passed.
    pub passed: usize,
    /// Total probes attempted.
    pub total: usize,
}

impl HandleEquivalenceReport {
    /// True iff every probe passed.
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.passed == self.total
    }

    /// Behavioural-divergence ratio in `[0, 1]`. The Phase-4 exit gate
    /// pins this to `0.0` for any pair of providers declared
    /// swap-compatible. Empty probe sets report `0.0`.
    #[must_use]
    pub fn divergence(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let missed = self.total.saturating_sub(self.passed);
        let missed_u32 = u32::try_from(missed).unwrap_or(u32::MAX);
        let total_u32 = u32::try_from(self.total).unwrap_or(u32::MAX);
        f64::from(missed_u32) / f64::from(total_u32)
    }
}

/// Verify polyhedral equivalence of two [`ProviderHandle`]s on a probe
/// set.
///
/// Returns [`HandleEquivalenceReport`] when every probe agrees, or the
/// first [`HandleEquivalenceError`] otherwise.
///
/// # Errors
/// Returns [`HandleEquivalenceError`] on the first probe where:
///
/// - either provider's `complete()` returns a transport / upstream error;
/// - the two providers produce different canonical bytes;
/// - `probe.expected` is set and either provider disagrees with it.
pub async fn verify_handle_equivalence(
    p: &dyn ProviderHandle,
    q: &dyn ProviderHandle,
    probes: &[ProviderProbe],
) -> Result<HandleEquivalenceReport, HandleEquivalenceError> {
    let total = probes.len();
    let mut passed = 0_usize;
    for (i, probe) in probes.iter().enumerate() {
        let p_out = p.complete(&probe.prompt).await.map_err(|e| {
            HandleEquivalenceError {
                probe_index: i,
                probe_name: probe.name.clone(),
                reason: format!("provider-p error: {e}"),
                p_canonical: Vec::new(),
                q_canonical: Vec::new(),
            }
        })?;
        let q_out = q.complete(&probe.prompt).await.map_err(|e| {
            HandleEquivalenceError {
                probe_index: i,
                probe_name: probe.name.clone(),
                reason: format!("provider-q error: {e}"),
                p_canonical: Vec::new(),
                q_canonical: Vec::new(),
            }
        })?;
        let p_canonical = canonical_bytes(&p_out);
        let q_canonical = canonical_bytes(&q_out);
        if p_canonical != q_canonical {
            return Err(HandleEquivalenceError {
                probe_index: i,
                probe_name: probe.name.clone(),
                reason: "p-vs-q canonical-bytes divergence".into(),
                p_canonical,
                q_canonical,
            });
        }
        if let Some(expected) = &probe.expected {
            let expected_bytes = canonical_bytes(expected);
            if p_canonical != expected_bytes {
                return Err(HandleEquivalenceError {
                    probe_index: i,
                    probe_name: probe.name.clone(),
                    reason: "p-vs-expected canonical-bytes divergence".into(),
                    p_canonical,
                    q_canonical: expected_bytes,
                });
            }
        }
        passed = passed.saturating_add(1);
    }
    Ok(HandleEquivalenceReport { passed, total })
}

/// Canonical structural bytes for a [`Completion`]: the JSON
/// serialisation with serde's field-order discipline. Two `Completion`s
/// that produce identical bytes are observationally indistinguishable
/// for any consumer that only sees the trait surface.
///
/// The `usage` field is **excluded** from the canonical bytes —
/// equivalent providers may meter token usage differently while still
/// agreeing on the output text. The polyhedral contract is about
/// observable answer, not vendor-specific telemetry.
fn canonical_bytes(c: &Completion) -> Vec<u8> {
    let v = serde_json::json!({
        "text": c.text,
        "model": c.model,
        "finish_reason": c.finish_reason,
    });
    serde_json::to_vec(&Value::Object(v.as_object().cloned().unwrap_or_default()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaussclaw_agent::{Message, TokenCount};
    use gaussclaw_providers::{
        AnthropicProvider, OpenAIProvider,
        backend::{HttpResponse, MockHttpBackend},
    };
    use std::sync::Arc;

    fn anthropic_with(text: &str, model: &str) -> AnthropicProvider {
        let body = serde_json::json!({
            "id": "msg_x",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "model": model,
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 2},
        });
        AnthropicProvider::new(
            Arc::new(MockHttpBackend::new(vec![HttpResponse {
                status: 200,
                body: serde_json::to_vec(&body).unwrap(),
            }])),
            "anthropic-test-key",
        )
    }

    fn openai_with(text: &str, model: &str) -> OpenAIProvider {
        let body = serde_json::json!({
            "id": "cmpl_x",
            "object": "chat.completion",
            "created": 0,
            "model": model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3},
        });
        OpenAIProvider::new(
            Arc::new(MockHttpBackend::new(vec![HttpResponse {
                status: 200,
                body: serde_json::to_vec(&body).unwrap(),
            }])),
            "openai-test-key",
        )
    }

    fn probe(name: &str, model: &str) -> ProviderProbe {
        ProviderProbe::new(
            name,
            Prompt::new(model, vec![Message::new("user", "hello")]),
        )
    }

    #[tokio::test]
    async fn two_equivalent_drivers_pass() {
        // Both providers report the same model id + same text + same
        // canonical finish_reason "stop". They are polyhedrally equivalent.
        let p = anthropic_with("hi", "shared-model");
        let q = openai_with("hi", "shared-model");
        let report = verify_handle_equivalence(&p, &q, &[probe("p1", "shared-model")])
            .await
            .expect("equivalent drivers must pass");
        assert!(report.ok());
        assert_eq!(report.passed, 1);
        assert!((report.divergence() - 0.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn two_diverging_drivers_report_first_divergence() {
        // Drivers diverge on the text body — must surface as p-vs-q
        // canonical-bytes divergence at probe 0.
        let p = anthropic_with("hi", "m");
        let q = openai_with("bye", "m");
        let err = verify_handle_equivalence(&p, &q, &[probe("p1", "m")])
            .await
            .expect_err("divergence must be flagged");
        assert_eq!(err.probe_index, 0);
        assert!(err.reason.contains("p-vs-q"));
        assert!(!err.p_canonical.is_empty());
        assert!(!err.q_canonical.is_empty());
        assert_ne!(err.p_canonical, err.q_canonical);
    }

    #[tokio::test]
    async fn expected_mismatch_fails_even_when_drivers_agree() {
        // Both drivers say "hi"; spec says "expected-text" — drivers
        // agree with each other but not with the contract.
        let p = anthropic_with("hi", "m");
        let q = openai_with("hi", "m");
        let expected = Completion::new(
            "expected-text",
            "m",
            "stop",
            TokenCount::new(0, 0),
        );
        let pr = probe("p1", "m").with_expected(expected);
        let err = verify_handle_equivalence(&p, &q, &[pr])
            .await
            .expect_err("spec mismatch must fail");
        assert!(err.reason.contains("expected"));
    }

    #[tokio::test]
    async fn empty_probe_set_passes_with_zero_divergence() {
        let p = anthropic_with("hi", "m");
        let q = openai_with("hi", "m");
        let report = verify_handle_equivalence(&p, &q, &[]).await.unwrap();
        assert!(report.ok());
        assert_eq!(report.total, 0);
        assert!((report.divergence() - 0.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn divergent_finish_reasons_are_caught() {
        // Anthropic stop_reason "max_tokens" → "length"; OpenAI stays
        // on "stop". Different canonical finish_reason → diverges.
        let body_anthropic = serde_json::json!({
            "id": "msg_x", "type": "message", "role": "assistant",
            "content": [{"type": "text", "text": "hi"}],
            "model": "m", "stop_reason": "max_tokens",
            "usage": {"input_tokens": 1, "output_tokens": 2},
        });
        let p = AnthropicProvider::new(
            Arc::new(MockHttpBackend::new(vec![HttpResponse {
                status: 200,
                body: serde_json::to_vec(&body_anthropic).unwrap(),
            }])),
            "k",
        );
        let q = openai_with("hi", "m");
        let err = verify_handle_equivalence(&p, &q, &[probe("p1", "m")])
            .await
            .expect_err("divergent finish_reason must be flagged");
        assert_eq!(err.probe_index, 0);
    }

    #[tokio::test]
    async fn usage_telemetry_does_not_affect_canonical_bytes() {
        // Anthropic reports 1/2; OpenAI reports 99/99. The canonical
        // form excludes `usage`, so the two providers are still
        // equivalent on the observable text+model+finish_reason axis.
        let body_anthropic = serde_json::json!({
            "id": "msg_x", "type": "message", "role": "assistant",
            "content": [{"type": "text", "text": "hi"}],
            "model": "m", "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 2},
        });
        let body_openai = serde_json::json!({
            "id": "cmpl_x", "object": "chat.completion", "created": 0, "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 99, "completion_tokens": 99, "total_tokens": 198},
        });
        let p = AnthropicProvider::new(
            Arc::new(MockHttpBackend::new(vec![HttpResponse {
                status: 200,
                body: serde_json::to_vec(&body_anthropic).unwrap(),
            }])),
            "k",
        );
        let q = OpenAIProvider::new(
            Arc::new(MockHttpBackend::new(vec![HttpResponse {
                status: 200,
                body: serde_json::to_vec(&body_openai).unwrap(),
            }])),
            "k",
        );
        let report = verify_handle_equivalence(&p, &q, &[probe("p1", "m")])
            .await
            .unwrap();
        assert!(report.ok());
    }
}
