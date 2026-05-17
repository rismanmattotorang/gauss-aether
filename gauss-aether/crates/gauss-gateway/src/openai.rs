//! OpenAI-compatible proxy schema (paper §XIII.D).
//!
//! These wire types let an OpenAI-Chat-Completions client talk to the
//! Gauss-Aether gateway as if it were the `OpenAI` API. The mapping is:
//!
//! * `messages[]` → joined observation body.
//! * `choices[].message.content` ← rendered text action(s).
//! * `usage` ← receipt-chain index + length (so the client can
//!   correlate audit events with token quotas).

use serde::{Deserialize, Serialize};

/// One chat message in the proxy payload.
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub struct OpenAiChatMessage {
    /// Role: `"user"`, `"assistant"`, `"system"`, `"tool"`.
    pub role: String,
    /// Message body.
    pub content: String,
}

impl OpenAiChatMessage {
    /// Construct.
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// `POST /v1/chat/completions` request body (subset).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OpenAiChatRequest {
    /// Model identifier.
    pub model: String,
    /// Messages array.
    pub messages: Vec<OpenAiChatMessage>,
    /// Optional sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Optional max tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Optional stream flag.
    #[serde(default)]
    pub stream: bool,
}

impl OpenAiChatRequest {
    /// Construct. Required because the struct is `#[non_exhaustive]`.
    #[must_use]
    pub fn new(model: impl Into<String>, messages: Vec<OpenAiChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            temperature: None,
            max_tokens: None,
            stream: false,
        }
    }
}

/// One choice in the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OpenAiChatChoice {
    /// 0-based index.
    pub index: u32,
    /// The generated message.
    pub message: OpenAiChatMessage,
    /// Stop reason (`"stop"`, `"length"`, `"content_filter"`).
    pub finish_reason: String,
}

/// Receipt-chain accounting surfaced in the `usage` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OpenAiUsage {
    /// 0-based chain index of the recorded turn.
    pub chain_index: u64,
    /// Chain length after the WAL append.
    pub chain_length: u64,
    /// Total tokens (or `0` if the deployment doesn't track tokens).
    #[serde(default)]
    pub total_tokens: u32,
}

/// `POST /v1/chat/completions` response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OpenAiChatResponse {
    /// Response identifier.
    pub id: String,
    /// Object kind (`"chat.completion"`).
    pub object: String,
    /// Unix seconds.
    pub created: u64,
    /// Echo of the requested model.
    pub model: String,
    /// Choices.
    pub choices: Vec<OpenAiChatChoice>,
    /// Receipt-chain accounting.
    pub usage: OpenAiUsage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_minimal_shape() {
        let r = OpenAiChatRequest {
            model: "gauss-1".into(),
            messages: vec![OpenAiChatMessage::new("user", "hi")],
            temperature: None,
            max_tokens: None,
            stream: false,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: OpenAiChatRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.messages.len(), 1);
        assert_eq!(back.messages[0].role, "user");
    }

    #[test]
    fn response_round_trips_with_usage() {
        let r = OpenAiChatResponse {
            id: "x-1".into(),
            object: "chat.completion".into(),
            created: 1_700_000_000,
            model: "gauss-1".into(),
            choices: vec![OpenAiChatChoice {
                index: 0,
                message: OpenAiChatMessage::new("assistant", "hi back"),
                finish_reason: "stop".into(),
            }],
            usage: OpenAiUsage {
                chain_index: 5,
                chain_length: 6,
                total_tokens: 0,
            },
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: OpenAiChatResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.usage.chain_index, 5);
    }

    #[test]
    fn omitting_temperature_keeps_payload_compact() {
        let r = OpenAiChatRequest {
            model: "gauss-1".into(),
            messages: Vec::new(),
            temperature: None,
            max_tokens: None,
            stream: false,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("temperature"));
    }
}
