//! [`RouterProvider`] super-trait and the router-transparency contract.
//!
//! Meta-routers (OpenRouter aggregator, NotDiamond learned router)
//! implement [`RouterProvider`] on top of [`gaussclaw_agent::ProviderHandle`].
//! The trait carries one additional postcondition the kernel and the
//! receipt chain rely on:
//!
//! > **Router transparency.** For any prompt P and any leaf m chosen
//! > from `router.catalogue()`, calling `route_complete(P, [m]) → c`
//! > must produce a completion whose **output schema** matches calling
//! > `complete(m, P)` directly.
//!
//! The router may *select* between leaves and *augment* metadata
//! (e.g. it can stamp the actual model id into the routed completion),
//! but it MUST NOT silently transform the completion text or alter the
//! `finish_reason` / `usage` shape.

use async_trait::async_trait;
use gaussclaw_agent::{Completion, Prompt, ProviderHandle, ProviderResult};
use thiserror::Error;

use crate::Catalogue;

/// Result returned by [`RouterProvider::route_complete`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RoutedCompletion {
    /// The leaf model the router selected.
    pub selected: String,
    /// The completion as if `complete` had been called on the leaf
    /// directly — router-transparency guarantee.
    pub completion: Completion,
    /// The candidate set the router was given (or the full catalogue
    /// when no candidates were specified). Recorded in the receipt
    /// chain so the routing decision is auditable.
    pub candidate_set: Vec<String>,
}

impl RoutedCompletion {
    /// Build a routed-completion record. Required because the struct
    /// is `#[non_exhaustive]`.
    #[must_use]
    pub fn new(
        selected: impl Into<String>,
        completion: Completion,
        candidate_set: Vec<String>,
    ) -> Self {
        Self {
            selected: selected.into(),
            completion,
            candidate_set,
        }
    }
}

/// Router-transparency check failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RouterTransparencyError {
    /// Selected model is not in `router.catalogue()`.
    #[error("selected model `{0}` not in router catalogue")]
    SelectedOutsideCatalogue(String),
    /// Routed completion's text differs from the direct completion.
    #[error("routed completion text differs from direct call to leaf `{0}`")]
    TextDiverged(String),
    /// Routed completion's finish_reason differs.
    #[error("routed completion finish_reason differs from direct call to leaf `{0}`")]
    FinishReasonDiverged(String),
}

/// Meta-router super-trait.
#[async_trait]
pub trait RouterProvider: ProviderHandle {
    /// Read-only view of the leaf catalogue.
    fn catalogue(&self) -> &Catalogue;

    /// Run a routed completion. `candidates`, when non-empty, restricts
    /// the router's choice to that subset of the catalogue.
    ///
    /// # Errors
    /// Returns [`ProviderError::Upstream`] if no candidate is reachable.
    async fn route_complete(
        &self,
        prompt: &Prompt,
        candidates: &[String],
    ) -> ProviderResult<RoutedCompletion>;
}

/// Run the router-transparency check against an external transparency
/// witness: the leaf provider's direct response on the same prompt.
///
/// Production deployments call this from `gaussclaw-conformance` at
/// CI time to guarantee a router doesn't drift from its leaves.
///
/// # Errors
/// Returns the first transparency violation detected.
pub fn check_transparency(
    routed: &RoutedCompletion,
    direct: &Completion,
    catalogue: &Catalogue,
) -> Result<(), RouterTransparencyError> {
    if catalogue.get(&routed.selected).is_none() {
        return Err(RouterTransparencyError::SelectedOutsideCatalogue(
            routed.selected.clone(),
        ));
    }
    if routed.completion.text != direct.text {
        return Err(RouterTransparencyError::TextDiverged(routed.selected.clone()));
    }
    if routed.completion.finish_reason != direct.finish_reason {
        return Err(RouterTransparencyError::FinishReasonDiverged(
            routed.selected.clone(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LeafModel;
    use gauss_core::CapToken;
    use gaussclaw_agent::{Completion, TokenCount};

    fn sample_catalogue() -> Catalogue {
        Catalogue::with_models(vec![LeafModel::new(
            "openai/gpt-4o",
            "openai",
            128_000,
            CapToken::NETWORK_GET,
        )])
    }

    fn comp(text: &str) -> Completion {
        Completion::new(text, "openai/gpt-4o", "stop", TokenCount::new(1, 1))
    }

    #[test]
    fn transparent_router_passes() {
        let routed = RoutedCompletion {
            selected: "openai/gpt-4o".into(),
            completion: comp("hello"),
            candidate_set: vec!["openai/gpt-4o".into()],
        };
        let direct = comp("hello");
        check_transparency(&routed, &direct, &sample_catalogue()).unwrap();
    }

    #[test]
    fn router_selecting_outside_catalogue_rejected() {
        let routed = RoutedCompletion {
            selected: "rogue/m".into(),
            completion: comp("hello"),
            candidate_set: vec![],
        };
        let direct = comp("hello");
        let err = check_transparency(&routed, &direct, &sample_catalogue()).unwrap_err();
        assert!(matches!(
            err,
            RouterTransparencyError::SelectedOutsideCatalogue(_)
        ));
    }

    #[test]
    fn diverged_text_rejected() {
        let routed = RoutedCompletion {
            selected: "openai/gpt-4o".into(),
            completion: comp("hello"),
            candidate_set: vec![],
        };
        let direct = comp("goodbye");
        let err = check_transparency(&routed, &direct, &sample_catalogue()).unwrap_err();
        assert!(matches!(err, RouterTransparencyError::TextDiverged(_)));
    }

    #[test]
    fn diverged_finish_reason_rejected() {
        let routed = RoutedCompletion {
            selected: "openai/gpt-4o".into(),
            completion: comp("hello"),
            candidate_set: vec![],
        };
        let mut direct = comp("hello");
        direct.finish_reason = "length".into();
        let err = check_transparency(&routed, &direct, &sample_catalogue()).unwrap_err();
        assert!(matches!(
            err,
            RouterTransparencyError::FinishReasonDiverged(_)
        ));
    }
}
