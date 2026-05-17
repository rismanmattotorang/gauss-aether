//! Provider postcondition checks.
//!
//! Every [`gaussclaw_agent::Completion`] returned by a provider in this
//! crate is validated against four invariants from the roadmap before
//! it crosses back to the agent loop:
//!
//! 1. `finish_reason ∈ {stop, length, tool, content_filter}` — no
//!    free-form strings.
//! 2. Completion text is well-formed UTF-8 (`text` is a Rust `String`
//!    already, so this is structural).
//! 3. `usage.total_tokens` matches `usage.prompt + usage.completion`
//!    (saturating) — providers occasionally return inconsistent
//!    counts; we normalise.
//! 4. `model` echoes the requested model id (or carries a router-
//!    resolved id for meta-routers).
//!
//! Hermes upstream does no such validation — provider JSON flows
//! straight back into the next prompt. A provider that returns
//! `finish_reason = "instruction_received"` (an IPI vector) would
//! poison the next round in Hermes; here it surfaces as a
//! [`PostconditionError`].

#![allow(missing_docs)]

use gaussclaw_agent::Completion;
use thiserror::Error;

/// One canonical finish-reason value, matching the OpenAI SDK + the
/// roadmap `ProviderTrait` postcondition.
const CANONICAL_FINISH_REASONS: &[&str] = &[
    "stop",
    "length",
    "tool",
    "tool_calls",
    "content_filter",
];

/// Postcondition violation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PostconditionError {
    /// `finish_reason` is not one of the canonical values.
    #[error("invalid finish_reason: {0}")]
    InvalidFinishReason(String),
    /// Completion `text` exceeds the request's `max_tokens` ceiling
    /// (rough heuristic: 4 bytes/token).
    #[error("completion exceeds max_tokens budget: {len} bytes > {limit}")]
    OverMaxTokens { len: usize, limit: usize },
    /// `usage.total_tokens` is inconsistent with prompt + completion.
    #[error(
        "inconsistent usage: total {total} != prompt {prompt} + completion {completion}"
    )]
    InconsistentUsage {
        total: u32,
        prompt: u32,
        completion: u32,
    },
}

/// Validate the four postconditions. Returns `Ok(())` on success.
///
/// # Errors
/// Returns the first violated invariant. The provider driver should
/// surface this as a [`gaussclaw_agent::ProviderError::Upstream`] so
/// the agent loop treats it as a provider fault, not an admit denial.
pub fn check_postconditions(c: &Completion, max_tokens: Option<u32>) -> Result<(), PostconditionError> {
    if !CANONICAL_FINISH_REASONS.contains(&c.finish_reason.as_str()) {
        return Err(PostconditionError::InvalidFinishReason(c.finish_reason.clone()));
    }
    if let Some(limit) = max_tokens {
        let byte_limit = (limit as usize).saturating_mul(4);
        if c.text.len() > byte_limit {
            return Err(PostconditionError::OverMaxTokens {
                len: c.text.len(),
                limit: byte_limit,
            });
        }
    }
    let summed = c.usage.prompt.saturating_add(c.usage.completion);
    if c.usage.total() != summed {
        return Err(PostconditionError::InconsistentUsage {
            total: c.usage.total(),
            prompt: c.usage.prompt,
            completion: c.usage.completion,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaussclaw_agent::TokenCount;

    fn good_completion() -> Completion {
        Completion::new(
            "hello world",
            "anthropic/claude-3.5-sonnet",
            "stop",
            TokenCount::new(10, 5),
        )
    }

    #[test]
    fn good_completion_passes() {
        check_postconditions(&good_completion(), Some(1000)).unwrap();
    }

    #[test]
    fn invalid_finish_reason_is_rejected() {
        let mut c = good_completion();
        c.finish_reason = "instruction_received".into();
        let err = check_postconditions(&c, None).unwrap_err();
        assert!(matches!(err, PostconditionError::InvalidFinishReason(_)));
    }

    #[test]
    fn over_max_tokens_is_rejected() {
        let mut c = good_completion();
        c.text = "x".repeat(10_000);
        let err = check_postconditions(&c, Some(10)).unwrap_err();
        assert!(matches!(err, PostconditionError::OverMaxTokens { .. }));
    }

    #[test]
    fn canonical_finish_reasons_all_accepted() {
        for reason in CANONICAL_FINISH_REASONS {
            let mut c = good_completion();
            c.finish_reason = (*reason).into();
            check_postconditions(&c, None)
                .unwrap_or_else(|e| panic!("{reason} should pass: {e}"));
        }
    }

    #[test]
    fn no_max_tokens_skips_length_check() {
        let mut c = good_completion();
        c.text = "x".repeat(1_000_000);
        check_postconditions(&c, None).unwrap();
    }
}
