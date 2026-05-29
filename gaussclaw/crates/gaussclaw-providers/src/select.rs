//! Config-driven vendor codec selection.
//!
//! [`pick_provider`] reads a [`gaussclaw_agent::ProviderHandle`]-shaped
//! choice out of a small typed [`ProviderChoice`] struct (which the bin
//! populates from `gaussclaw-config::ProviderConfig`) and returns the
//! right vendor codec. The full `gaussclaw-config` dependency is left
//! to the caller so this module stays as light as possible.
//!
//! ## What's wired today
//!
//! - `anthropic` → [`AnthropicProvider`]
//! - `openai` → [`OpenAIProvider`]
//! - anything else (or empty) → [`gaussclaw_agent::EchoProvider`]
//!
//! ## The HTTP backend story
//!
//! Callers attach the transport via [`ProviderChoice::with_backend`].
//! The production transport is `gaussclaw_http::ReqwestProviderBackend`
//! (a `reqwest`/rustls client); `gaussclaw-bin` builds it once and
//! threads it through every codec.
//!
//! When [`ProviderChoice::backend`] is left `None` — e.g. a build with
//! no transport, or a unit test that doesn't care — the codec is still
//! constructed and typed correctly, but wraps the built-in
//! [`UnconfiguredBackend`], which returns a deterministic
//! `HttpError::Network("backend not configured")` on every send. The
//! dashboard renders that as a clean error frame rather than silently
//! returning a stub echo. Tests inject a [`MockHttpBackend`] instead.

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{EchoProvider, ProviderHandle};

use crate::backend::{HttpBackend, HttpError, HttpRequest, HttpResponse};
use crate::{AnthropicProvider, OpenAIProvider};

/// Caller-supplied subset of `gaussclaw_config::ProviderConfig`.
/// Kept narrow so this crate doesn't depend on `gaussclaw-config`.
#[derive(Clone)]
#[non_exhaustive]
pub struct ProviderChoice {
    /// Lowercase vendor id (`"anthropic"`, `"openai"`, …) or empty
    /// when no vendor is configured.
    pub name: String,
    /// API key (typically sourced from an env var by the caller).
    /// Empty when no key is available.
    pub api_key: String,
    /// Optional HTTP transport. `None` selects the built-in
    /// [`UnconfiguredBackend`] (every send returns
    /// `HttpError::Network("backend not configured")`).
    pub backend: Option<Arc<dyn HttpBackend>>,
}

impl ProviderChoice {
    /// Build a minimal choice from a vendor id; useful for callers
    /// that don't have an api_key or backend yet.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            api_key: String::new(),
            backend: None,
        }
    }

    /// Builder: set the API key.
    #[must_use]
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = key.into();
        self
    }

    /// Builder: attach an HTTP backend.
    #[must_use]
    pub fn with_backend(mut self, backend: Arc<dyn HttpBackend>) -> Self {
        self.backend = Some(backend);
        self
    }
}

/// Returned with the provider so callers can surface the actual
/// selected vendor in their status payload / log output. Avoids
/// re-querying `ProviderHandle::name()` (which is `&'static str` on
/// some impls and only valid for the lifetime of the handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PickedProvider {
    /// Wired to an [`AnthropicProvider`].
    Anthropic,
    /// Wired to an [`OpenAIProvider`].
    OpenAI,
    /// Fell back to [`gaussclaw_agent::EchoProvider`] — either the
    /// config didn't name a vendor or the name wasn't recognised.
    Echo,
}

impl PickedProvider {
    /// Stable string id: `"anthropic"` / `"openai"` / `"echo"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::Echo => "echo",
        }
    }
}

/// Select a vendor codec based on [`ProviderChoice::name`].
///
/// Returns `(Arc<dyn ProviderHandle>, PickedProvider)` so the caller
/// can surface the selected vendor in status payloads + audit-log
/// rows without having to introspect the trait object.
#[must_use]
pub fn pick_provider(choice: &ProviderChoice) -> (Arc<dyn ProviderHandle>, PickedProvider) {
    let backend = choice
        .backend
        .clone()
        .unwrap_or_else(|| Arc::new(UnconfiguredBackend));
    let lower = choice.name.to_ascii_lowercase();
    match lower.as_str() {
        "anthropic" => {
            let p = AnthropicProvider::new(backend, choice.api_key.clone());
            (Arc::new(p), PickedProvider::Anthropic)
        }
        "openai" => {
            let p = OpenAIProvider::new(backend, choice.api_key.clone());
            (Arc::new(p), PickedProvider::OpenAI)
        }
        _ => (Arc::new(EchoProvider::default()), PickedProvider::Echo),
    }
}

// ─── UnconfiguredBackend ──────────────────────────────────────────────────

/// Built-in [`HttpBackend`] that fails closed on every send.
///
/// Used when [`ProviderChoice::backend`] is `None` — the vendor codec
/// is still constructed, the wire shape is still validated at compile
/// time, but every actual request returns a deterministic transport
/// error. The dashboard's chat path surfaces this as an `error` frame
/// instead of falling back to a stub echo.
///
/// Production deployments replace `None` with an `Arc<dyn HttpBackend>`
/// pointing at a real transport (typically `reqwest`-backed) once the
/// workspace grows one.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnconfiguredBackend;

#[async_trait]
impl HttpBackend for UnconfiguredBackend {
    async fn send(&self, _req: HttpRequest) -> Result<HttpResponse, HttpError> {
        Err(HttpError::Network(
            "HttpBackend not configured for this build; \
             vendor codec is wired but the transport layer is absent. \
             Plumb an Arc<dyn HttpBackend> through `ProviderChoice::with_backend`."
                .into(),
        ))
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockHttpBackend;
    use gaussclaw_agent::{Message, Prompt};

    #[tokio::test]
    async fn pick_anthropic_returns_anthropic_provider() {
        let choice = ProviderChoice::new("anthropic").with_api_key("sk-x");
        let (p, kind) = pick_provider(&choice);
        assert_eq!(kind, PickedProvider::Anthropic);
        assert_eq!(p.name(), "anthropic");
    }

    #[tokio::test]
    async fn pick_openai_returns_openai_provider() {
        let choice = ProviderChoice::new("openai").with_api_key("sk-y");
        let (p, kind) = pick_provider(&choice);
        assert_eq!(kind, PickedProvider::OpenAI);
        assert_eq!(p.name(), "openai");
    }

    #[tokio::test]
    async fn name_is_case_insensitive() {
        let choice = ProviderChoice::new("ANTHROPIC").with_api_key("sk");
        let (_, kind) = pick_provider(&choice);
        assert_eq!(kind, PickedProvider::Anthropic);
    }

    #[tokio::test]
    async fn empty_name_falls_back_to_echo() {
        let choice = ProviderChoice::new("");
        let (_, kind) = pick_provider(&choice);
        assert_eq!(kind, PickedProvider::Echo);
    }

    #[tokio::test]
    async fn unknown_name_falls_back_to_echo() {
        let choice = ProviderChoice::new("not-a-vendor");
        let (_, kind) = pick_provider(&choice);
        assert_eq!(kind, PickedProvider::Echo);
    }

    #[tokio::test]
    async fn unconfigured_backend_fails_with_transport_error() {
        let b = UnconfiguredBackend;
        let req = HttpRequest {
            url: "https://example.test".into(),
            method: "POST".into(),
            headers: vec![],
            body: vec![],
        };
        let err = b.send(req).await.unwrap_err();
        match err {
            HttpError::Network(msg) => assert!(msg.contains("not configured")),
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    /// With the unconfigured backend, the picked provider is reachable
    /// and the call shape is correct — but the wire fails with a clean
    /// transport error rather than a silent stub. Confirms the "wired
    /// but unconfigured" contract.
    #[tokio::test]
    async fn picked_provider_with_default_backend_fails_at_send() {
        let choice = ProviderChoice::new("anthropic").with_api_key("sk-x");
        let (p, _) = pick_provider(&choice);
        let prompt = Prompt::new(
            "claude-3.5-sonnet",
            vec![Message::new("user", "hello".to_string())],
        );
        let err = p.complete(&prompt).await.unwrap_err();
        // The provider surfaces transport errors as ProviderError::Transport.
        match err {
            gaussclaw_agent::ProviderError::Transport(msg) => {
                assert!(msg.contains("not configured"));
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    /// With a real (mock) backend attached, the picked provider works
    /// end-to-end. This is the production wiring path validated.
    #[tokio::test]
    async fn picked_provider_with_explicit_backend_completes() {
        use serde_json::json;
        let body = json!({
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 },
        });
        let mock = Arc::new(MockHttpBackend::new(vec![HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }]));
        let choice = ProviderChoice::new("anthropic")
            .with_api_key("sk-x")
            .with_backend(mock);
        let (p, _) = pick_provider(&choice);
        let prompt = Prompt::new(
            "claude-3.5-sonnet",
            vec![Message::new("user", "hi".to_string())],
        );
        let c = p.complete(&prompt).await.expect("complete");
        assert_eq!(c.text, "ok");
    }

    #[test]
    fn picked_provider_as_str_is_stable() {
        assert_eq!(PickedProvider::Anthropic.as_str(), "anthropic");
        assert_eq!(PickedProvider::OpenAI.as_str(), "openai");
        assert_eq!(PickedProvider::Echo.as_str(), "echo");
    }
}
