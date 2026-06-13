//! Integration: the Gauss-Agent0 LinUCB router driving the live
//! `NotDiamondProvider`.
//!
//! Proves [`gaussclaw_rsi::LinUcbStrategy`] plugs into the real meta-router as
//! a `SelectionStrategy`: after the bandit is rewarded toward one model, the
//! live router dispatches there and returns that leaf's completion verbatim
//! (router-transparency).

#![allow(clippy::doc_markdown)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::CapToken;
use gaussclaw_agent::{Completion, Message, Prompt, ProviderHandle, ProviderResult, TokenCount};
use gaussclaw_providers::{Catalogue, LeafModel, RouterProvider};
use gaussclaw_providers_meta::NotDiamondProvider;
use gaussclaw_rsi::LinUcbStrategy;

/// A leaf provider that echoes its own slug, so the routed completion reveals
/// which model actually served the request.
struct NamedLeaf(&'static str);

#[async_trait]
impl ProviderHandle for NamedLeaf {
    fn name(&self) -> &str {
        self.0
    }
    async fn complete(&self, _p: &Prompt) -> ProviderResult<Completion> {
        Ok(Completion::new(
            format!("served-by:{}", self.0),
            self.0,
            "stop",
            TokenCount::new(1, 1),
        ))
    }
}

fn prompt() -> Prompt {
    Prompt::new("router", vec![Message::new("user", "route this task")])
}

#[tokio::test]
async fn linucb_strategy_routes_the_live_notdiamond_provider() {
    let slugs = ["openai/gpt-4o", "anthropic/claude-sonnet-4.5"];

    // Reward the OpenAI arm so the bandit prefers it.
    let strategy = LinUcbStrategy::new(slugs.iter().map(|s| (*s).to_owned()).collect(), 0.0, 0.0);
    for _ in 0..10 {
        strategy.reward(slugs[0], &prompt(), 1.0, 0.0, 0.0);
        strategy.reward(slugs[1], &prompt(), 0.0, 0.0, 0.0);
    }

    // Build the live meta-router over both leaves with the LinUCB strategy.
    let catalogue = Catalogue::with_models(vec![
        LeafModel::new(slugs[0], "openai", 8192, CapToken::NETWORK_POST),
        LeafModel::new(slugs[1], "anthropic", 8192, CapToken::NETWORK_POST),
    ]);
    let mut leaves: HashMap<String, Arc<dyn ProviderHandle>> = HashMap::new();
    leaves.insert(slugs[0].to_owned(), Arc::new(NamedLeaf("openai/gpt-4o")));
    leaves.insert(
        slugs[1].to_owned(),
        Arc::new(NamedLeaf("anthropic/claude-sonnet-4.5")),
    );
    let router = NotDiamondProvider::new(catalogue, leaves, Arc::new(strategy));

    let candidates: Vec<String> = slugs.iter().map(|s| (*s).to_owned()).collect();
    let decision = router
        .route_complete(&prompt(), &candidates)
        .await
        .expect("routing should succeed");

    // The bandit-preferred model was selected, and its completion came through
    // unchanged (router transparency).
    assert_eq!(decision.selected, "openai/gpt-4o");
    assert_eq!(decision.completion.text, "served-by:openai/gpt-4o");
}
