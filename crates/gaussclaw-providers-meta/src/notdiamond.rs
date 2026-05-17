//! [`NotDiamondProvider`] — learned router (advisory mode).
//!
//! Selects one model from a candidate set via a pluggable
//! [`SelectionStrategy`], then delegates to the corresponding leaf.
//! The "advisory" mode means the strategy decides; the actual model
//! dispatch goes through the same leaf catalogue as
//! [`OpenRouterProvider`].
//!
//! Phase 4 slice 2 ships [`FirstCandidateStrategy`] — picks the first
//! candidate that has a registered leaf. A real ML-trained
//! strategy plugs in via the [`SelectionStrategy`] trait.
//!
//! ## Hermes-superior contract
//!
//! NotDiamond's hosted router is a black box from the agent's point
//! of view in Hermes. GaussClaw's [`SelectionStrategy`] trait:
//!
//! - Makes the routing decision **testable** — every strategy is a
//!   pure function from `(prompt, candidates)` → `selected`.
//! - Records both the candidate set and the chosen model in the
//!   receipt chain via [`RoutedCompletion`]'s `candidate_set` +
//!   `selected` fields.
//! - Maintains router-transparency: a routed completion's
//!   text/finish_reason exactly matches the leaf's direct response.
//! - Lets a deployment swap strategies without changing the wire
//!   surface — the trait is the same in dev, staging, and production.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{Completion, Prompt, ProviderError, ProviderHandle, ProviderResult};
use gaussclaw_providers::{Catalogue, RoutedCompletion, RouterProvider};

/// Pure selection function over `(prompt, candidate ids)`.
pub trait SelectionStrategy: Send + Sync {
    /// Pick one id from `candidates`. Implementations MAY consult
    /// `prompt` to make a content-aware choice. Returns `None` if no
    /// candidate is usable.
    fn select(&self, prompt: &Prompt, candidates: &[String]) -> Option<String>;
}

/// Trivial strategy: returns the first candidate in the supplied
/// order. Deterministic; the simplest correct default.
#[derive(Debug, Default, Clone, Copy)]
pub struct FirstCandidateStrategy;

impl SelectionStrategy for FirstCandidateStrategy {
    fn select(&self, _prompt: &Prompt, candidates: &[String]) -> Option<String> {
        candidates.first().cloned()
    }
}

/// Learned-router meta-router with a pluggable [`SelectionStrategy`].
pub struct NotDiamondProvider {
    catalogue: Catalogue,
    leaves: HashMap<String, Arc<dyn ProviderHandle>>,
    strategy: Arc<dyn SelectionStrategy>,
}

impl NotDiamondProvider {
    /// Build with a chosen strategy.
    #[must_use]
    pub fn new(
        catalogue: Catalogue,
        leaves: HashMap<String, Arc<dyn ProviderHandle>>,
        strategy: Arc<dyn SelectionStrategy>,
    ) -> Self {
        Self {
            catalogue,
            leaves,
            strategy,
        }
    }

    /// Build with [`FirstCandidateStrategy`] (deterministic baseline).
    #[must_use]
    pub fn with_first_candidate(
        catalogue: Catalogue,
        leaves: HashMap<String, Arc<dyn ProviderHandle>>,
    ) -> Self {
        Self::new(catalogue, leaves, Arc::new(FirstCandidateStrategy))
    }
}

#[async_trait]
impl ProviderHandle for NotDiamondProvider {
    fn name(&self) -> &'static str {
        "notdiamond"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let leaf = self
            .leaves
            .get(&prompt.model)
            .ok_or_else(|| ProviderError::UnknownModel(prompt.model.clone()))?;
        leaf.complete(prompt).await
    }
}

#[async_trait]
impl RouterProvider for NotDiamondProvider {
    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    async fn route_complete(
        &self,
        prompt: &Prompt,
        candidates: &[String],
    ) -> ProviderResult<RoutedCompletion> {
        let universe: Vec<String> = if candidates.is_empty() {
            self.catalogue
                .ids()
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            candidates
                .iter()
                .filter(|id| self.catalogue.get(id).is_some() && self.leaves.contains_key(*id))
                .cloned()
                .collect()
        };
        if universe.is_empty() {
            return Err(ProviderError::Transport(
                "no reachable candidates in catalogue".into(),
            ));
        }
        let selected = self
            .strategy
            .select(prompt, &universe)
            .ok_or_else(|| ProviderError::Transport("strategy refused all candidates".into()))?;
        let leaf = self
            .leaves
            .get(&selected)
            .ok_or_else(|| ProviderError::UnknownModel(selected.clone()))?;
        let mut routed_prompt = prompt.clone();
        routed_prompt.model = selected.clone();
        let completion = leaf.complete(&routed_prompt).await?;
        Ok(RoutedCompletion::new(selected, completion, universe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use gauss_core::CapToken;
    use gaussclaw_agent::{Completion, Message, ProviderHandle, ProviderResult, TokenCount};
    use gaussclaw_providers::LeafModel;

    /// Inline mock leaf for tests.
    struct MockLeaf {
        model_id: String,
    }

    #[async_trait]
    impl ProviderHandle for MockLeaf {
        fn name(&self) -> &'static str {
            "mock"
        }
        async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
            Ok(Completion::new(
                format!("(mock {} replied)", self.model_id),
                prompt.model.clone(),
                "stop",
                TokenCount::new(1, 1),
            ))
        }
    }

    /// Content-aware strategy: prefers `openai/*` when the prompt
    /// mentions "code", else falls back to first.
    struct CodeAffineStrategy;

    impl SelectionStrategy for CodeAffineStrategy {
        fn select(&self, prompt: &Prompt, candidates: &[String]) -> Option<String> {
            let last = prompt
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.content.as_str())
                .unwrap_or("");
            if last.contains("code") {
                candidates
                    .iter()
                    .find(|c| c.starts_with("openai/"))
                    .cloned()
            } else {
                candidates.first().cloned()
            }
        }
    }

    fn sample_router(strategy: Arc<dyn SelectionStrategy>) -> NotDiamondProvider {
        let catalogue = Catalogue::with_models(vec![
            LeafModel::new(
                "anthropic/claude-3.5-sonnet",
                "anthropic",
                200_000,
                CapToken::NETWORK_GET,
            ),
            LeafModel::new("openai/gpt-4o", "openai", 128_000, CapToken::NETWORK_GET),
        ]);
        let mut leaves: HashMap<String, Arc<dyn ProviderHandle>> = HashMap::new();
        for id in catalogue.ids() {
            leaves.insert(
                id.to_string(),
                Arc::new(MockLeaf {
                    model_id: id.to_string(),
                }),
            );
        }
        NotDiamondProvider::new(catalogue, leaves, strategy)
    }

    #[tokio::test]
    async fn first_candidate_strategy_picks_in_order() {
        let r = sample_router(Arc::new(FirstCandidateStrategy));
        let p = Prompt::new(
            "anthropic/claude-3.5-sonnet",
            vec![Message::new("user", "hi")],
        );
        let routed = r
            .route_complete(
                &p,
                &["openai/gpt-4o".into(), "anthropic/claude-3.5-sonnet".into()],
            )
            .await
            .unwrap();
        assert_eq!(routed.selected, "openai/gpt-4o");
    }

    #[tokio::test]
    async fn content_aware_strategy_dispatches_differently_on_content() {
        let r = sample_router(Arc::new(CodeAffineStrategy));
        let p_code = Prompt::new("any", vec![Message::new("user", "please write some code")]);
        let routed_code = r.route_complete(&p_code, &[]).await.unwrap();
        assert!(
            routed_code.selected.starts_with("openai/"),
            "code prompt should route to openai/* (got {})",
            routed_code.selected
        );

        let p_other = Prompt::new("any", vec![Message::new("user", "say hi")]);
        let routed_other = r.route_complete(&p_other, &[]).await.unwrap();
        assert_eq!(routed_other.selected, "anthropic/claude-3.5-sonnet");
    }

    #[tokio::test]
    async fn empty_universe_errors() {
        let r = sample_router(Arc::new(FirstCandidateStrategy));
        let p = Prompt::new("x", vec![Message::new("user", "hi")]);
        let err = r
            .route_complete(&p, &["nope/missing".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Transport(_)));
    }

    #[tokio::test]
    async fn router_satisfies_transparency_contract() {
        use gaussclaw_providers::router::check_transparency;
        let r = sample_router(Arc::new(FirstCandidateStrategy));
        let p = Prompt::new("any", vec![Message::new("user", "transparency check")]);
        let routed = r.route_complete(&p, &[]).await.unwrap();
        let mut leaf_prompt = p.clone();
        leaf_prompt.model = routed.selected.clone();
        let leaf = r.leaves.get(&routed.selected).unwrap();
        let direct = leaf.complete(&leaf_prompt).await.unwrap();
        check_transparency(&routed, &direct, r.catalogue())
            .expect("NotDiamond must satisfy router-transparency");
    }
}
