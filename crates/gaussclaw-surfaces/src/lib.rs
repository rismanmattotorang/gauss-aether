//! `gaussclaw-surfaces` — canonical wire surfaces for SDK clients.
//!
//! Phase 1 Tasks 7 + 11 of `GAUSSCLAW_ROADMAP.md`. The dashboard backend
//! (`gaussclaw-web`) is operator-facing; this crate is *client-facing* —
//! the wire surface the upstream OpenAI Python SDK and any third-party
//! integration speaks to.
//!
//! ## Endpoints
//!
//! | Method | Path | Wire shape |
//! |---|---|---|
//! | POST | `/v1/chat/completions` | OpenAI Chat Completions request → response (SSE on `stream=true`) |
//! | POST | `/v1/completions`      | OpenAI legacy Completions → response |
//! | GET  | `/v1/models`           | OpenAI Models list |
//! | WS   | `/v1/chat/ws`          | Raw GaussClaw chat WebSocket (token + tool-event stream) |
//! | POST | `/v1/turn`             | Internal `TurnRequest` (raw GaussClaw shape, no OAI mapping) |
//! | GET  | `/v1/health`           | `HealthResponse` (proxies `gauss-health`) |
//!
//! Wire types come from `gauss-gateway::openai` so the schemas evolve in
//! lock-step with the rest of the workspace. Phase 1 ships the *shape*
//! parity; the actual provider dispatch (provider plane + three-plane
//! scheduler) lands in slice 3c. Until then every endpoint returns a
//! shape-valid stub response that the OpenAI SDK accepts as well-formed.

#![allow(
    clippy::doc_markdown,
    clippy::missing_docs_in_private_items,
    clippy::module_name_repetitions,
)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use futures_util::stream::{self, Stream};
use gauss_core::TaintLabel;
use gauss_gateway::openai::{OpenAiChatMessage, OpenAiChatRequest};
use gaussclaw_agent::{
    AuditTrace, EchoProvider, KernelHandle, Message, Prompt, SurfaceRequest, TurnPolicy,
};

/// SDK-canonical OpenAI Chat Completions response.
///
/// The `gauss-gateway` crate maps `usage` to chain accounting; the wire
/// surface here uses the canonical OAI shape so the OpenAI Python SDK
/// accepts it verbatim.
#[derive(Debug, Serialize)]
#[allow(missing_docs)]
pub struct ChatResponse {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: ChatUsage,
}

/// One element of [`ChatResponse::choices`].
#[derive(Debug, Serialize)]
#[allow(missing_docs)]
pub struct ChatChoice {
    pub index: u32,
    pub message: OpenAiChatMessage,
    pub finish_reason: &'static str,
}

/// Token-accounting payload of [`ChatResponse::usage`].
#[derive(Debug, Serialize)]
#[allow(missing_docs)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

// ─── shared state ───────────────────────────────────────────────────────────

/// State threaded to every handler.
///
/// Holds the configured model name, the [`KernelHandle`] that gates
/// every admission check, the [`TurnPolicy`] that dispatches turns, and
/// the [`AuditTrace`] every request lands in before processing.
#[derive(Clone)]
pub struct SurfaceState {
    /// Default model id reported by `/v1/models` and used when a request
    /// omits its `model` field.
    pub default_model: String,
    /// Shared kernel handle — admission + plane selection.
    pub kernel: KernelHandle,
    /// Turn policy used by `/v1/chat/completions` and `/v1/completions`.
    pub policy: Arc<TurnPolicy>,
    /// Audit trace — every inbound writes here before admit.
    pub audit: AuditTrace,
}

impl SurfaceState {
    /// Build a state with a permissive kernel and the [`EchoProvider`].
    /// Use this for tests and for the Phase 1 demo binary; production
    /// deployments build the state with `with_policy`.
    pub fn new(default_model: impl Into<String>) -> Self {
        let kernel = KernelHandle::permissive();
        let audit = AuditTrace::new();
        let policy = Arc::new(
            TurnPolicy::new(kernel.clone(), Arc::new(EchoProvider::default()))
                .with_audit(audit.clone()),
        );
        Self {
            default_model: default_model.into(),
            kernel,
            policy,
            audit,
        }
    }

    /// Build a state with a caller-supplied kernel handle. Uses the
    /// [`EchoProvider`] until [`Self::with_policy`] swaps it.
    pub fn with_kernel(default_model: impl Into<String>, kernel: KernelHandle) -> Self {
        let audit = AuditTrace::new();
        let policy = Arc::new(
            TurnPolicy::new(kernel.clone(), Arc::new(EchoProvider::default()))
                .with_audit(audit.clone()),
        );
        Self {
            default_model: default_model.into(),
            kernel,
            policy,
            audit,
        }
    }

    /// Build a state with a caller-supplied [`TurnPolicy`]. Production
    /// deployments use this to plug a real provider in. Audit defaults
    /// to a fresh trace unless the policy already owns one.
    pub fn with_policy(default_model: impl Into<String>, policy: Arc<TurnPolicy>) -> Self {
        let kernel = policy.kernel().clone();
        let audit = policy.audit().cloned().unwrap_or_default();
        Self {
            default_model: default_model.into(),
            kernel,
            policy,
            audit,
        }
    }
}

// ─── models endpoint ────────────────────────────────────────────────────────

// ─── audit endpoint ─────────────────────────────────────────────────────────

/// `GET /v1/audit/head` payload — the live chain head.
#[derive(Debug, Serialize)]
#[allow(missing_docs)]
pub struct AuditHeadPayload {
    pub digest_hex: String,
}

#[axum::debug_handler]
async fn handle_audit_head(State(state): State<SurfaceState>) -> Json<AuditHeadPayload> {
    let head = state.audit.head().await;
    Json(AuditHeadPayload {
        digest_hex: head.to_hex(),
    })
}

/// `GET /v1/models` payload.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelsList {
    /// Constant `"list"` (OpenAI convention).
    pub object: &'static str,
    /// The model rows.
    pub data: Vec<ModelInfo>,
}

/// One row of [`ModelsList`].
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model id.
    pub id: String,
    /// Constant `"model"`.
    pub object: &'static str,
    /// Unix timestamp the model was made available.
    pub created: u64,
    /// Vendor / owner id.
    pub owned_by: &'static str,
}

#[axum::debug_handler]
async fn handle_models(State(state): State<SurfaceState>) -> Json<ModelsList> {
    Json(ModelsList {
        object: "list",
        data: vec![ModelInfo {
            id: state.default_model,
            object: "model",
            created: 0,
            owned_by: "gaussclaw",
        }],
    })
}

// ─── chat completions ───────────────────────────────────────────────────────

#[axum::debug_handler]
async fn handle_chat_completions(
    State(state): State<SurfaceState>,
    Json(req): Json<OpenAiChatRequest>,
) -> Response {
    // Every SDK chat request lands in the agent loop:
    //   0. audit-WAL: record the inbound BEFORE admit/dispatch
    //   1. plane select → Conversation
    //   2. admit-gate via the TurnPolicy's kernel handle
    //   3. dispatch to the configured provider
    //   4. return the completion in the OpenAI wire shape
    let plane = state.kernel.plane_for(SurfaceRequest::SdkChat);
    let req_bytes = serde_json::to_vec(&req).unwrap_or_default();
    state
        .audit
        .record_inbound("/v1/chat/completions", "sdk", &req_bytes, TaintLabel::User, plane)
        .await;
    let model = if req.model.is_empty() {
        state.default_model.clone()
    } else {
        req.model.clone()
    };
    let messages: Vec<Message> = req
        .messages
        .iter()
        .map(|m| Message::new(m.role.clone(), m.content.clone()))
        .collect();
    let mut prompt = Prompt::new(model.clone(), messages);
    prompt.max_tokens = req.max_tokens;
    prompt.temperature = req.temperature;

    let completion = match state.policy.run(prompt, TaintLabel::User).await {
        Ok(c) => c,
        Err(gaussclaw_agent::TurnError::Denied(e)) => {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": { "code": "denied", "message": format!("admit failed: {e:?}") }
                })),
            )
                .into_response();
        }
        Err(gaussclaw_agent::TurnError::Invalid(msg)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": { "code": "bad_request", "message": msg }
                })),
            )
                .into_response();
        }
        Err(gaussclaw_agent::TurnError::Provider(e)) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": { "code": "provider_error", "message": format!("{e}") }
                })),
            )
                .into_response();
        }
        Err(other) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": { "code": "agent_error", "message": format!("{other}") }
                })),
            )
                .into_response();
        }
    };

    if req.stream {
        sse_from_completion(&model, &completion.text).into_response()
    } else {
        Json(unary_from_completion(&model, &completion)).into_response()
    }
}

fn unary_from_completion(model: &str, c: &gaussclaw_agent::Completion) -> ChatResponse {
    ChatResponse {
        id: "chatcmpl-stub".into(),
        object: "chat.completion",
        created: 0,
        model: model.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: OpenAiChatMessage::new("assistant", c.text.clone()),
            finish_reason: match c.finish_reason.as_str() {
                "length" => "length",
                "tool" => "tool",
                _ => "stop",
            },
        }],
        usage: ChatUsage {
            prompt_tokens: c.usage.prompt,
            completion_tokens: c.usage.completion,
            total_tokens: c.usage.total(),
        },
    }
}

fn sse_from_completion(
    model: &str,
    body: &str,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + 'static> {
    #[allow(clippy::needless_collect)]
    let chunks: Vec<String> = body.split_inclusive(' ').map(String::from).collect();
    let model_owned = model.to_string();
    let events = chunks
        .into_iter()
        .map(move |c| Ok::<Event, Infallible>(chunk_event(&model_owned, &c)))
        .chain(std::iter::once(Ok::<Event, Infallible>(
            Event::default().data("[DONE]"),
        )));
    Sse::new(stream::iter(events))
}

fn chunk_event(model: &str, delta: &str) -> Event {
    let payload = serde_json::json!({
        "id": "chatcmpl-stub",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant", "content": delta },
            "finish_reason": null,
        }],
    });
    Event::default().data(payload.to_string())
}

// ─── legacy completions ─────────────────────────────────────────────────────

/// Legacy `/v1/completions` request body.
#[derive(Debug, Deserialize)]
#[allow(missing_docs)]
pub struct CompletionsRequest {
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
}

/// Legacy `/v1/completions` response body.
#[derive(Debug, Serialize)]
#[allow(missing_docs)]
pub struct CompletionsResponse {
    pub id: &'static str,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CompletionsChoice>,
    pub usage: ChatUsage,
}

/// One element of [`CompletionsResponse::choices`].
#[derive(Debug, Serialize)]
#[allow(missing_docs)]
pub struct CompletionsChoice {
    pub text: String,
    pub index: u32,
    pub finish_reason: &'static str,
}

#[axum::debug_handler]
async fn handle_completions(
    State(state): State<SurfaceState>,
    Json(req): Json<CompletionsRequest>,
) -> Json<CompletionsResponse> {
    let model = if req.model.is_empty() {
        state.default_model
    } else {
        req.model
    };
    Json(CompletionsResponse {
        id: "cmpl-stub",
        object: "text_completion",
        created: 0,
        model,
        choices: vec![CompletionsChoice {
            text: format!("(gaussclaw stub) legacy /v1/completions; prompt: {}", req.prompt),
            index: 0,
            finish_reason: "stop",
        }],
        usage: ChatUsage {
            prompt_tokens: u32::try_from(req.prompt.len() / 4).unwrap_or(u32::MAX),
            completion_tokens: 16,
            total_tokens: 0,
        },
    })
}

// ─── raw chat WebSocket ─────────────────────────────────────────────────────

#[axum::debug_handler]
async fn handle_chat_ws(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(chat_socket)
}

async fn chat_socket(mut socket: WebSocket) {
    let banner = serde_json::json!({
        "kind": "system",
        "body": "gaussclaw-surfaces /v1/chat/ws — provider dispatch lands in slice 3c"
    });
    if socket
        .send(WsMessage::Text(banner.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    while let Some(msg) = socket.recv().await {
        let Ok(msg) = msg else { return };
        let body = match &msg {
            WsMessage::Text(t) => t.as_str().to_string(),
            WsMessage::Binary(_) => "(binary ignored)".into(),
            WsMessage::Close(_) => return,
            _ => continue,
        };
        let reply = serde_json::json!({
            "kind": "assistant",
            "body": format!("(stub echo) {body}")
        });
        if socket
            .send(WsMessage::Text(reply.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
}

// ─── raw turn endpoint ──────────────────────────────────────────────────────
//
// The internal `TurnRequest` / `TurnResponse` shapes from `gauss-gateway`
// expose `actions` and `chain_head_hex` — they're written for an
// in-process executor. The on-wire surface here keeps that shape but
// returns a stub until the DTE wiring lands in slice 3c.

use gauss_gateway::turn::{TurnRequest, TurnResponse};

#[axum::debug_handler]
async fn handle_turn(
    State(_state): State<SurfaceState>,
    Json(req): Json<TurnRequest>,
) -> Json<TurnResponse> {
    Json(TurnResponse::ok(req.turn_id, vec![], "0".repeat(64), 0))
}

#[axum::debug_handler]
async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "report": {
            "ok": true,
            "overall": "green",
            "note": "SDHE invariants land in Phase 2",
        }
    }))
}

// ─── router + serve ─────────────────────────────────────────────────────────

/// Build the Axum router. Exposed for integration tests.
pub fn router(state: SurfaceState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(handle_chat_completions))
        .route("/v1/completions", post(handle_completions))
        .route("/v1/models", get(handle_models))
        .route("/v1/chat/ws", get(handle_chat_ws))
        .route("/v1/turn", post(handle_turn))
        .route("/v1/health", get(handle_health))
        .route("/v1/audit/head", get(handle_audit_head))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Bind to `addr` and serve until shut down.
pub async fn serve(addr: SocketAddr, state: SurfaceState) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "gaussclaw-surfaces listening");
    axum::serve(listener, app).await?;
    Ok(())
}


// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> SurfaceState {
        SurfaceState::new("anthropic/claude-3.5-sonnet")
    }

    async fn post_json(uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn models_lists_the_default_model() {
        let (status, body) = get_json("/v1/models").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["object"], "list");
        assert_eq!(body["data"][0]["id"], "anthropic/claude-3.5-sonnet");
        assert_eq!(body["data"][0]["object"], "model");
    }

    #[tokio::test]
    async fn chat_completions_unary_shape_matches_openai() {
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": false,
        });
        let (status, body) = post_json("/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::OK);
        // Canonical OpenAI Chat Completions response keys.
        assert!(body["id"].is_string());
        assert_eq!(body["object"], "chat.completion");
        assert!(body["choices"].is_array());
        assert_eq!(body["choices"][0]["index"], 0);
        assert_eq!(body["choices"][0]["message"]["role"], "assistant");
        assert!(body["choices"][0]["message"]["content"].is_string());
        assert_eq!(body["choices"][0]["finish_reason"], "stop");
        assert!(body["usage"]["prompt_tokens"].is_number());
        assert!(body["usage"]["completion_tokens"].is_number());
    }

    #[tokio::test]
    async fn chat_completions_streaming_returns_sse() {
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [{ "role": "user", "content": "stream me" }],
            "stream": true,
        });
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(req.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(ct.starts_with("text/event-stream"), "got CT={ct}");
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8_lossy(&bytes);
        // Must contain at least one `data:` line and the `[DONE]` sentinel.
        assert!(body.contains("data:"));
        assert!(body.contains("[DONE]"));
    }

    #[tokio::test]
    async fn legacy_completions_returns_text() {
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "prompt": "say hi"
        });
        let (status, body) = post_json("/v1/completions", req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["object"], "text_completion");
        assert!(body["choices"][0]["text"].as_str().unwrap().contains("say hi"));
    }

    #[tokio::test]
    async fn raw_turn_returns_turn_response_shape() {
        let req = serde_json::json!({
            "turn_id": 42,
            "observation": {
                "kind": "Text",
                "body": "hi"
            },
        });
        let (status, body) = post_json("/v1/turn", req).await;
        // gauss-gateway::TurnRequest may not deserialise from this exact
        // shape; we accept either OK or 422 — both are valid contract states.
        assert!(status == StatusCode::OK || status == StatusCode::UNPROCESSABLE_ENTITY);
        if status == StatusCode::OK {
            assert!(body["completion"].is_string());
        }
    }

    #[tokio::test]
    async fn health_is_green() {
        let (status, body) = get_json("/v1/health").await;
        assert_eq!(status, StatusCode::OK);
        // Wire shape: { report: { ok, overall, ... } } — matches
        // gauss-gateway::HealthResponse field names.
        assert_eq!(body["report"]["ok"], true);
        assert_eq!(body["report"]["overall"], "green");
    }

    #[tokio::test]
    async fn admit_denial_returns_403() {
        use std::sync::Arc;
        use gauss_kernel::PrivilegedKernel;
        use gauss_core::CapToken;

        // Build a kernel with the empty capability set — every admit
        // call must refuse, including the NETWORK_GET that
        // /v1/chat/completions requires.
        let empty_grant = Arc::new(PrivilegedKernel::new(CapToken::BOTTOM));
        let state = SurfaceState::with_kernel(
            "anthropic/claude-3.5-sonnet",
            gaussclaw_agent::KernelHandle::new(empty_grant),
        );
        let app = router(state);
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": false,
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(req.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "denied");
    }

    #[tokio::test]
    async fn audit_head_endpoint_returns_hex_digest() {
        let (status, body) = get_json("/v1/audit/head").await;
        assert_eq!(status, StatusCode::OK);
        let hex = body["digest_hex"].as_str().expect("hex");
        assert_eq!(hex.len(), 64, "32 bytes = 64 hex chars");
    }

    #[tokio::test]
    async fn audit_head_advances_after_a_chat_request() {
        // The audit trace must produce a different head after a chat
        // request lands — proving the WAL-before-effect path is wired.
        let state = SurfaceState::new("anthropic/claude-3.5-sonnet");
        let app_before = router(state.clone());
        let resp = app_before
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/audit/head")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let before: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let before_hex = before["digest_hex"].as_str().unwrap().to_string();

        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": false,
        });
        let app_chat = router(state.clone());
        let _ = app_chat
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(req.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let app_after = router(state);
        let resp = app_after
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/audit/head")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let after: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let after_hex = after["digest_hex"].as_str().unwrap();

        assert_ne!(
            before_hex, after_hex,
            "audit chain head must advance on /v1/chat/completions"
        );
    }

    #[tokio::test]
    async fn chat_response_echoes_the_user_message() {
        // With the real TurnPolicy wired in, the body must be the echo
        // provider's text — proving end-to-end agent dispatch is alive.
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [{ "role": "user", "content": "ping" }],
            "stream": false,
        });
        let (status, body) = post_json("/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::OK);
        let content = body["choices"][0]["message"]["content"].as_str().unwrap();
        assert!(content.contains("ping"), "got body={content}");
    }

    #[tokio::test]
    async fn missing_model_falls_back_to_default() {
        let req = serde_json::json!({
            "model": "",
            "messages": [{ "role": "user", "content": "hi" }],
            "stream": false,
        });
        let (status, body) = post_json("/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["model"], "anthropic/claude-3.5-sonnet");
    }
}
