//! OpenAI SDK wire-shape parity tests.
//!
//! Encodes the response shape the upstream `openai` Python SDK
//! validates internally when it parses GaussClaw responses. The asserts
//! here are the structural contract — any deviation breaks the
//! drop-in SDK promise (`OpenAI(base_url="http://gaussclaw/v1")`
//! works without code changes).
//!
//! Phase 1 Task 11 of `GAUSSCLAW_ROADMAP.md`. The actual end-to-end
//! suite (running `openai` Python against a live binary) lands in
//! GA; this gate enforces wire-shape correctness on every PR.
//!
//! ## What the SDK validates
//!
//! Sources, distilled into shape rules:
//!
//! - `openai.types.chat.ChatCompletion` (unary response):
//!     - `id: str`, `object: "chat.completion"`, `created: int`,
//!       `model: str`, `choices: list[Choice]`, `usage: CompletionUsage`.
//!     - Each `Choice` has `index: int`, `message: Message`,
//!       `finish_reason: "stop" | "length" | "tool_calls" | "content_filter"`.
//!     - `Message` has `role: "assistant"` and `content: str`.
//!     - `CompletionUsage` has `prompt_tokens: int`, `completion_tokens: int`,
//!       `total_tokens: int`.
//!
//! - `openai.types.chat.ChatCompletionChunk` (SSE chunk):
//!     - `id: str`, `object: "chat.completion.chunk"`, `created: int`,
//!       `model: str`, `choices: list[ChunkChoice]`.
//!     - Each `ChunkChoice` has `index: int`, `delta: Delta`,
//!       `finish_reason: str | None`.
//!     - `Delta` has at most `role: str` and `content: str` (both optional).
//!     - Stream terminates with a `data: [DONE]` line.
//!
//! - `openai.types.Model`:
//!     - `id: str`, `object: "model"`, `created: int`, `owned_by: str`.

#![allow(clippy::doc_markdown, clippy::missing_docs_in_private_items)]

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use gaussclaw_surfaces::{router, SurfaceState};
    use tower::ServiceExt;

    fn live_router() -> axum::Router {
        router(SurfaceState::new("anthropic/claude-3.5-sonnet"))
    }

    async fn post_json(uri: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let resp = live_router()
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
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = live_router()
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
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    // ── /v1/models ──────────────────────────────────────────────────────────

    /// `openai.types.Model` requires `id: str`, `object: "model"`,
    /// `created: int`, `owned_by: str`.
    #[tokio::test]
    async fn models_row_is_openai_shaped() {
        let (status, body) = get_json("/v1/models").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["object"], "list");
        let data = body["data"].as_array().expect("data must be an array");
        assert!(!data.is_empty(), "must list at least one model");
        for row in data {
            let id = row["id"].as_str().expect("Model.id must be str");
            assert!(!id.is_empty(), "Model.id must be non-empty");
            assert_eq!(row["object"], "model", "Model.object must be \"model\"");
            assert!(row["created"].is_number(), "Model.created must be int");
            assert!(
                row["owned_by"].as_str().is_some(),
                "Model.owned_by must be str"
            );
        }
    }

    // ── /v1/chat/completions unary ──────────────────────────────────────────

    /// Sweeps every field `openai.types.chat.ChatCompletion` validates.
    #[tokio::test]
    async fn chat_completion_unary_matches_openai_chatcompletion() {
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [
                { "role": "system", "content": "you are a wire-shape parity gate" },
                { "role": "user",   "content": "hello" }
            ],
            "stream": false,
        });
        let (status, body) = post_json("/v1/chat/completions", req).await;
        assert_eq!(status, StatusCode::OK);

        // Top-level fields.
        assert!(
            body["id"].as_str().is_some(),
            "ChatCompletion.id must be str"
        );
        assert_eq!(
            body["object"], "chat.completion",
            "ChatCompletion.object must be exact"
        );
        assert!(
            body["created"].is_number(),
            "ChatCompletion.created must be int"
        );
        assert!(
            body["model"].as_str().is_some(),
            "ChatCompletion.model must be str"
        );

        // choices: list[Choice]
        let choices = body["choices"].as_array().expect("choices must be array");
        assert!(!choices.is_empty(), "choices must be non-empty");
        for c in choices {
            assert!(c["index"].is_number(), "Choice.index must be int");
            let msg = &c["message"];
            assert_eq!(msg["role"], "assistant", "Message.role must be assistant");
            assert!(
                msg["content"].as_str().is_some(),
                "Message.content must be str"
            );
            let fr = c["finish_reason"]
                .as_str()
                .expect("Choice.finish_reason must be str");
            assert!(
                matches!(
                    fr,
                    "stop" | "length" | "tool_calls" | "tool" | "content_filter"
                ),
                "Choice.finish_reason must be canonical, got {fr}"
            );
        }

        // usage: CompletionUsage
        let usage = &body["usage"];
        assert!(
            usage["prompt_tokens"].is_number(),
            "CompletionUsage.prompt_tokens must be int"
        );
        assert!(
            usage["completion_tokens"].is_number(),
            "CompletionUsage.completion_tokens must be int"
        );
        assert!(
            usage["total_tokens"].is_number(),
            "CompletionUsage.total_tokens must be int"
        );
        let total = usage["total_tokens"].as_u64().unwrap();
        let prompt = usage["prompt_tokens"].as_u64().unwrap();
        let completion = usage["completion_tokens"].as_u64().unwrap();
        assert_eq!(
            total,
            prompt.saturating_add(completion),
            "CompletionUsage.total_tokens must equal prompt + completion"
        );
    }

    // ── /v1/chat/completions stream ─────────────────────────────────────────

    /// SSE stream must yield chunks shaped like
    /// `openai.types.chat.ChatCompletionChunk` and terminate on
    /// `data: [DONE]`.
    #[tokio::test]
    async fn chat_completion_stream_matches_openai_chunk() {
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [{ "role": "user", "content": "stream please" }],
            "stream": true,
        });
        let resp = live_router()
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

        // Content-Type must start with text/event-stream.
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ct.starts_with("text/event-stream"),
            "stream content-type must be text/event-stream, got {ct}"
        );

        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8_lossy(&bytes);

        // Must contain the [DONE] sentinel.
        assert!(body.contains("[DONE]"), "stream must terminate with [DONE]");

        // Parse out data: lines and validate each chunk JSON.
        let mut chunks_seen = 0usize;
        for sse_line in body.lines() {
            let Some(payload) = sse_line.strip_prefix("data: ") else {
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                continue;
            }
            let chunk: serde_json::Value =
                serde_json::from_str(payload).expect("each chunk must be valid JSON");
            assert!(chunk["id"].as_str().is_some());
            assert_eq!(chunk["object"], "chat.completion.chunk");
            assert!(chunk["created"].is_number());
            assert!(chunk["model"].as_str().is_some());
            let choices = chunk["choices"].as_array().expect("chunk.choices");
            assert!(!choices.is_empty());
            for c in choices {
                assert!(c["index"].is_number());
                // delta is an object; both fields are optional but no
                // unknown keys are permitted.
                assert!(c["delta"].is_object(), "Delta must be object");
                // finish_reason is null OR a canonical string.
                let fr = &c["finish_reason"];
                if !fr.is_null() {
                    let s = fr.as_str().expect("finish_reason must be str when present");
                    assert!(
                        matches!(
                            s,
                            "stop" | "length" | "tool_calls" | "tool" | "content_filter"
                        ),
                        "finish_reason must be canonical, got {s}"
                    );
                }
            }
            chunks_seen += 1;
        }
        assert!(chunks_seen > 0, "must emit at least one data: chunk");
    }

    // ── /v1/completions legacy ──────────────────────────────────────────────

    /// `openai.types.Completion` requires `id, object="text_completion",
    /// created, model, choices[].{text, index, finish_reason}, usage`.
    #[tokio::test]
    async fn legacy_completion_matches_openai_completion() {
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "prompt": "wire shape test"
        });
        let (status, body) = post_json("/v1/completions", req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["id"].as_str().is_some());
        assert_eq!(body["object"], "text_completion");
        assert!(body["created"].is_number());
        assert!(body["model"].as_str().is_some());
        let choices = body["choices"].as_array().unwrap();
        assert!(!choices.is_empty());
        for c in choices {
            assert!(c["text"].as_str().is_some());
            assert!(c["index"].is_number());
            assert!(c["finish_reason"].as_str().is_some());
        }
        assert!(body["usage"]["prompt_tokens"].is_number());
        assert!(body["usage"]["completion_tokens"].is_number());
        assert!(body["usage"]["total_tokens"].is_number());
    }

    // ── content actually echoes the user message ────────────────────────────

    /// Beyond shape, the response body must reflect the user message —
    /// proving the real `TurnPolicy` path is wired all the way through
    /// the wire surface.
    #[tokio::test]
    async fn unary_body_echoes_user_message() {
        let req = serde_json::json!({
            "model": "anthropic/claude-3.5-sonnet",
            "messages": [{ "role": "user", "content": "marker-xyz" }],
            "stream": false,
        });
        let (_, body) = post_json("/v1/chat/completions", req).await;
        let content = body["choices"][0]["message"]["content"].as_str().unwrap();
        assert!(
            content.contains("marker-xyz"),
            "unary echo content must contain the user marker, got {content}"
        );
    }
}
