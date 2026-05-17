//! [`OllamaProvider`] — local Ollama API leaf driver.
//!
//! Request:  `POST /api/generate` with `{model, prompt, stream: false, ...}`
//! Response: `{response, done, prompt_eval_count, eval_count, ...}`
//!
//! Ollama runs locally (`http://localhost:11434` by default) so the
//! `cap_required` in the catalogue entry is `NETWORK_GET +
//! FILESYSTEM_READ` (the latter reflects the model file load); the
//! kernel admit gate refuses dispatch under Web / Adversarial taint
//! by the default declassification policy.

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Local Ollama generate-API leaf driver.
pub struct OllamaProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
}

impl OllamaProvider {
    /// Build a new driver.
    #[must_use]
    pub fn new(backend: Arc<dyn HttpBackend>) -> Self {
        Self {
            backend,
            base_url: DEFAULT_BASE_URL.into(),
        }
    }

    /// Override the base URL (Ollama on a remote host, gateway, etc.).
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn build_body(prompt: &Prompt) -> Vec<u8> {
        // Flatten messages into a single prompt string (Ollama's
        // /api/generate is not chat-shaped; /api/chat is — using
        // generate keeps the example simple and the wire shape stable).
        let joined: Vec<String> = prompt
            .messages
            .iter()
            .map(|m| format!("[{}]\n{}", m.role, m.content))
            .collect();
        let flat = joined.join("\n\n");
        // Strip "ollama/" prefix if present; Ollama doesn't use the
        // vendor prefix internally.
        let model = prompt
            .model
            .strip_prefix("ollama/")
            .unwrap_or(&prompt.model);
        let body = serde_json::json!({
            "model":  model,
            "prompt": flat,
            "stream": false,
        });
        serde_json::to_vec(&body).unwrap_or_default()
    }

    fn parse_response(model: &str, raw: &[u8]) -> ProviderResult<Completion> {
        let v: Value = serde_json::from_slice(raw).map_err(|e| ProviderError::Upstream {
            code: 0,
            message: format!("ollama response parse: {e}"),
        })?;
        let text = v["response"].as_str().unwrap_or("").to_string();
        // Ollama doesn't emit a structured finish_reason; "stop" is
        // canonical for any complete response. `done = false` would be
        // a streaming chunk we don't expect here.
        let finish_reason = "stop".to_string();
        let prompt_tokens = v["prompt_eval_count"].as_u64().unwrap_or(0);
        let completion_tokens = v["eval_count"].as_u64().unwrap_or(0);
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
impl ProviderHandle for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let req = HttpRequest {
            url: format!("{}/api/generate", self.base_url),
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
            "model":             "llama3",
            "response":          text,
            "done":              true,
            "prompt_eval_count": 6,
            "eval_count":        8,
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    #[tokio::test]
    async fn complete_round_trips_through_mock() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("hi from ollama")]));
        let p = OllamaProvider::new(mock);
        let prompt = Prompt::new("ollama/llama3", vec![Message::new("user", "hello")]);
        let c = p.complete(&prompt).await.unwrap();
        assert_eq!(c.text, "hi from ollama");
        assert_eq!(c.finish_reason, "stop");
        assert_eq!(c.usage.prompt, 6);
        assert_eq!(c.usage.completion, 8);
        check_postconditions(&c, Some(2048)).unwrap();
    }

    #[tokio::test]
    async fn vendor_prefix_is_stripped_in_request() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = OllamaProvider::new(mock.clone());
        let prompt = Prompt::new("ollama/llama3", vec![Message::new("user", "hi")]);
        let _ = p.complete(&prompt).await.unwrap();
        let seen = mock.seen();
        let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
        // The Ollama API doesn't use the vendor prefix.
        assert_eq!(body["model"], "llama3");
    }
}
