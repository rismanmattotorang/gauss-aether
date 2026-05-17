//! [`LlamaCppProvider`] — local llama.cpp HTTP server (`/completion` shape).
//!
//! llama.cpp's standalone server exposes two HTTP paths: a native
//! `/completion` route with a simple `{prompt, n_predict, stop, …}`
//! body, and an `/v1/chat/completions` route that mirrors OpenAI's
//! Chat Completions shape. This driver wires the native path so
//! deployments that haven't enabled the OpenAI-compat mode still
//! work. Use [`crate::openai_compat::OpenAICompatProvider`] when the
//! compat route is enabled.
//!
//! Wire shape:
//!
//! - `POST http://localhost:8080/completion`
//! - Body: `{"prompt": "<flattened>", "n_predict","temperature","stop": ["[user]"]}`
//! - Response: `{"content": "…", "stop": true, "tokens_evaluated", "tokens_predicted"}`

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "http://localhost:8080";

/// Local llama.cpp `/completion` leaf driver.
pub struct LlamaCppProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
}

impl LlamaCppProvider {
    /// Build a new driver.
    #[must_use]
    pub fn new(backend: Arc<dyn HttpBackend>) -> Self {
        Self {
            backend,
            base_url: DEFAULT_BASE_URL.into(),
        }
    }

    /// Override the base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn flatten_messages(prompt: &Prompt) -> String {
        let parts: Vec<String> = prompt
            .messages
            .iter()
            .map(|m| format!("[{}]\n{}", m.role, m.content))
            .collect();
        parts.join("\n\n")
    }

    fn build_body(prompt: &Prompt) -> Vec<u8> {
        let mut body = serde_json::json!({
            "prompt": Self::flatten_messages(prompt),
            // Make `[user]` and `[system]` act as stop tokens so the
            // model doesn't trail off into a new turn.
            "stop": ["[user]", "[system]"],
        });
        if let Some(mt) = prompt.max_tokens {
            body["n_predict"] = Value::from(mt);
        }
        if let Some(t) = prompt.temperature {
            body["temperature"] = Value::from(t);
        }
        serde_json::to_vec(&body).unwrap_or_default()
    }

    fn parse_response(prompt: &Prompt, raw: &[u8]) -> ProviderResult<Completion> {
        let v: Value = serde_json::from_slice(raw).map_err(|e| ProviderError::Upstream {
            code: 0,
            message: format!("llama.cpp response parse: {e}"),
        })?;
        let text = v["content"].as_str().unwrap_or("").to_string();
        // llama.cpp emits "stopped_eos", "stopped_word", "stopped_limit"
        // — we map the limit case onto length, everything else stop.
        let finish_reason = if v["stopped_limit"].as_bool().unwrap_or(false) {
            "length"
        } else {
            "stop"
        }
        .to_string();
        let prompt_tokens = v["tokens_evaluated"].as_u64().unwrap_or(0);
        let completion_tokens = v["tokens_predicted"].as_u64().unwrap_or(0);
        Ok(Completion::new(
            text,
            prompt.model.clone(),
            finish_reason,
            TokenCount::new(
                u32::try_from(prompt_tokens).unwrap_or(u32::MAX),
                u32::try_from(completion_tokens).unwrap_or(u32::MAX),
            ),
        ))
    }
}

#[async_trait]
impl ProviderHandle for LlamaCppProvider {
    fn name(&self) -> &'static str {
        "llama_cpp"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let req = HttpRequest {
            url: format!("{}/completion", self.base_url),
            method: "POST".into(),
            headers: vec![("content-type".into(), "application/json".into())],
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
        Self::parse_response(prompt, &resp.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{HttpResponse, MockHttpBackend};
    use crate::check_postconditions;
    use gaussclaw_agent::Message;

    fn mock_response(content: &str, stopped_limit: bool) -> HttpResponse {
        let body = serde_json::json!({
            "content": content,
            "stop": true,
            "stopped_limit": stopped_limit,
            "tokens_evaluated": 12,
            "tokens_predicted": 24,
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt::new(
            "llama_cpp/llama-3-8b.q5_k_m.gguf",
            vec![Message::new("user", "hi")],
        )
    }

    #[tokio::test]
    async fn complete_round_trips_through_mock() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("hi from llama.cpp", false)]));
        let p = LlamaCppProvider::new(mock);
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from llama.cpp");
        assert_eq!(c.finish_reason, "stop");
        assert_eq!(c.usage.prompt, 12);
        assert_eq!(c.usage.completion, 24);
        check_postconditions(&c, Some(4096)).unwrap();
    }

    #[tokio::test]
    async fn stopped_limit_maps_to_length() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("truncated", true)]));
        let p = LlamaCppProvider::new(mock);
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.finish_reason, "length");
    }

    #[tokio::test]
    async fn url_uses_completion_endpoint() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x", false)]));
        let p = LlamaCppProvider::new(mock.clone());
        let _ = p.complete(&sample_prompt()).await.unwrap();
        assert!(mock.seen()[0].url.ends_with("/completion"));
    }
}
