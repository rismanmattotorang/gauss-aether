//! `gaussclaw-providers` — Phase 4 provider plane.
//!
//! Replaces the upstream Hermes `backends/*` Python catalogue with a
//! Rust catalogue where every leaf vendor driver:
//!
//! 1. Implements [`gaussclaw_agent::ProviderHandle`] — the canonical
//!    async trait the agent loop dispatches through.
//! 2. Carries a typed [`LeafModel`] declaration (id, vendor, max
//!    tokens, capability requirement, cost hints) so the kernel can
//!    intersect requirements before routing.
//! 3. Wears a [`Postconditions`] guarantee verified at every call:
//!    completion length ≤ `max_tokens`, finish reason is canonical,
//!    UTF-8 well-formed text.
//! 4. Hides its wire transport behind [`HttpBackend`] — production
//!    builds plug in a `reqwest`-based backend; tests use
//!    [`MockHttpBackend`] with deterministic per-call responses.
//!
//! ## Six Hermes-superiorities (verified by tests in this crate)
//!
//! 1. **Postcondition enforcement.** Every provider's response is
//!    schema-validated before crossing back to the agent. Hermes feeds
//!    raw provider JSON to the next prompt verbatim.
//! 2. **Capability-lower-bound for routing.** A [`Catalogue`] exposes
//!    [`Catalogue::capability_lower_bound`] — the **intersection** of
//!    every leaf's `cap_required`. The kernel admit gate must satisfy
//!    that bound before any leaf in the catalogue can be chosen.
//!    Hermes has no equivalent.
//! 3. **Backend abstraction.** [`HttpBackend`] separates wire shape
//!    (vendor-specific JSON) from transport. Swapping `reqwest` for
//!    `hyper` for `ureq` is one impl. Hermes hardcodes `requests`.
//! 4. **Vendor parity in one binary.** All twenty Hermes vendors
//!    eventually live as `gaussclaw_providers::<vendor>::Provider`
//!    types — typed, swappable, polyhedrally verifiable. Hermes lists
//!    them as separate Python files with no surface contract.
//! 5. **Cost telemetry per call.** [`Completion::usage`] is populated
//!    in every driver (token counts, dollars-per-call from manifest).
//!    Hermes returns whatever the vendor SDK chose.
//! 6. **Deterministic CI.** [`MockHttpBackend`] gives byte-stable
//!    test fixtures, so the provider plane has a reproducible
//!    conformance gate. Hermes has no provider conformance test.
//!
//! ## Reference catalogue
//!
//! Phase 4 slice 1 ships three leaf drivers covering the three wire
//! shapes the Hermes backend file lists:
//!
//! - [`anthropic::AnthropicProvider`] — Anthropic Messages API
//! - [`openai::OpenAIProvider`] — OpenAI Chat Completions API
//! - [`ollama::OllamaProvider`] — local Ollama generate API
//!
//! Follow-on slices port the remaining seventeen by plugging the
//! vendor-specific request/response codecs into the same `HttpBackend`
//! seam.

#![allow(
    clippy::doc_markdown,
    clippy::missing_docs_in_private_items,
    clippy::match_same_arms,
    clippy::option_if_let_else,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_value,
    clippy::single_match_else,
    clippy::map_unwrap_or
)]

pub mod anthropic;
pub mod backend;
pub mod catalogue;
pub mod cohere;
pub mod fallback;
pub mod google;
pub mod huggingface;
pub mod llama_cpp;
pub mod ollama;
pub mod openai;
pub mod openai_compat;
pub mod postconditions;
pub mod replicate;
pub mod router;

pub use anthropic::AnthropicProvider;
pub use backend::{HttpBackend, HttpError, HttpRequest, HttpResponse, MockHttpBackend};
pub use catalogue::{Catalogue, CostHints, LeafModel};
pub use cohere::CohereProvider;
pub use fallback::{
    AttemptRecord, FallbackBuildError, FallbackChain, FallbackChainBuilder, FallbackResult,
};
pub use google::GoogleProvider;
pub use huggingface::HuggingFaceProvider;
pub use llama_cpp::LlamaCppProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub use openai_compat::{
    anyscale, cerebras, deepseek, fireworks, groq, mistral, octoai, perplexity, tgi, together,
    vllm, xai, OpenAICompatProvider,
};
pub use postconditions::{check_postconditions, PostconditionError};
pub use replicate::ReplicateProvider;
pub use router::{RoutedCompletion, RouterProvider, RouterTransparencyError};
