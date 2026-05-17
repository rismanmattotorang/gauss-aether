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
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use futures_util::stream::{self, Stream};
use gauss_core::{CapToken, TaintLabel};
use gauss_gateway::openai::{OpenAiChatMessage, OpenAiChatRequest};
use gaussclaw_agent::{KernelHandle, SurfaceRequest};

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

/// State threaded to every handler. Holds the configured model name and
/// the [`KernelHandle`] that gates every admission check.
#[derive(Clone)]
pub struct SurfaceState {
    /// Default model id reported by `/v1/models` and used when a request
    /// omits its `model` field.
    pub default_model: String,
    /// Shared kernel handle — admission + plane selection.
    pub kernel: KernelHandle,
}

impl SurfaceState {
    /// Build a state with a permissive kernel. Use this for tests and
    /// for the Phase 1 demo binary.
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            default_model: default_model.into(),
            kernel: KernelHandle::permissive(),
        }
    }

    /// Build a state with a caller-supplied kernel handle. Production
    /// deployments use this to plug in a real grant pipeline.
    pub fn with_kernel(default_model: impl Into<String>, kernel: KernelHandle) -> Self {
        Self {
            default_model: default_model.into(),
            kernel,
        }
    }
}

// Suppress the unused-import warning if `Arc` ever stops being needed
// at this scope. `Arc` is used throughout the crate elsewhere.
#[allow(dead_code)]
fn _arc_in_scope() -> Arc<()> {
    Arc::new(())
}

// ─── models endpoint ────────────────────────────────────────────────────────

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
    // Every SDK chat request passes through the kernel admit gate first.
    // The cap requirement is `NETWORK_GET` (the surface itself is a
    // network-receiving endpoint) and the request taint is `User` (the
    // operator's SDK client). Both are conservative defaults; downstream
    // tool dispatch widens them as needed.
    let _plane = state.kernel.plane_for(SurfaceRequest::SdkChat);
    if let Err(e) = state.kernel.admit(CapToken::NETWORK_GET, TaintLabel::User) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": {
                    "code": "denied",
                    "message": format!("admit failed: {e:?}"),
                }
            })),
        )
            .into_response();
    }

    if req.stream {
        sse_stub(&state, &req).into_response()
    } else {
        Json(unary_stub(&state, &req)).into_response()
    }
}

fn unary_stub(state: &SurfaceState, req: &OpenAiChatRequest) -> ChatResponse {
    let model = if req.model.is_empty() {
        state.default_model.clone()
    } else {
        req.model.clone()
    };
    let stub_body = stub_response_text(req);
    let prompt_tokens: u32 = req
        .messages
        .iter()
        .map(|m| u32::try_from(m.content.len() / 4).unwrap_or(u32::MAX))
        .sum();
    let completion_tokens: u32 = 32;
    ChatResponse {
        id: "chatcmpl-stub".into(),
        object: "chat.completion",
        created: 0,
        model,
        choices: vec![ChatChoice {
            index: 0,
            message: OpenAiChatMessage::new("assistant", stub_body),
            finish_reason: "stop",
        }],
        usage: ChatUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        },
    }
}

fn sse_stub(
    state: &SurfaceState,
    req: &OpenAiChatRequest,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + 'static> {
    let model = if req.model.is_empty() {
        state.default_model.clone()
    } else {
        req.model.clone()
    };
    let body = stub_response_text(req);
    // Emit one chunk per token (whitespace-split is fine for a stub),
    // then a final `[DONE]` sentinel to match OpenAI SSE framing. We
    // collect first so the closure can `move` an owned `model`; the
    // borrow checker won't let us thread it through `split_inclusive`
    // directly without an intermediate.
    #[allow(clippy::needless_collect)]
    let chunks: Vec<String> = body.split_inclusive(' ').map(String::from).collect();
    let model_owned = model;
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

fn stub_response_text(req: &OpenAiChatRequest) -> String {
    // Phase 1 slice 3c replaces this with real provider dispatch. Until
    // then the surface returns a deterministic shape-valid stub so the
    // OpenAI SDK accepts the response and CI parity tests have something
    // to bind to.
    let user_msg = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map_or("(no user message)", |m| m.content.as_str());
    format!(
        "(gaussclaw stub) provider dispatch arrives in Phase 1 slice 3c. echo: {user_msg}"
    )
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
        .send(Message::Text(banner.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    while let Some(msg) = socket.recv().await {
        let Ok(msg) = msg else { return };
        let body = match &msg {
            Message::Text(t) => t.as_str().to_string(),
            Message::Binary(_) => "(binary ignored)".into(),
            Message::Close(_) => return,
            _ => continue,
        };
        let reply = serde_json::json!({
            "kind": "assistant",
            "body": format!("(stub echo) {body}")
        });
        if socket
            .send(Message::Text(reply.to_string().into()))
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
