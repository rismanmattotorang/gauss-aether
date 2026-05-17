//! [`OpenAIProvider`] — OpenAI Chat Completions API leaf driver.
//!
//! Request:  `POST /v1/chat/completions` with `{model, messages, ...}`
//! Response: `{choices: [{message: {content}, finish_reason}], usage: {prompt_tokens, completion_tokens, total_tokens}, ...}`

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// OpenAI Chat Completions API leaf driver.
pub struct OpenAIProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
    api_key: String,
}

impl OpenAIProvider {
    /// Build a new driver.
    #[must_use]
    pub fn new(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> Self {
        Self {
            backend,
            base_url: DEFAULT_BASE_URL.into(),
            api_key: api_key.into(),
        }
    }

    /// Override the base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn build_body(prompt: &Prompt) -> Vec<u8> {
        let messages: Vec<Value> = prompt
            .messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role":    m.role,
                    "content": m.content,
                })
            })
            .collect();
        let mut body = serde_json::json!({
            "model":    prompt.model,
            "messages": messages,
        });
        if let Some(mt) = prompt.max_tokens {
            body["max_tokens"] = Value::from(mt);
        }
        if let Some(t) = prompt.temperature {
            body["temperature"] = Value::from(t);
        }
        serde_json::to_vec(&body).unwrap_or_default()
    }

    fn parse_response(model: &str, raw: &[u8]) -> ProviderResult<Completion> {
        let v: Value = serde_json::from_slice(raw).map_err(|e| ProviderError::Upstream {
            code: 0,
            message: format!("openai response parse: {e}"),
        })?;
        let text = v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let finish_reason = match v["choices"][0]["finish_reason"].as_str().unwrap_or("stop") {
            "stop" => "stop",
            "length" => "length",
            "tool_calls" => "tool",
            "content_filter" => "content_filter",
            _ => "stop",
        }
        .to_string();
        let prompt_tokens = v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        let completion_tokens = v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
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
impl ProviderHandle for OpenAIProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let req = HttpRequest {
            url: format!("{}/v1/chat/completions", self.base_url),
            method: "POST".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("authorization".into(), format!("Bearer {}", self.api_key)),
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
            "id":       "chatcmpl-xyz",
            "object":   "chat.completion",
            "created":  0,
            "model":    "gpt-4o",
            "choices":  [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }],
            "usage":    {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7},
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt::new("openai/gpt-4o", vec![Message::new("user", "hello")])
    }

    #[tokio::test]
    async fn complete_round_trips_through_mock() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("hi from openai")]));
        let p = OpenAIProvider::new(mock, "sk-test");
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from openai");
        assert_eq!(c.finish_reason, "stop");
        assert_eq!(c.usage.prompt, 3);
        assert_eq!(c.usage.completion, 4);
        check_postconditions(&c, Some(1024)).unwrap();
    }

    #[tokio::test]
    async fn request_carries_bearer_auth() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = OpenAIProvider::new(mock.clone(), "sk-secret");
        let _ = p.complete(&sample_prompt()).await.unwrap();
        let seen = mock.seen();
        let h = &seen[0].headers;
        assert!(h
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer sk-secret"));
    }

    #[tokio::test]
    async fn finish_reasons_map_correctly() {
        for (raw, canonical) in [
            ("stop", "stop"),
            ("length", "length"),
            ("tool_calls", "tool"),
            ("content_filter", "content_filter"),
        ] {
            let body = serde_json::json!({
                "choices": [{
                    "message": {"content": "x"},
                    "finish_reason": raw,
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            });
            let mock = Arc::new(MockHttpBackend::new(vec![HttpResponse {
                status: 200,
                body: serde_json::to_vec(&body).unwrap(),
            }]));
            let p = OpenAIProvider::new(mock, "sk-x");
            let c = p.complete(&sample_prompt()).await.unwrap();
            assert_eq!(c.finish_reason, canonical, "raw={raw}");
        }
    }
}
