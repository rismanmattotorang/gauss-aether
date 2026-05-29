//! OpenAI-compatible `/v1/chat/completions` + `/v1/models` shapes.
//!
//! These types mirror the OpenAI REST API closely enough that an
//! unmodified OpenAI SDK pointed at `http://localhost:8080/v1` keeps
//! working — the README's "point any OpenAI client at localhost"
//! promise. Unlike `gauss_gateway::openai` (which surfaces audit-chain
//! accounting in `usage`), this module emits the standard
//! `prompt_tokens` / `completion_tokens` / `total_tokens` triple a real
//! client expects.
//!
//! The crate is framework-agnostic: it owns the wire shapes and the
//! request↔engine conversions, but no HTTP server. `gaussclaw-web`
//! mounts the axum handlers that call [`prompt_from_request`], run the
//! turn through the live `TurnPolicy`, and render the result with
//! [`response_from_completion`].

use gaussclaw_agent::{Completion, Message, Prompt};
use serde::{Deserialize, Serialize};

/// One message in a chat-completions request or response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    /// `"system"`, `"user"`, `"assistant"`, or `"tool"`.
    pub role: String,
    /// Message content. OpenAI permits `null` content on assistant
    /// tool-call messages; we coerce that (and a missing field) to an
    /// empty string.
    #[serde(default, deserialize_with = "de_nullable_string")]
    pub content: String,
}

/// Deserialize a `String` that may arrive as JSON `null` (OpenAI sends
/// `content: null` on assistant tool-call messages) — coerced to `""`.
fn de_nullable_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

impl ChatMessage {
    /// Construct a message.
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// `POST /v1/chat/completions` request body.
///
/// Unknown fields are tolerated (OpenAI clients send many optional
/// knobs — `top_p`, `presence_penalty`, `tools`, …); we deserialize the
/// subset the engine acts on and ignore the rest rather than 400.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// Requested model id.
    pub model: String,
    /// Conversation messages, in order.
    pub messages: Vec<ChatMessage>,
    /// Optional sampling temperature (accepted; the engine's provider
    /// codec decides whether to forward it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Optional max output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Streaming flag. When `true`, the response layer delivers
    /// `chat.completion.chunk` Server-Sent Events instead of a single
    /// JSON body.
    #[serde(default)]
    pub stream: bool,
}

/// Why a request couldn't be turned into an engine prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestError {
    /// `messages` was empty.
    NoMessages,
}

impl RequestError {
    /// OpenAI-style error `type` slug.
    #[must_use]
    pub const fn error_type(&self) -> &'static str {
        match self {
            Self::NoMessages => "invalid_request_error",
        }
    }

    /// Human-readable message.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        match self {
            Self::NoMessages => "`messages` must contain at least one message",
        }
    }
}

/// Token accounting in the OpenAI-standard shape.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    /// Prompt (input) tokens.
    pub prompt_tokens: u32,
    /// Completion (output) tokens.
    pub completion_tokens: u32,
    /// Sum of the two.
    pub total_tokens: u32,
}

/// One choice in the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    /// 0-based index.
    pub index: u32,
    /// The generated message.
    pub message: ChatMessage,
    /// `"stop"`, `"length"`, `"tool_calls"`, or `"content_filter"`.
    pub finish_reason: String,
}

/// `POST /v1/chat/completions` response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// Response id (`chatcmpl-…`).
    pub id: String,
    /// Always `"chat.completion"`.
    pub object: String,
    /// Unix seconds the response was created.
    pub created: u64,
    /// Echo of the requested model.
    pub model: String,
    /// Generated choices (always exactly one today).
    pub choices: Vec<ChatChoice>,
    /// Token accounting.
    pub usage: Usage,
}

/// `GET /v1/models` list entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCard {
    /// Model id (e.g. `"anthropic/claude-3.5-sonnet"`).
    pub id: String,
    /// Always `"model"`.
    pub object: String,
    /// Unix seconds (creation time; we use the server's "since" stamp).
    pub created: u64,
    /// Owning org / vendor.
    pub owned_by: String,
}

/// `GET /v1/models` response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelList {
    /// Always `"list"`.
    pub object: String,
    /// The available models.
    pub data: Vec<ModelCard>,
}

/// OpenAI-style error envelope: `{ "error": { "message", "type", "code" } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    /// The error body.
    pub error: ApiErrorBody,
}

/// The inner error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Human-readable message.
    pub message: String,
    /// Error category slug (`"invalid_request_error"`, …).
    #[serde(rename = "type")]
    pub error_type: String,
    /// Optional machine-readable code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl ApiError {
    /// Build an error envelope.
    #[must_use]
    pub fn new(message: impl Into<String>, error_type: impl Into<String>) -> Self {
        Self {
            error: ApiErrorBody {
                message: message.into(),
                error_type: error_type.into(),
                code: None,
            },
        }
    }

    /// Build an envelope from a [`RequestError`].
    #[must_use]
    pub fn from_request_error(e: &RequestError) -> Self {
        Self::new(e.message(), e.error_type())
    }
}

// ─── conversions ────────────────────────────────────────────────────────────

/// Map an OpenAI chat-completions request into an engine [`Prompt`].
///
/// Streaming (`stream: true`) is handled at the response layer, not
/// here — this just builds the prompt either way.
///
/// # Errors
/// - [`RequestError::NoMessages`] when `messages` is empty.
pub fn prompt_from_request(req: &ChatCompletionRequest) -> Result<Prompt, RequestError> {
    if req.messages.is_empty() {
        return Err(RequestError::NoMessages);
    }
    let messages = req
        .messages
        .iter()
        .map(|m| Message::new(m.role.clone(), m.content.clone()))
        .collect();
    Ok(Prompt::new(req.model.clone(), messages))
}

/// Translate the engine's canonical `finish_reason` to the OpenAI slug.
///
/// The engine uses `"tool"`; OpenAI uses `"tool_calls"`. Everything else
/// (`"stop"`, `"length"`, `"content_filter"`) is already aligned.
#[must_use]
pub fn map_finish_reason(engine: &str) -> &str {
    match engine {
        "tool" => "tool_calls",
        other => other,
    }
}

/// Render a finished [`Completion`] as an OpenAI chat-completions
/// response. The caller supplies the response `id` and `created` stamp
/// (the engine doesn't mint them).
#[must_use]
pub fn response_from_completion(
    requested_model: &str,
    completion: &Completion,
    id: impl Into<String>,
    created: u64,
) -> ChatCompletionResponse {
    let prompt_tokens = completion.usage.prompt;
    let completion_tokens = completion.usage.completion;
    ChatCompletionResponse {
        id: id.into(),
        object: "chat.completion".into(),
        created,
        // Echo the model the client asked for, matching OpenAI behaviour.
        model: requested_model.to_owned(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage::new("assistant", completion.text.clone()),
            finish_reason: map_finish_reason(&completion.finish_reason).to_owned(),
        }],
        usage: Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        },
    }
}

// ─── streaming (SSE) ──────────────────────────────────────────────────────────

/// Incremental delta in a streamed chunk. `role` appears on the first
/// chunk, `content` on body chunks; both are omitted on the final
/// (finish) chunk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Delta {
    /// Present only on the opening chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// A slice of the assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// One choice within a streamed [`ChatCompletionChunk`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// 0-based index.
    pub index: u32,
    /// Incremental delta for this chunk.
    pub delta: Delta,
    /// Set only on the terminal chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// One `chat.completion.chunk` SSE frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    /// Response id (`chatcmpl-…`); stable across the whole stream.
    pub id: String,
    /// Always `"chat.completion.chunk"`.
    pub object: String,
    /// Unix seconds.
    pub created: u64,
    /// Echo of the requested model.
    pub model: String,
    /// Choices (always exactly one today).
    pub choices: Vec<ChunkChoice>,
}

/// Maximum characters per streamed content delta. Splitting the final
/// text into several frames gives clients a genuine incremental stream
/// rather than one giant frame.
const STREAM_CHUNK_CHARS: usize = 16;

/// Render a finished [`Completion`] as the ordered streaming chunks.
///
/// An opening `{role:"assistant"}` frame, one or more `{content:…}`
/// frames, then a terminal frame carrying `finish_reason`. The caller
/// emits each as an SSE `data:` line and closes with `data: [DONE]`.
///
/// This is "stream-shaped" delivery of a completed turn — faithful to
/// the OpenAI wire protocol — not token-by-token generation (the
/// provider codec returns the full text in one call today).
#[must_use]
pub fn stream_chunks(
    requested_model: &str,
    completion: &Completion,
    id: &str,
    created: u64,
) -> Vec<ChatCompletionChunk> {
    let frame = |delta: Delta, finish: Option<String>| ChatCompletionChunk {
        id: id.to_owned(),
        object: "chat.completion.chunk".into(),
        created,
        model: requested_model.to_owned(),
        choices: vec![ChunkChoice {
            index: 0,
            delta,
            finish_reason: finish,
        }],
    };

    let mut chunks = Vec::new();
    // Opening frame announces the assistant role.
    chunks.push(frame(
        Delta {
            role: Some("assistant".into()),
            content: None,
        },
        None,
    ));
    // Body frames — char-windowed so multi-byte text is never split
    // mid-codepoint.
    let text_chars: Vec<char> = completion.text.chars().collect();
    for window in text_chars.chunks(STREAM_CHUNK_CHARS) {
        chunks.push(frame(
            Delta {
                role: None,
                content: Some(window.iter().collect()),
            },
            None,
        ));
    }
    // Terminal frame carries the finish reason and an empty delta.
    chunks.push(frame(
        Delta::default(),
        Some(map_finish_reason(&completion.finish_reason).to_owned()),
    ));
    chunks
}

/// Build a `/v1/models` listing from `(id, owned_by)` pairs.
pub fn model_list<I, S>(models: I, created: u64) -> ModelList
where
    I: IntoIterator<Item = (S, S)>,
    S: Into<String>,
{
    ModelList {
        object: "list".into(),
        data: models
            .into_iter()
            .map(|(id, owned_by)| ModelCard {
                id: id.into(),
                object: "model".into(),
                created,
                owned_by: owned_by.into(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaussclaw_agent::TokenCount;

    fn req(stream: bool, messages: Vec<ChatMessage>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "anthropic/claude-3.5-sonnet".into(),
            messages,
            temperature: None,
            max_tokens: None,
            stream,
        }
    }

    #[test]
    fn request_tolerates_unknown_fields() {
        // OpenAI clients send top_p, presence_penalty, tools, etc.
        let json = r#"{
            "model": "gpt-4o",
            "messages": [{"role":"user","content":"hi"}],
            "top_p": 0.9,
            "presence_penalty": 0.1,
            "tools": [{"type":"function"}]
        }"#;
        let parsed: ChatCompletionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.model, "gpt-4o");
        assert_eq!(parsed.messages.len(), 1);
    }

    #[test]
    fn null_or_missing_content_coerces_to_empty() {
        // Explicit null (assistant tool-call message) → "".
        let m: ChatMessage =
            serde_json::from_str(r#"{"role":"assistant","content":null}"#).unwrap();
        assert_eq!(m.content, "");
        // Missing field → "".
        let m: ChatMessage = serde_json::from_str(r#"{"role":"assistant"}"#).unwrap();
        assert_eq!(m.content, "");
        // Present string → preserved.
        let m: ChatMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).unwrap();
        assert_eq!(m.content, "hi");
    }

    #[test]
    fn prompt_from_request_maps_messages_in_order() {
        let r = req(
            false,
            vec![
                ChatMessage::new("system", "be terse"),
                ChatMessage::new("user", "hello"),
            ],
        );
        let prompt = prompt_from_request(&r).unwrap();
        assert_eq!(prompt.model, "anthropic/claude-3.5-sonnet");
        assert_eq!(prompt.messages.len(), 2);
        assert_eq!(prompt.messages[0].role, "system");
        assert_eq!(prompt.messages[1].content, "hello");
    }

    #[test]
    fn streaming_request_still_builds_a_prompt() {
        // Streaming is handled at the response layer; prompt construction
        // succeeds regardless of the stream flag.
        let r = req(true, vec![ChatMessage::new("user", "hi")]);
        let prompt = prompt_from_request(&r).unwrap();
        assert_eq!(prompt.messages.len(), 1);
    }

    #[test]
    fn stream_chunks_open_body_and_finish() {
        let completion = Completion::new("hello there friend", "m", "stop", TokenCount::new(2, 3));
        let chunks = stream_chunks("gpt-4o", &completion, "chatcmpl-x", 7);
        // First frame announces the role, last carries finish_reason.
        assert_eq!(
            chunks.first().unwrap().choices[0].delta.role.as_deref(),
            Some("assistant")
        );
        assert!(chunks.first().unwrap().choices[0].finish_reason.is_none());
        let last = chunks.last().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
        assert!(last.choices[0].delta.content.is_none());
        // Reassembling the content deltas reproduces the full text.
        let reassembled: String = chunks
            .iter()
            .filter_map(|c| c.choices[0].delta.content.clone())
            .collect();
        assert_eq!(reassembled, "hello there friend");
        // Every frame echoes the requested model + chunk object kind.
        assert!(chunks
            .iter()
            .all(|c| c.model == "gpt-4o" && c.object == "chat.completion.chunk"));
    }

    #[test]
    fn stream_chunks_empty_text_is_open_then_finish() {
        let completion = Completion::new("", "m", "stop", TokenCount::new(0, 0));
        let chunks = stream_chunks("m", &completion, "id", 1);
        // No content frames for empty text: just role + finish.
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].choices[0].finish_reason.is_some());
    }

    #[test]
    fn empty_messages_is_rejected() {
        let r = req(false, vec![]);
        assert_eq!(
            prompt_from_request(&r).unwrap_err(),
            RequestError::NoMessages
        );
    }

    #[test]
    fn finish_reason_tool_maps_to_tool_calls() {
        assert_eq!(map_finish_reason("tool"), "tool_calls");
        assert_eq!(map_finish_reason("stop"), "stop");
        assert_eq!(map_finish_reason("length"), "length");
        assert_eq!(map_finish_reason("content_filter"), "content_filter");
    }

    #[test]
    fn response_from_completion_fills_standard_usage() {
        let completion = Completion::new(
            "hello back",
            "anthropic/claude-3.5-sonnet",
            "stop",
            TokenCount::new(11, 7),
        );
        let resp = response_from_completion("gpt-4o", &completion, "chatcmpl-test", 1_700_000_000);
        assert_eq!(resp.object, "chat.completion");
        // Model echoes what the *client* requested, not the engine's.
        assert_eq!(resp.model, "gpt-4o");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.role, "assistant");
        assert_eq!(resp.choices[0].message.content, "hello back");
        assert_eq!(resp.choices[0].finish_reason, "stop");
        assert_eq!(resp.usage.prompt_tokens, 11);
        assert_eq!(resp.usage.completion_tokens, 7);
        assert_eq!(resp.usage.total_tokens, 18);
    }

    #[test]
    fn response_serializes_to_openai_shape() {
        let completion = Completion::new("x", "m", "stop", TokenCount::new(1, 1));
        let resp = response_from_completion("m", &completion, "chatcmpl-1", 1);
        let v: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["choices"][0]["message"]["role"], "assistant");
        assert_eq!(v["usage"]["total_tokens"], 2);
        // The audit-flavoured field names from gauss_gateway must NOT
        // leak into the OpenAI surface.
        assert!(v["usage"]["chain_index"].is_null());
    }

    #[test]
    fn model_list_has_list_envelope() {
        let list = model_list(
            vec![
                ("anthropic/claude-3.5-sonnet", "anthropic"),
                ("openai/gpt-4o", "openai"),
            ],
            42,
        );
        assert_eq!(list.object, "list");
        assert_eq!(list.data.len(), 2);
        assert_eq!(list.data[0].object, "model");
        assert_eq!(list.data[0].owned_by, "anthropic");
        assert_eq!(list.data[1].id, "openai/gpt-4o");
    }

    #[test]
    fn api_error_from_request_error_round_trips() {
        let e = ApiError::from_request_error(&RequestError::NoMessages);
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["error"]["type"], "invalid_request_error");
        assert!(v["error"]["message"].as_str().unwrap().contains("messages"));
    }
}
