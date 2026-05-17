//! [`OpenRouterProvider`] — aggregator meta-router.
//!
//! Holds a [`Catalogue`] and a per-model leaf [`ProviderHandle`].
//! `complete` dispatches to the leaf matching `prompt.model`;
//! `route_complete` selects a candidate from the catalogue ∩
//! `candidates` argument and delegates.
//!
//! The first-reachable selection strategy is deliberately simple — a
//! production OpenRouter deployment swaps it with their hosted
//! routing API. The Hermes-superior win is structural, not
//! algorithmic: the router can never **drift** from the leaves, and
//! every routed call is auditable.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{Completion, Prompt, ProviderError, ProviderHandle, ProviderResult};
use gaussclaw_providers::{Catalogue, RoutedCompletion, RouterProvider};

/// Aggregator meta-router (OpenRouter-style).
pub struct OpenRouterProvider {
    catalogue: Catalogue,
    leaves: HashMap<String, Arc<dyn ProviderHandle>>,
}

impl OpenRouterProvider {
    /// Build a router. `leaves` maps fully-qualified model id to the
    /// leaf provider that handles it.
    #[must_use]
    pub fn new(catalogue: Catalogue, leaves: HashMap<String, Arc<dyn ProviderHandle>>) -> Self {
        Self { catalogue, leaves }
    }

    /// Resolve the leaf for `model_id` (returns `None` for unknown ids).
    pub fn leaf(&self, model_id: &str) -> Option<&Arc<dyn ProviderHandle>> {
        self.leaves.get(model_id)
    }
}

#[async_trait]
impl ProviderHandle for OpenRouterProvider {
    fn name(&self) -> &'static str {
        "openrouter"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        // `complete` dispatches by exact `prompt.model`. The router's
        // value-add comes from `route_complete`; direct calls are a
        // strict aggregator pass-through.
        let leaf = self.leaves.get(&prompt.model).ok_or_else(|| {
            ProviderError::UnknownModel(prompt.model.clone())
        })?;
        leaf.complete(prompt).await
    }
}

#[async_trait]
impl RouterProvider for OpenRouterProvider {
    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    async fn route_complete(
        &self,
        prompt: &Prompt,
        candidates: &[String],
    ) -> ProviderResult<RoutedCompletion> {
        // Candidate set: explicit override, or the full catalogue if
        // empty. The router must select from candidates ∩ catalogue.
        let universe: Vec<String> = if candidates.is_empty() {
            self.catalogue.ids().iter().map(|s| (*s).to_string()).collect()
        } else {
            candidates
                .iter()
                .filter(|id| self.catalogue.get(id).is_some())
                .cloned()
                .collect()
        };
        if universe.is_empty() {
            return Err(ProviderError::Transport(
                "no reachable candidates in catalogue".into(),
            ));
        }
        // Simplest strategy: first reachable that has a registered leaf.
        let selected = universe
            .iter()
            .find(|id| self.leaves.contains_key(*id))
            .ok_or_else(|| {
                ProviderError::Transport("no candidate has a registered leaf".into())
            })?
            .clone();
        // Build a routed prompt with the selected model id; the leaf
        // sees the exact prompt it would on a direct call. This is the
        // router-transparency guarantee.
        let mut routed_prompt = prompt.clone();
        routed_prompt.model = selected.clone();
        let leaf = &self.leaves[&selected];
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

    /// Inline mock leaf for tests. Returns a deterministic completion.
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

    fn sample_router() -> OpenRouterProvider {
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
        leaves.insert(
            "anthropic/claude-3.5-sonnet".into(),
            Arc::new(MockLeaf {
                model_id: "anthropic/claude-3.5-sonnet".into(),
            }),
        );
        leaves.insert(
            "openai/gpt-4o".into(),
            Arc::new(MockLeaf {
                model_id: "openai/gpt-4o".into(),
            }),
        );
        OpenRouterProvider::new(catalogue, leaves)
    }

    #[tokio::test]
    async fn complete_dispatches_by_exact_model() {
        let r = sample_router();
        let p = Prompt::new(
            "openai/gpt-4o",
            vec![Message::new("user", "hi")],
        );
        let c = r.complete(&p).await.unwrap();
        assert!(c.text.contains("openai/gpt-4o"));
    }

    #[tokio::test]
    async fn complete_unknown_model_errors() {
        let r = sample_router();
        let p = Prompt::new("rogue/m", vec![Message::new("user", "hi")]);
        let err = r.complete(&p).await.unwrap_err();
        assert!(matches!(err, ProviderError::UnknownModel(_)));
    }

    #[tokio::test]
    async fn route_complete_picks_from_candidates() {
        let r = sample_router();
        let p = Prompt::new(
            "anthropic/claude-3.5-sonnet",
            vec![Message::new("user", "hi")],
        );
        let routed = r
            .route_complete(&p, &["openai/gpt-4o".into()])
            .await
            .unwrap();
        assert_eq!(routed.selected, "openai/gpt-4o");
        assert!(routed.completion.text.contains("openai/gpt-4o"));
        assert_eq!(routed.candidate_set, vec!["openai/gpt-4o".to_string()]);
    }

    #[tokio::test]
    async fn route_complete_falls_back_to_catalogue_when_candidates_empty() {
        let r = sample_router();
        let p = Prompt::new(
            "anthropic/claude-3.5-sonnet",
            vec![Message::new("user", "hi")],
        );
        let routed = r.route_complete(&p, &[]).await.unwrap();
        // First reachable candidate. Order is catalogue insertion order.
        assert!(routed.candidate_set.len() >= 1);
    }

    #[tokio::test]
    async fn route_complete_no_reachable_candidates_errors() {
        let r = sample_router();
        let p = Prompt::new("any", vec![Message::new("user", "hi")]);
        let err = r
            .route_complete(&p, &["rogue/missing".into()])
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Transport(_)));
    }

    /// Router transparency: routing through OpenRouter vs calling the
    /// leaf directly must produce the same `text` + `finish_reason`.
    /// This is the explicit T7 contract verified by
    /// `gaussclaw_providers::router::check_transparency`.
    #[tokio::test]
    async fn router_satisfies_transparency_contract() {
        use gaussclaw_providers::router::check_transparency;
        let r = sample_router();
        let p = Prompt::new(
            "anthropic/claude-3.5-sonnet",
            vec![Message::new("user", "transparency check")],
        );
        let routed = r.route_complete(&p, &[]).await.unwrap();
        // Compute the direct-call result for the selected leaf:
        let mut leaf_prompt = p.clone();
        leaf_prompt.model = routed.selected.clone();
        let direct = r.leaf(&routed.selected).unwrap().complete(&leaf_prompt).await.unwrap();
        check_transparency(&routed, &direct, r.catalogue()).expect("transparency");
    }
}
