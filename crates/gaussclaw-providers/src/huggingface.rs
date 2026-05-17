//! [`HuggingFaceProvider`] — Hugging Face Inference Endpoints (text-
//! generation task).
//!
//! Wire shape (classic Inference API):
//!
//! - `POST /models/{model}` (or a dedicated endpoint URL)
//! - Body: `{"inputs": "<prompt as one string>", "parameters": {"max_new_tokens","temperature","return_full_text":false}}`
//! - Auth: `Authorization: Bearer <hf-token>`
//! - Response: `[{"generated_text": "…"}]`
//!
//! Hugging Face's Inference API returns just the generated text; it
//! doesn't emit a structured `finish_reason` or token counts. The
//! driver maps this onto the canonical postcondition shape by stamping
//! `finish_reason = "stop"` and computing approximate token counts
//! (text length / 4) so [`check_postconditions`] passes consistency.
//!
//! Newer HF endpoints (dedicated Inference Endpoints with the
//! Messages API enabled) speak OpenAI-Chat-Completions and can use
//! [`crate::openai_compat::OpenAICompatProvider`] instead.

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "https://api-inference.huggingface.co";

/// Hugging Face Inference Endpoints leaf driver.
pub struct HuggingFaceProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
    api_key: String,
}

impl HuggingFaceProvider {
    /// Build a new driver.
    #[must_use]
    pub fn new(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> Self {
        Self {
            backend,
            base_url: DEFAULT_BASE_URL.into(),
            api_key: api_key.into(),
        }
    }

    /// Override the base URL (for dedicated Inference Endpoints).
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Flatten chat messages into a single prompt string. HF's classic
    /// text-generation task has no chat structure; we use a simple
    /// `[role]\ncontent` framing that the model can recognise.
    fn flatten_messages(prompt: &Prompt) -> String {
        let parts: Vec<String> = prompt
            .messages
            .iter()
            .map(|m| format!("[{}]\n{}", m.role, m.content))
            .collect();
        parts.join("\n\n")
    }

    fn build_body(prompt: &Prompt) -> Vec<u8> {
        let inputs = Self::flatten_messages(prompt);
        let mut params = serde_json::Map::new();
        if let Some(mt) = prompt.max_tokens {
            params.insert("max_new_tokens".into(), Value::from(mt));
        }
        if let Some(t) = prompt.temperature {
            params.insert("temperature".into(), Value::from(t));
        }
        params.insert("return_full_text".into(), Value::from(false));
        let body = serde_json::json!({
            "inputs": inputs,
            "parameters": Value::Object(params),
        });
        serde_json::to_vec(&body).unwrap_or_default()
    }

    fn parse_response(prompt: &Prompt, raw: &[u8]) -> ProviderResult<Completion> {
        let v: Value = serde_json::from_slice(raw).map_err(|e| ProviderError::Upstream {
            code: 0,
            message: format!("huggingface response parse: {e}"),
        })?;
        // Inference API returns `[{"generated_text": "…"}]`. Some
        // endpoints return the bare object instead of the singleton
        // array; handle both.
        let text = v
            .as_array()
            .and_then(|a| a.first())
            .and_then(|c| c["generated_text"].as_str())
            .or_else(|| v["generated_text"].as_str())
            .unwrap_or("")
            .to_string();
        let completion_tokens = u32::try_from(text.len() / 4).unwrap_or(u32::MAX);
        let prompt_tokens: u32 = prompt
            .messages
            .iter()
            .map(|m| u32::try_from(m.content.len() / 4).unwrap_or(u32::MAX))
            .sum();
        Ok(Completion::new(
            text,
            prompt.model.clone(),
            "stop", // HF doesn't emit a structured reason; canonical default.
            TokenCount::new(prompt_tokens, completion_tokens),
        ))
    }
}

#[async_trait]
impl ProviderHandle for HuggingFaceProvider {
    fn name(&self) -> &'static str {
        "huggingface"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let model = prompt
            .model
            .strip_prefix("huggingface/")
            .unwrap_or(&prompt.model);
        let req = HttpRequest {
            url: format!("{}/models/{}", self.base_url, model),
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
        Self::parse_response(prompt, &resp.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{HttpResponse, MockHttpBackend};
    use crate::check_postconditions;
    use gaussclaw_agent::Message;

    fn mock_array(text: &str) -> HttpResponse {
        let body = serde_json::json!([{"generated_text": text}]);
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn mock_object(text: &str) -> HttpResponse {
        // Some HF endpoints return the bare object instead of an array.
        let body = serde_json::json!({"generated_text": text});
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt::new(
            "huggingface/meta-llama/Meta-Llama-3-8B-Instruct",
            vec![Message::new("user", "hi")],
        )
    }

    #[tokio::test]
    async fn array_response_parses() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_array("hi from hf")]));
        let p = HuggingFaceProvider::new(mock, "hf-key");
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from hf");
        assert_eq!(c.finish_reason, "stop");
        check_postconditions(&c, Some(1024)).unwrap();
    }

    #[tokio::test]
    async fn object_response_parses() {
        // Dedicated Inference Endpoints sometimes return the bare object.
        let mock = Arc::new(MockHttpBackend::new(vec![mock_object("hi from hf bare")]));
        let p = HuggingFaceProvider::new(mock, "hf-key");
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from hf bare");
    }

    #[tokio::test]
    async fn url_strips_vendor_prefix_and_carries_auth() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_array("x")]));
        let p = HuggingFaceProvider::new(mock.clone(), "hf-secret");
        let _ = p.complete(&sample_prompt()).await.unwrap();
        let seen = &mock.seen()[0];
        assert!(seen.url.contains("/models/meta-llama/"));
        assert!(!seen.url.contains("huggingface/"));
        assert!(seen
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer hf-secret"));
    }
}
