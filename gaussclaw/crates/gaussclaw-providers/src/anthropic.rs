//! [`AnthropicProvider`] — Anthropic Messages API leaf driver.
//!
//! Knows the Anthropic Messages API JSON wire shape:
//!
//! Request:  `POST /v1/messages` with `{model, max_tokens, messages: [...]}`
//! Response: `{content: [{text, type}], stop_reason, usage: {input_tokens, output_tokens}, ...}`
//!
//! Production deployments use an HTTP backend that authenticates with
//! the `x-api-key` header and calls `https://api.anthropic.com`. The
//! transport seam lives in [`HttpBackend`]; the wire shape is encoded
//! here.

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_VERSION: &str = "2023-06-01";

/// Anthropic Messages API leaf driver.
pub struct AnthropicProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
    api_key: String,
    version: String,
}

impl AnthropicProvider {
    /// Build a new driver.
    #[must_use]
    pub fn new(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> Self {
        Self {
            backend,
            base_url: DEFAULT_BASE_URL.into(),
            api_key: api_key.into(),
            version: DEFAULT_VERSION.into(),
        }
    }

    /// Override the base URL (for staging, on-prem proxies, …).
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override the API version header.
    #[must_use]
    pub fn with_version(mut self, v: impl Into<String>) -> Self {
        self.version = v.into();
        self
    }

    /// Encode the request body as Anthropic's Messages API expects.
    fn build_body(prompt: &Prompt) -> Vec<u8> {
        // Anthropic's `messages` excludes the system role; system text
        // is a top-level `system` field.
        let mut system_parts = Vec::new();
        let mut messages = Vec::new();
        for m in &prompt.messages {
            if m.role == "system" {
                system_parts.push(m.content.clone());
            } else {
                messages.push(serde_json::json!({
                    "role":    m.role,
                    "content": m.content,
                }));
            }
        }
        let mut body = serde_json::json!({
            "model":      prompt.model,
            "max_tokens": prompt.max_tokens.unwrap_or(1024),
            "messages":   messages,
        });
        if !system_parts.is_empty() {
            body["system"] = Value::String(system_parts.join("\n\n"));
        }
        if let Some(t) = prompt.temperature {
            body["temperature"] = Value::from(t);
        }
        serde_json::to_vec(&body).unwrap_or_default()
    }

    /// Parse the Messages API response into a canonical [`Completion`].
    fn parse_response(model: &str, raw: &[u8]) -> ProviderResult<Completion> {
        let v: Value = serde_json::from_slice(raw).map_err(|e| ProviderError::Upstream {
            code: 0,
            message: format!("anthropic response parse: {e}"),
        })?;
        let text = v["content"]
            .as_array()
            .and_then(|arr| arr.iter().find_map(|c| c["text"].as_str()))
            .unwrap_or("")
            .to_string();
        // Map Anthropic stop_reason → canonical finish_reason.
        let finish_reason = match v["stop_reason"].as_str().unwrap_or("end_turn") {
            "end_turn" | "stop_sequence" => "stop",
            "max_tokens" => "length",
            "tool_use" => "tool",
            _ => "stop",
        }
        .to_string();
        let prompt_tokens = v["usage"]["input_tokens"].as_u64().unwrap_or(0);
        let completion_tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
        Ok(Completion::new(
            text,
            model.to_string(),
            finish_reason,
            TokenCount::new(
                u32::try_from(prompt_tokens).unwrap_or(u32::MAX),
                u32::try_from(completion_tokens).unwrap_or(u32::MAX),
            ),
        ))
    }
}

#[async_trait]
impl ProviderHandle for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let req = HttpRequest {
            url: format!("{}/v1/messages", self.base_url),
            method: "POST".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-api-key".into(), self.api_key.clone()),
                ("anthropic-version".into(), self.version.clone()),
            ],
            body: Self::build_body(prompt),
        };
        let resp = self.backend.send(req).await.map_err(|e| match e {
            crate::HttpError::Upstream { status, body } => ProviderError::Upstream {
                code: status,
                message: body,
            },
            other => ProviderError::Transport(format!("{other}")),
        })?;
        if !(200..300).contains(&resp.status) {
            return Err(ProviderError::Upstream {
                code: resp.status,
                message: String::from_utf8_lossy(&resp.body).into_owned(),
            });
        }
        Self::parse_response(&prompt.model, &resp.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{HttpResponse, MockHttpBackend};
    use crate::check_postconditions;
    use gaussclaw_agent::Message;

    fn mock_response(text: &str) -> HttpResponse {
        let body = serde_json::json!({
            "content":    [{"type": "text", "text": text}],
            "stop_reason": "end_turn",
            "usage":       {"input_tokens": 5, "output_tokens": 7},
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt::new(
            "anthropic/claude-3.5-sonnet",
            vec![
                Message::new("system", "you are a wire test fixture"),
                Message::new("user", "hello"),
            ],
        )
    }

    #[tokio::test]
    async fn build_body_extracts_system_field() {
        let body = AnthropicProvider::build_body(&sample_prompt());
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["system"], "you are a wire test fixture");
        // System role removed from messages array.
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[tokio::test]
    async fn complete_round_trips_through_mock() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response(
            "hi from anthropic",
        )]));
        let p = AnthropicProvider::new(mock.clone(), "sk-test-key");
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from anthropic");
        assert_eq!(c.finish_reason, "stop");
        assert_eq!(c.usage.prompt, 5);
        assert_eq!(c.usage.completion, 7);
        check_postconditions(&c, Some(1024)).unwrap();
    }

    #[tokio::test]
    async fn upstream_5xx_propagates() {
        let mock = Arc::new(MockHttpBackend::new(vec![HttpResponse {
            status: 503,
            body: b"upstream down".to_vec(),
        }]));
        let p = AnthropicProvider::new(mock, "sk-test-key");
        let err = p.complete(&sample_prompt()).await.unwrap_err();
        match err {
            ProviderError::Upstream { code, .. } => assert_eq!(code, 503),
            other => panic!("expected Upstream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn request_carries_x_api_key_and_version() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = AnthropicProvider::new(mock.clone(), "sk-secret");
        let _ = p.complete(&sample_prompt()).await.unwrap();
        let seen = mock.seen();
        let h = &seen[0].headers;
        assert!(h.iter().any(|(k, v)| k == "x-api-key" && v == "sk-secret"));
        assert!(h.iter().any(|(k, _)| k == "anthropic-version"));
    }

    #[tokio::test]
    async fn max_tokens_stop_reason_maps_to_length() {
        let body = serde_json::json!({
            "content":    [{"type": "text", "text": "truncated"}],
            "stop_reason": "max_tokens",
            "usage":       {"input_tokens": 1, "output_tokens": 1},
        });
        let mock = Arc::new(MockHttpBackend::new(vec![HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }]));
        let p = AnthropicProvider::new(mock, "sk-x");
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.finish_reason, "length");
    }
}
