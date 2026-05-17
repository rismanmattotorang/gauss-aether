//! [`FallbackChain`] — typed ordered fallback over multiple [`ProviderHandle`]s.
//!
//! ## Purpose
//!
//! Production deployments rarely depend on a single provider — they want
//! a primary, plus an ordered fallback list, plus the guarantee that a
//! fallback produces output **schema-equivalent** to the primary. Hermes
//! upstream leaves the fallback policy to user code (per-call site
//! try/except trees with no contract). GaussClaw moves it into a typed
//! builder so a misconfigured chain fails at construction time rather
//! than mid-conversation.
//!
//! ## Structural contract
//!
//! 1. **Polyhedral equivalence on the working set.** Members must agree
//!    on the schema of the response (every member's
//!    [`Completion::finish_reason`] inhabits the same canonical set,
//!    `usage` carries the same fields). The default
//!    [`FallbackChain::build`] uses
//!    [`gaussclaw_providers::check_postconditions`] as the equivalence
//!    witness — every member's mock response must pass.
//!
//! 2. **Audit per attempt.** A `FallbackChain` records each
//!    `(member_index, error)` triple it walked before succeeding. The
//!    receipt chain consumes the trace.
//!
//! 3. **No silent reordering.** The builder freezes the order at
//!    construction; runtime cannot rearrange. Hermes's per-call site
//!    fallback often has subtle ordering bugs (try/except blocks
//!    matched on exception class, not on provider).

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult,
};
use thiserror::Error;

/// One attempt record — used by the audit trail when a fallback walks
/// past a primary that failed.
#[derive(Debug, Clone)]
pub struct AttemptRecord {
    /// 0-based index in the chain (`0` = primary).
    pub index: u32,
    /// Provider name that was attempted.
    pub provider: String,
    /// Formatted error if the attempt failed; `None` if it succeeded.
    pub error: Option<String>,
}

/// Result of a fallback dispatch — the completion plus the walk record.
#[derive(Debug, Clone)]
pub struct FallbackResult {
    /// 0-based index in the chain of the member that succeeded.
    pub succeeded_at: u32,
    /// The completion.
    pub completion: Completion,
    /// Every attempt in order, including the successful one.
    pub trace: Vec<AttemptRecord>,
}

/// Builder error — the chain refused to compile.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FallbackBuildError {
    /// Empty chain (no primary).
    #[error("fallback chain must have at least one member")]
    Empty,
    /// Equivalence-witness failure.
    #[error("member {index} ({provider}) failed equivalence: {reason}")]
    EquivalenceFailed {
        /// Member index.
        index: u32,
        /// Provider name.
        provider: String,
        /// Reason text.
        reason: String,
    },
}

/// Typed fallback chain.
///
/// Use [`FallbackChain::builder`] to construct; `build()` runs the
/// equivalence check across an optional witness prompt and refuses on
/// any mismatch.
pub struct FallbackChain {
    members: Vec<Arc<dyn ProviderHandle>>,
}

impl core::fmt::Debug for FallbackChain {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FallbackChain")
            .field("len", &self.members.len())
            .field(
                "providers",
                &self
                    .members
                    .iter()
                    .map(|p| p.name())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl FallbackChain {
    /// Begin building a chain.
    #[must_use]
    pub fn builder() -> FallbackChainBuilder {
        FallbackChainBuilder::default()
    }

    /// Number of members.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the chain is empty (always false — `build()` refuses empties).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Borrow a member by index.
    pub fn member(&self, idx: usize) -> Option<&Arc<dyn ProviderHandle>> {
        self.members.get(idx)
    }

    /// Run the chain — try each member in order until one succeeds.
    ///
    /// # Errors
    /// Returns the **last** provider's error if every member fails.
    pub async fn dispatch(&self, prompt: &Prompt) -> Result<FallbackResult, ProviderError> {
        let mut trace: Vec<AttemptRecord> = Vec::with_capacity(self.members.len());
        let mut last_err: Option<ProviderError> = None;
        for (i, m) in self.members.iter().enumerate() {
            let idx = u32::try_from(i).unwrap_or(u32::MAX);
            match m.complete(prompt).await {
                Ok(c) => {
                    trace.push(AttemptRecord {
                        index: idx,
                        provider: m.name().to_string(),
                        error: None,
                    });
                    return Ok(FallbackResult {
                        succeeded_at: idx,
                        completion: c,
                        trace,
                    });
                }
                Err(e) => {
                    trace.push(AttemptRecord {
                        index: idx,
                        provider: m.name().to_string(),
                        error: Some(format!("{e}")),
                    });
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ProviderError::Transport("empty fallback chain".into())))
    }
}

/// Builder for a [`FallbackChain`].
#[derive(Default)]
pub struct FallbackChainBuilder {
    members: Vec<Arc<dyn ProviderHandle>>,
}

impl FallbackChainBuilder {
    /// Append a member.
    #[must_use]
    pub fn push(mut self, provider: Arc<dyn ProviderHandle>) -> Self {
        self.members.push(provider);
        self
    }

    /// Finalise the chain without an equivalence witness — useful when
    /// the caller has already proven equivalence (e.g. via the
    /// `gauss-poly` verifier on a probe set).
    ///
    /// # Errors
    /// Returns [`FallbackBuildError::Empty`] when no members were added.
    pub fn build_unchecked(self) -> Result<FallbackChain, FallbackBuildError> {
        if self.members.is_empty() {
            return Err(FallbackBuildError::Empty);
        }
        Ok(FallbackChain {
            members: self.members,
        })
    }

    /// Finalise the chain after running an equivalence witness:
    /// every member must produce a [`Completion`] that passes
    /// [`crate::postconditions::check_postconditions`] on the same
    /// witness prompt.
    ///
    /// # Errors
    /// Returns [`FallbackBuildError::EquivalenceFailed`] on the first
    /// member that doesn't pass; [`FallbackBuildError::Empty`] when
    /// no members were added.
    pub async fn build_with_witness(
        self,
        witness_prompt: &Prompt,
    ) -> Result<FallbackChain, FallbackBuildError> {
        if self.members.is_empty() {
            return Err(FallbackBuildError::Empty);
        }
        for (i, m) in self.members.iter().enumerate() {
            let idx = u32::try_from(i).unwrap_or(u32::MAX);
            let c = m.complete(witness_prompt).await.map_err(|e| {
                FallbackBuildError::EquivalenceFailed {
                    index: idx,
                    provider: m.name().to_string(),
                    reason: format!("witness call failed: {e}"),
                }
            })?;
            crate::postconditions::check_postconditions(&c, witness_prompt.max_tokens).map_err(
                |e| FallbackBuildError::EquivalenceFailed {
                    index: idx,
                    provider: m.name().to_string(),
                    reason: format!("{e}"),
                },
            )?;
        }
        Ok(FallbackChain {
            members: self.members,
        })
    }
}

// `ProviderHandle` adapter so a FallbackChain plugs into TurnPolicy
// wherever a single provider would.
#[async_trait]
impl ProviderHandle for FallbackChain {
    fn name(&self) -> &'static str {
        "fallback_chain"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let r = self.dispatch(prompt).await?;
        Ok(r.completion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gaussclaw_agent::{Completion, Message, TokenCount};

    /// A test provider that always succeeds with a fixed reply.
    struct OkProvider {
        name: &'static str,
    }

    #[async_trait]
    impl ProviderHandle for OkProvider {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn complete(&self, _: &Prompt) -> ProviderResult<Completion> {
            Ok(Completion::new(
                format!("{} replied", self.name),
                "x",
                "stop",
                TokenCount::new(1, 1),
            ))
        }
    }

    /// A test provider that always fails with a Transport error.
    struct ErrProvider {
        name: &'static str,
    }

    #[async_trait]
    impl ProviderHandle for ErrProvider {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn complete(&self, _: &Prompt) -> ProviderResult<Completion> {
            Err(ProviderError::Transport(format!("{} down", self.name)))
        }
    }

    fn ok_provider(name: &'static str) -> Arc<dyn ProviderHandle> {
        Arc::new(OkProvider { name })
    }

    fn err_provider(name: &'static str) -> Arc<dyn ProviderHandle> {
        Arc::new(ErrProvider { name })
    }

    fn sample_prompt() -> Prompt {
        Prompt::new("any", vec![Message::new("user", "hi")])
    }

    #[tokio::test]
    async fn builder_refuses_empty_chain() {
        let r = FallbackChain::builder().build_unchecked();
        assert!(matches!(r.unwrap_err(), FallbackBuildError::Empty));
    }

    #[tokio::test]
    async fn primary_succeeds_skips_fallback() {
        let chain = FallbackChain::builder()
            .push(ok_provider("primary"))
            .push(ok_provider("backup"))
            .build_unchecked()
            .unwrap();
        let r = chain.dispatch(&sample_prompt()).await.unwrap();
        assert_eq!(r.succeeded_at, 0);
        assert_eq!(r.trace.len(), 1);
        assert!(r.completion.text.contains("primary"));
    }

    #[tokio::test]
    async fn primary_fails_fallback_succeeds() {
        let chain = FallbackChain::builder()
            .push(err_provider("primary"))
            .push(ok_provider("backup"))
            .build_unchecked()
            .unwrap();
        let r = chain.dispatch(&sample_prompt()).await.unwrap();
        assert_eq!(r.succeeded_at, 1);
        assert_eq!(r.trace.len(), 2);
        assert!(r.trace[0].error.is_some());
        assert!(r.trace[1].error.is_none());
        assert!(r.completion.text.contains("backup"));
    }

    #[tokio::test]
    async fn every_member_fails_returns_last_error() {
        let chain = FallbackChain::builder()
            .push(err_provider("a"))
            .push(err_provider("b"))
            .build_unchecked()
            .unwrap();
        let err = chain.dispatch(&sample_prompt()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Transport(_)));
    }

    #[tokio::test]
    async fn build_with_witness_passes_on_canonical_members() {
        let chain = FallbackChain::builder()
            .push(ok_provider("a"))
            .push(ok_provider("b"))
            .build_with_witness(&sample_prompt())
            .await
            .unwrap();
        assert_eq!(chain.len(), 2);
    }

    #[tokio::test]
    async fn build_with_witness_rejects_member_that_fails_postconditions() {
        // A provider whose finish_reason is non-canonical breaks the
        // equivalence-witness check.
        struct BadFinishReason;
        #[async_trait]
        impl ProviderHandle for BadFinishReason {
            fn name(&self) -> &'static str {
                "bad"
            }
            async fn complete(&self, _: &Prompt) -> ProviderResult<Completion> {
                Ok(Completion::new(
                    "x",
                    "y",
                    "instruction_received", // not in canonical set
                    TokenCount::new(0, 0),
                ))
            }
        }
        let err = FallbackChain::builder()
            .push(ok_provider("good"))
            .push(Arc::new(BadFinishReason))
            .build_with_witness(&sample_prompt())
            .await
            .unwrap_err();
        match err {
            FallbackBuildError::EquivalenceFailed { index, .. } => assert_eq!(index, 1),
            other => panic!("expected EquivalenceFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chain_implements_provider_handle() {
        // ProviderHandle adapter — drop a FallbackChain anywhere a
        // single provider would go.
        let chain = FallbackChain::builder()
            .push(err_provider("a"))
            .push(ok_provider("b"))
            .build_unchecked()
            .unwrap();
        let c = chain.complete(&sample_prompt()).await.unwrap();
        assert!(c.text.contains('b'));
    }
}
