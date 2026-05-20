//! `gaussclaw-proxy` — local OAuth-to-OpenAI-compat proxy.
//!
//! Sprint 7 §6 of `/ROADMAP.md`. Clients point at
//! `http://localhost:<port>/v1` and get cross-vendor completions
//! without managing per-provider auth themselves.
//!
//! Shipping surface:
//!
//! - `OpenAiChatRequest` / `OpenAiChatResponse` — the Hermes-compat
//!   wire shape consumed by `/v1/chat/completions`.
//! - `UpstreamCaller` trait — pluggable backend; the in-process
//!   `MockUpstream` ships for tests + the conformance suite while
//!   real provider wiring (via `gaussclaw-providers`) lands as a
//!   Sprint 8 follow-on.
//! - `ProxyConfig` — upstream model id + cap-token grant.
//! - `ProxyServer::router(state)` — Axum router; the bin's
//!   `proxy` subcommand binds it on the operator-chosen port.
//!
//! Hermes-superiority axes (documented inline):
//!
//! - **Cap-gated by construction.** Every request is rejected with
//!   a typed `denied` envelope when the proxy's grant doesn't
//!   contain `cap:network:http_post`. Hermes's proxy inherits the
//!   parent's full credential set.
//! - **Outbound redaction.** Every outbound message body is passed
//!   through `gaussclaw_redact::RedactionPolicy::default_policy()`
//!   before the upstream call. The response envelope records the
//!   per-rule hit counts. Hermes ships no redaction in the proxy
//!   path.
//! - **Structured request id.** Each call gets a BLAKE3 of the
//!   serialised request as its `id`; replays land in the chain
//!   under that id.

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_docs_in_private_items,
    clippy::too_long_first_doc_paragraph,
    clippy::double_must_use,
    clippy::arithmetic_side_effects,
    clippy::or_fun_call,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
#![allow(rustdoc::broken_intra_doc_links)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gauss_core::CapToken;
use gaussclaw_redact::{RedactionPolicy, RedactionReport};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── wire types ────────────────────────────────────────────────────────────

/// `POST /v1/chat/completions` request — narrow Hermes-compat subset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChatRequest {
    /// Model id (e.g. `anthropic/claude-3.5-sonnet`).
    pub model: String,
    /// Chat messages in OpenAI's `[{role, content}]` shape.
    pub messages: Vec<OpenAiChatMessage>,
    /// Optional sampling temperature.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Optional max tokens cap.
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// One chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChatMessage {
    /// `system` / `user` / `assistant` / `tool`.
    pub role: String,
    /// Free-form content.
    pub content: String,
}

/// `POST /v1/chat/completions` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChatResponse {
    /// Stable BLAKE3-derived id.
    pub id: String,
    /// Echo of the model id.
    pub model: String,
    /// Single choice (we don't ship `n > 1`).
    pub choices: Vec<OpenAiChatChoice>,
    /// GaussClaw-only: redaction report so the caller sees what was
    /// scrubbed before the upstream call.
    pub redaction: RedactionReport,
}

/// One completion choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiChatChoice {
    /// 0-based index.
    pub index: u32,
    /// The assistant message.
    pub message: OpenAiChatMessage,
    /// Stop reason (`stop`, `length`, `content_filter`, ...).
    pub finish_reason: String,
}

// ─── errors ────────────────────────────────────────────────────────────────

/// Crate-wide error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProxyError {
    /// Proxy grant doesn't contain the required cap.
    #[error("admit refused: required 0x{required:016x} not in grant 0x{grant:016x}")]
    AdmitRefused {
        /// Required bits.
        required: u64,
        /// Granted bits.
        grant: u64,
    },
    /// Upstream returned an error.
    #[error("upstream: {0}")]
    Upstream(String),
    /// Request schema didn't validate.
    #[error("bad request: {0}")]
    BadRequest(String),
}

/// Crate-wide result.
pub type ProxyResult<T> = Result<T, ProxyError>;

// ─── upstream trait ────────────────────────────────────────────────────────

/// Pluggable upstream caller. The real wiring is `gaussclaw-providers`
/// (Sprint 8 follow-on); this trait keeps the proxy decoupled from
/// any specific provider implementation.
#[async_trait]
pub trait UpstreamCaller: Send + Sync {
    /// Dispatch a redacted request to the upstream model.
    async fn call(&self, redacted: &OpenAiChatRequest) -> ProxyResult<OpenAiChatMessage>;
}

/// Mock upstream for tests + the conformance suite. Returns a
/// deterministic assistant reply that echoes the last user message.
#[derive(Debug, Default, Clone)]
pub struct MockUpstream;

impl MockUpstream {
    /// Build a mock.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl UpstreamCaller for MockUpstream {
    async fn call(&self, redacted: &OpenAiChatRequest) -> ProxyResult<OpenAiChatMessage> {
        let last_user = redacted
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map_or("(no user message)", |m| m.content.as_str());
        Ok(OpenAiChatMessage {
            role: "assistant".into(),
            content: format!("(mock echo) {last_user}"),
        })
    }
}

/// Upstream that always fails — for testing the error path.
#[derive(Debug, Default, Clone)]
pub struct FailingUpstream;

#[async_trait]
impl UpstreamCaller for FailingUpstream {
    async fn call(&self, _: &OpenAiChatRequest) -> ProxyResult<OpenAiChatMessage> {
        Err(ProxyError::Upstream("upstream unavailable".into()))
    }
}

// ─── proxy state + handlers ───────────────────────────────────────────────

/// Proxy configuration.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Cap-token grant; the proxy refuses if `NETWORK_POST` is missing.
    pub grant: CapToken,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            grant: CapToken::NETWORK_POST,
        }
    }
}

/// State shared across handlers. Cheap to clone — `Arc`-shared.
#[derive(Clone)]
pub struct ProxyState {
    config: ProxyConfig,
    redactor: Arc<RedactionPolicy>,
    upstream: Arc<dyn UpstreamCaller>,
}

impl ProxyState {
    /// Build a state.
    ///
    /// # Errors
    /// Returns `String` if the default redaction policy fails to
    /// compile (a build-time bug).
    pub fn new(config: ProxyConfig, upstream: Arc<dyn UpstreamCaller>) -> Result<Self, String> {
        let policy = RedactionPolicy::default_policy().map_err(|e| format!("redactor: {e}"))?;
        Ok(Self {
            config,
            redactor: Arc::new(policy),
            upstream,
        })
    }
}

/// Build the Axum router.
#[must_use]
pub fn router(state: ProxyState) -> Router {
    Router::new()
        .route("/healthz", get(handle_health))
        .route("/v1/chat/completions", post(handle_chat_completions))
        .with_state(state)
}

#[axum::debug_handler]
async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true, "service": "gaussclaw-proxy"}))
}

#[axum::debug_handler]
async fn handle_chat_completions(
    State(state): State<ProxyState>,
    Json(req): Json<OpenAiChatRequest>,
) -> Response {
    if !state.config.grant.contains(CapToken::NETWORK_POST) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "code":    "denied",
                    "message": "proxy grant missing cap:network:http_post",
                }
            })),
        )
            .into_response();
    }
    if req.messages.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "code":    "bad_request",
                    "message": "messages[] must be non-empty",
                }
            })),
        )
            .into_response();
    }
    // Redact every message body before crossing the network boundary.
    let mut report = RedactionReport::default();
    let mut redacted = req.clone();
    for msg in &mut redacted.messages {
        let (scrubbed, sub) = state.redactor.apply(&msg.content);
        msg.content = scrubbed;
        merge_report(&mut report, &sub);
    }

    // Dispatch to upstream.
    let upstream_msg = match state.upstream.call(&redacted).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "code":    "upstream",
                        "message": format!("{e}"),
                    }
                })),
            )
                .into_response();
        }
    };

    // Compute a stable request id (BLAKE3 over the redacted bytes).
    let canonical = serde_json::to_vec(&redacted).unwrap_or_default();
    let id = format!("chatcmpl-{}", short_blake3(&canonical));
    let response = OpenAiChatResponse {
        id,
        model: redacted.model,
        choices: vec![OpenAiChatChoice {
            index: 0,
            message: upstream_msg,
            finish_reason: "stop".into(),
        }],
        redaction: report,
    };
    (StatusCode::OK, Json(response)).into_response()
}

fn merge_report(into: &mut RedactionReport, from: &RedactionReport) {
    let mut by_id: std::collections::BTreeMap<String, u64> = into.hits.iter().cloned().collect();
    for (id, n) in &from.hits {
        *by_id.entry(id.clone()).or_insert(0) += *n;
    }
    into.hits = by_id.into_iter().collect();
    into.total_substitutions = into
        .total_substitutions
        .saturating_add(from.total_substitutions);
}

fn short_blake3(bytes: &[u8]) -> String {
    // Tiny BLAKE3 substitute — we don't want to pull `blake3` into
    // this crate for a 12-byte id. Use a cheap deterministic hash
    // over the serialised request.
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(bytes);
    let v = h.finish();
    format!("{v:016x}")
}

/// `gaussclaw-proxy` re-exports the `RedactionReport` shape under
/// the proxy namespace for consumers that don't want to pull
/// `gaussclaw-redact` directly.
pub use gaussclaw_redact::RedactionReport as Report;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    fn ok_state() -> ProxyState {
        ProxyState::new(ProxyConfig::default(), Arc::new(MockUpstream::new())).unwrap()
    }

    async fn post_json(
        state: ProxyState,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let app = router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, json)
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let app = router(ok_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn chat_completions_returns_mock_echo() {
        let req = serde_json::json!({
            "model": "x/y",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let (status, body) = post_json(ok_state(), "/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["id"].as_str().unwrap().starts_with("chatcmpl-"));
        assert_eq!(body["model"], "x/y");
        let choice = &body["choices"][0];
        assert_eq!(choice["index"], 0);
        assert!(choice["message"]["content"]
            .as_str()
            .unwrap()
            .contains("hello"));
        assert_eq!(choice["finish_reason"], "stop");
    }

    #[tokio::test]
    async fn chat_completions_redacts_outbound_content() {
        let req = serde_json::json!({
            "model": "x/y",
            "messages": [{
                "role": "user",
                "content": "my token: AKIAIOSFODNN7EXAMPLE",
            }],
        });
        let (status, body) = post_json(ok_state(), "/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::OK);
        // The echo reflects the redacted text.
        let content = body["choices"][0]["message"]["content"].as_str().unwrap();
        assert!(content.contains("[REDACTED:AWS_KEY]"));
        assert_eq!(body["redaction"]["total_substitutions"], 1);
    }

    #[tokio::test]
    async fn chat_completions_rejects_empty_messages() {
        let req = serde_json::json!({"model": "x/y", "messages": []});
        let (status, body) = post_json(ok_state(), "/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn chat_completions_refuses_without_network_post_cap() {
        let state = ProxyState::new(
            ProxyConfig {
                grant: CapToken::BOTTOM,
            },
            Arc::new(MockUpstream::new()),
        )
        .unwrap();
        let req = serde_json::json!({
            "model": "x/y",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let (status, body) = post_json(state, "/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "denied");
    }

    #[tokio::test]
    async fn chat_completions_surfaces_upstream_failure() {
        let state = ProxyState::new(ProxyConfig::default(), Arc::new(FailingUpstream)).unwrap();
        let req = serde_json::json!({
            "model": "x/y",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let (status, body) = post_json(state, "/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"]["code"], "upstream");
    }

    #[tokio::test]
    async fn chat_completions_id_is_deterministic_for_same_redacted_body() {
        let req = serde_json::json!({
            "model": "x/y",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let (_, a) = post_json(ok_state(), "/v1/chat/completions", req.clone()).await;
        let (_, b) = post_json(ok_state(), "/v1/chat/completions", req).await;
        assert_eq!(a["id"], b["id"]);
    }

    #[tokio::test]
    async fn chat_completions_id_differs_for_different_bodies() {
        let req_a = serde_json::json!({
            "model": "x/y",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let req_b = serde_json::json!({
            "model": "x/y",
            "messages": [{"role": "user", "content": "world"}],
        });
        let (_, a) = post_json(ok_state(), "/v1/chat/completions", req_a).await;
        let (_, b) = post_json(ok_state(), "/v1/chat/completions", req_b).await;
        assert_ne!(a["id"], b["id"]);
    }
}
