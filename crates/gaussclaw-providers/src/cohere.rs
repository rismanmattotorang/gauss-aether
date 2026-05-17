//! [`CohereProvider`] — Cohere v2 chat API leaf driver.
//!
//! Wire shape:
//!
//! - `POST /v2/chat`
//! - Body: `{"model","messages":[{"role","content"}],"max_tokens","temperature"}`
//! - Auth: `Authorization: Bearer …`
//! - Response: `{"id","message":{"role":"assistant","content":[{"type":"text","text"}]},"finish_reason","usage":{"tokens":{"input_tokens","output_tokens"}}}`
//!
//! Cohere's `finish_reason` values: `COMPLETE`, `MAX_TOKENS`,
//! `STOP_SEQUENCE`, `TOOL_CALL`, `ERROR`. These map onto the
//! canonical postcondition set.

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "https://api.cohere.com";

/// Cohere v2 chat-API leaf driver.
pub struct CohereProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
    api_key: String,
}

impl CohereProvider {
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
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect();
        let model = prompt
            .model
            .strip_prefix("cohere/")
            .unwrap_or(&prompt.model);
        let mut body = serde_json::json!({
            "model":    model,
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
            message: format!("cohere response parse: {e}"),
        })?;
        // Cohere wraps the assistant text in {content: [{type: "text", text: …}]}.
        let text = v["message"]["content"]
            .as_array()
            .and_then(|arr| arr.iter().find_map(|c| c["text"].as_str()))
            .unwrap_or("")
            .to_string();
        let finish_reason = match v["finish_reason"].as_str().unwrap_or("COMPLETE") {
            "COMPLETE" | "STOP_SEQUENCE" => "stop",
            "MAX_TOKENS" => "length",
            "TOOL_CALL" => "tool",
            _ => "stop",
        }
        .to_string();
        let prompt_tokens = v["usage"]["tokens"]["input_tokens"].as_u64().unwrap_or(0);
        let completion_tokens = v["usage"]["tokens"]["output_tokens"].as_u64().unwrap_or(0);
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
impl ProviderHandle for CohereProvider {
    fn name(&self) -> &'static str {
        "cohere"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let req = HttpRequest {
            url: format!("{}/v2/chat", self.base_url),
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

    fn mock_response(text: &str, finish: &str) -> HttpResponse {
        let body = serde_json::json!({
            "id":      "ch-1",
            "message": {
                "role":    "assistant",
                "content": [{"type": "text", "text": text}],
            },
            "finish_reason": finish,
            "usage": {"tokens": {"input_tokens": 8, "output_tokens": 12}},
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt::new("cohere/command-r", vec![Message::new("user", "hi")])
    }

    #[tokio::test]
    async fn complete_round_trips_through_mock() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response(
            "hi from cohere",
            "COMPLETE",
        )]));
        let p = CohereProvider::new(mock.clone(), "co-test-key");
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from cohere");
        assert_eq!(c.finish_reason, "stop");
        assert_eq!(c.usage.prompt, 8);
        assert_eq!(c.usage.completion, 12);
        check_postconditions(&c, Some(1024)).unwrap();
    }

    #[tokio::test]
    async fn finish_reasons_map_correctly() {
        for (raw, canonical) in [
            ("COMPLETE", "stop"),
            ("STOP_SEQUENCE", "stop"),
            ("MAX_TOKENS", "length"),
            ("TOOL_CALL", "tool"),
        ] {
            let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x", raw)]));
            let p = CohereProvider::new(mock, "k");
            let c = p.complete(&sample_prompt()).await.unwrap();
            assert_eq!(c.finish_reason, canonical, "raw={raw}");
        }
    }

    #[tokio::test]
    async fn request_strips_vendor_prefix_and_carries_auth() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x", "COMPLETE")]));
        let p = CohereProvider::new(mock.clone(), "co-secret");
        let _ = p.complete(&sample_prompt()).await.unwrap();
        let seen = &mock.seen()[0];
        assert!(seen.url.contains("/v2/chat"));
        let body: Value = serde_json::from_slice(&seen.body).unwrap();
        assert_eq!(body["model"], "command-r");
        assert!(seen
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer co-secret"));
    }
}
