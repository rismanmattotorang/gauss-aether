//! [`LeafModel`] + [`Catalogue`] — typed model lists for the provider plane.
//!
//! A `Catalogue` is the typed analogue of Hermes's
//! `backends/*.py` enumeration. Every entry carries:
//!
//! - a fully-qualified id (`anthropic/claude-3.5-sonnet`)
//! - the vendor short-name (`anthropic`)
//! - the model's `max_tokens` ceiling
//! - the `CapToken` the kernel must grant before the agent can
//!   dispatch to this model
//! - per-call cost hints (tokens, wall-clock, dollars)
//!
//! The kernel uses [`Catalogue::capability_lower_bound`] to compute the
//! **intersection** `⋂ K_t(mᵢ)` of every model's cap requirement —
//! that bound must be a subset of the session's grant before the
//! catalogue's router (e.g. OpenRouter / NotDiamond) is allowed to
//! choose any leaf.

use gauss_core::CapToken;
use serde::{Deserialize, Serialize};

/// Cost telemetry hints for one model.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CostHints {
    /// Estimated tokens per call (prompt + completion).
    pub tokens_per_call: u32,
    /// Estimated wall-clock latency in milliseconds.
    pub wallclock_ms: u32,
    /// Estimated cost in dollars (model + provider markup).
    pub dollars_per_call: f64,
}

/// One leaf model in a catalogue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LeafModel {
    /// Fully-qualified id (`anthropic/claude-3.5-sonnet`).
    pub id: String,
    /// Vendor short-name (`anthropic`).
    pub vendor: String,
    /// Maximum tokens the model accepts (prompt + completion ceiling).
    pub max_tokens: u32,
    /// Capability the kernel must grant before dispatch.
    pub cap_required: CapToken,
    /// Cost telemetry.
    #[serde(default)]
    pub cost: CostHints,
}

impl LeafModel {
    /// Build a leaf-model entry.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        vendor: impl Into<String>,
        max_tokens: u32,
        cap_required: CapToken,
    ) -> Self {
        Self {
            id: id.into(),
            vendor: vendor.into(),
            max_tokens,
            cap_required,
            cost: CostHints::default(),
        }
    }

    /// Attach cost hints.
    #[must_use]
    pub const fn with_cost(mut self, cost: CostHints) -> Self {
        self.cost = cost;
        self
    }
}

/// A typed model catalogue.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Catalogue {
    /// Models in canonical order.
    pub models: Vec<LeafModel>,
}

impl Catalogue {
    /// Build an empty catalogue.
    #[must_use]
    pub const fn new() -> Self {
        Self { models: Vec::new() }
    }

    /// Build from a list of models.
    #[must_use]
    pub const fn with_models(models: Vec<LeafModel>) -> Self {
        Self { models }
    }

    /// Append a model.
    pub fn push(&mut self, model: LeafModel) {
        self.models.push(model);
    }

    /// Look up a model by fully-qualified id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&LeafModel> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the catalogue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// All ids in canonical order.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.models.iter().map(|m| m.id.as_str()).collect()
    }

    /// Subset by vendor.
    #[must_use]
    pub fn by_vendor(&self, vendor: &str) -> Vec<&LeafModel> {
        self.models.iter().filter(|m| m.vendor == vendor).collect()
    }

    /// Filter to the subset whose `cap_required` is satisfied by `grant`.
    ///
    /// This is the "kernel filters M before router sees it" semantics
    /// from the roadmap: a meta-router can only choose between leaves
    /// the kernel would admit.
    #[must_use]
    pub fn filter_by_grant(&self, grant: CapToken) -> Self {
        let models = self
            .models
            .iter()
            .filter(|m| m.cap_required.bits() & grant.bits() == m.cap_required.bits())
            .cloned()
            .collect();
        Self { models }
    }

    /// **Capability lower bound** = the intersection (bit-AND) of every
    /// model's `cap_required`. Returns [`CapToken::TOP`] for the empty
    /// catalogue (vacuously true — no model means no constraint).
    ///
    /// The kernel checks `lower_bound ⊑ session_grant` before allowing
    /// the router to consider this catalogue at all. The router can
    /// still choose a model with stricter caps, but the lower bound
    /// fails fast for catalogues every leaf of which is unreachable.
    #[must_use]
    pub fn capability_lower_bound(&self) -> CapToken {
        if self.models.is_empty() {
            return CapToken::TOP;
        }
        let bits = self
            .models
            .iter()
            .fold(u64::MAX, |acc, m| acc & m.cap_required.bits());
        CapToken::from_bits(bits)
    }

    /// **Capability upper bound** = the union (bit-OR) of every
    /// model's `cap_required`. The session grant must cover this if
    /// the router is permitted to pick freely.
    #[must_use]
    pub fn capability_upper_bound(&self) -> CapToken {
        let bits = self
            .models
            .iter()
            .fold(0u64, |acc, m| acc | m.cap_required.bits());
        CapToken::from_bits(bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Catalogue {
        Catalogue::with_models(vec![
            LeafModel::new(
                "anthropic/claude-3.5-sonnet",
                "anthropic",
                200_000,
                CapToken::NETWORK_GET,
            ),
            LeafModel::new("openai/gpt-4o", "openai", 128_000, CapToken::NETWORK_GET),
            LeafModel::new(
                "ollama/llama3",
                "ollama",
                8_192,
                CapToken::from_bits(
                    CapToken::NETWORK_GET.bits() | CapToken::FILESYSTEM_READ.bits(),
                ),
            ),
        ])
    }

    #[test]
    fn get_finds_by_id() {
        let c = sample();
        assert!(c.get("anthropic/claude-3.5-sonnet").is_some());
        assert!(c.get("openai/gpt-4o").is_some());
        assert!(c.get("nope/model").is_none());
    }

    #[test]
    fn by_vendor_filters() {
        let c = sample();
        assert_eq!(c.by_vendor("anthropic").len(), 1);
        assert_eq!(c.by_vendor("nonvendor").len(), 0);
    }

    #[test]
    fn capability_lower_bound_is_intersection() {
        // anthropic + openai need only NETWORK_GET; ollama needs
        // NETWORK_GET + FILESYSTEM_READ. Intersection: NETWORK_GET.
        let c = sample();
        let lower = c.capability_lower_bound();
        assert_eq!(lower.bits(), CapToken::NETWORK_GET.bits());
    }

    #[test]
    fn capability_upper_bound_is_union() {
        let c = sample();
        let upper = c.capability_upper_bound();
        assert_eq!(
            upper.bits(),
            CapToken::NETWORK_GET.bits() | CapToken::FILESYSTEM_READ.bits()
        );
    }

    #[test]
    fn empty_catalogue_lower_bound_is_top() {
        let c = Catalogue::new();
        assert_eq!(c.capability_lower_bound().bits(), CapToken::TOP.bits());
        assert_eq!(c.capability_upper_bound().bits(), 0);
    }

    #[test]
    fn filter_by_grant_drops_unreachable_models() {
        let c = sample();
        // Grant only NETWORK_GET — ollama needs FILESYSTEM_READ too.
        let filtered = c.filter_by_grant(CapToken::NETWORK_GET);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.get("anthropic/claude-3.5-sonnet").is_some());
        assert!(filtered.get("openai/gpt-4o").is_some());
        assert!(filtered.get("ollama/llama3").is_none());
    }
}
