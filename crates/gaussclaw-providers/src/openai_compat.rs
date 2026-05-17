//! [`OpenAICompatProvider`] — generic driver for OpenAI-Chat-Completions-
//! compatible vendors.
//!
//! A surprisingly large fraction of the Hermes backend catalogue speaks
//! the same wire shape: Groq, Cerebras, Fireworks, DeepSeek, Mistral,
//! Together, Anyscale, OctoAI, vLLM, TGI, Replicate (some endpoints) —
//! all expose `POST /v1/chat/completions` with the OpenAI Chat
//! Completions request/response schema.
//!
//! GaussClaw collapses that into one driver parameterised by:
//!
//! - **vendor** name (`"groq"`, `"cerebras"`, …) — used as the provider
//!   `name()` and the receipt-chain attribution.
//! - **base URL** — the vendor's API root (e.g. `https://api.groq.com`).
//! - **auth scheme** — `Bearer <key>` is the universal default;
//!   builders can override the auth header name for vendors with a
//!   non-OpenAI convention.
//!
//! This is the structural Hermes-superiority gain for slice 4: instead
//! of porting ten near-identical Python files, GaussClaw ships **one
//! driver + ten configurations**. Vendor parity in the catalogue is a
//! `LeafModel::new(...)` call away.

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

/// Generic OpenAI-Chat-Completions-compatible driver.
pub struct OpenAICompatProvider {
    backend: Arc<dyn HttpBackend>,
    vendor: &'static str,
    base_url: String,
    api_key: String,
    auth_header: String,
    auth_scheme: String,
}

impl OpenAICompatProvider {
    /// Build a driver for one OpenAI-compat vendor.
    ///
    /// `vendor` is the static name surfaced via [`ProviderHandle::name`]
    /// (and used in receipt-chain attribution). `base_url` is the
    /// vendor's API root (e.g. `"https://api.groq.com"`). `api_key`
    /// is the secret; the default auth header is `Authorization` with
    /// the `Bearer <key>` scheme.
    #[must_use]
    pub fn new(
        backend: Arc<dyn HttpBackend>,
        vendor: &'static str,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            backend,
            vendor,
            base_url: base_url.into(),
            api_key: api_key.into(),
            auth_header: "authorization".into(),
            auth_scheme: "Bearer ".into(),
        }
    }

    /// Override the auth header name (vendors like Anthropic use
    /// `x-api-key` — but Anthropic isn't OpenAI-compat, so this knob
    /// is for vendors like Hugging Face that follow a different
    /// convention while keeping the Chat Completions body shape).
    #[must_use]
    pub fn with_auth_header(mut self, name: impl Into<String>) -> Self {
        self.auth_header = name.into();
        self
    }

    /// Override the auth-scheme prefix (default `"Bearer "`).
    #[must_use]
    pub fn with_auth_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.auth_scheme = scheme.into();
        self
    }

    fn build_body(prompt: &Prompt) -> Vec<u8> {
        let messages: Vec<Value> = prompt
            .messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
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
            message: format!("oai-compat response parse: {e}"),
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
impl ProviderHandle for OpenAICompatProvider {
    fn name(&self) -> &'static str {
        self.vendor
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let req = HttpRequest {
            url: format!("{}/v1/chat/completions", self.base_url),
            method: "POST".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                (
                    self.auth_header.clone(),
                    format!("{}{}", self.auth_scheme, self.api_key),
                ),
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

// ─── factory helpers for the catalogue ─────────────────────────────────────

/// Builder factory for **Groq** (`https://api.groq.com`).
#[must_use]
pub fn groq(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "groq", "https://api.groq.com", api_key)
}

/// Builder factory for **Cerebras** (`https://api.cerebras.ai`).
#[must_use]
pub fn cerebras(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "cerebras", "https://api.cerebras.ai", api_key)
}

/// Builder factory for **Fireworks** (`https://api.fireworks.ai/inference`).
#[must_use]
pub fn fireworks(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(
        backend,
        "fireworks",
        "https://api.fireworks.ai/inference",
        api_key,
    )
}

/// Builder factory for **DeepSeek** (`https://api.deepseek.com`).
#[must_use]
pub fn deepseek(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "deepseek", "https://api.deepseek.com", api_key)
}

/// Builder factory for **Mistral** (`https://api.mistral.ai`).
#[must_use]
pub fn mistral(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "mistral", "https://api.mistral.ai", api_key)
}

/// Builder factory for **Together** (`https://api.together.xyz`).
#[must_use]
pub fn together(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "together", "https://api.together.xyz", api_key)
}

/// Builder factory for **xAI Grok** (`https://api.x.ai`).
#[must_use]
pub fn xai(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "xai", "https://api.x.ai", api_key)
}

/// Builder factory for **Perplexity** (`https://api.perplexity.ai`).
#[must_use]
pub fn perplexity(
    backend: Arc<dyn HttpBackend>,
    api_key: impl Into<String>,
) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "perplexity", "https://api.perplexity.ai", api_key)
}

/// Builder factory for **local vLLM** (`http://localhost:8000`).
#[must_use]
pub fn vllm(backend: Arc<dyn HttpBackend>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "vllm", "http://localhost:8000", String::new())
}

/// Builder factory for **local Text-Generation-Inference**
/// (`http://localhost:3000`).
#[must_use]
pub fn tgi(backend: Arc<dyn HttpBackend>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "tgi", "http://localhost:3000", String::new())
}

/// Builder factory for **OctoAI** (`https://text.octoai.run`).
#[must_use]
pub fn octoai(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> OpenAICompatProvider {
    OpenAICompatProvider::new(backend, "octoai", "https://text.octoai.run", api_key)
}

/// Builder factory for **Anyscale Endpoints**
/// (`https://api.endpoints.anyscale.com`).
#[must_use]
pub fn anyscale(
    backend: Arc<dyn HttpBackend>,
    api_key: impl Into<String>,
) -> OpenAICompatProvider {
    OpenAICompatProvider::new(
        backend,
        "anyscale",
        "https://api.endpoints.anyscale.com",
        api_key,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{HttpResponse, MockHttpBackend};
    use crate::check_postconditions;
    use gaussclaw_agent::Message;

    fn mock_response(text: &str) -> HttpResponse {
        let body = serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt(model: &str) -> Prompt {
        Prompt::new(model, vec![Message::new("user", "hi")])
    }

    #[tokio::test]
    async fn groq_round_trips() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("hi from groq")]));
        let p = groq(mock.clone(), "gsk-test");
        assert_eq!(p.name(), "groq");
        let c = p.complete(&sample_prompt("groq/llama-3.3-70b")).await.unwrap();
        assert_eq!(c.text, "hi from groq");
        check_postconditions(&c, Some(1024)).unwrap();
        let seen = mock.seen();
        assert!(seen[0].url.contains("api.groq.com"));
        assert!(seen[0]
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer gsk-test"));
    }

    #[tokio::test]
    async fn cerebras_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = cerebras(mock.clone(), "csk-x");
        let _ = p.complete(&sample_prompt("cerebras/llama-3.1-70b")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.cerebras.ai"));
    }

    #[tokio::test]
    async fn fireworks_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = fireworks(mock.clone(), "fw-x");
        let _ = p.complete(&sample_prompt("fireworks/llama-v3p1-70b")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.fireworks.ai"));
    }

    #[tokio::test]
    async fn deepseek_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = deepseek(mock.clone(), "ds-x");
        let _ = p.complete(&sample_prompt("deepseek/v3")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.deepseek.com"));
    }

    #[tokio::test]
    async fn mistral_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = mistral(mock.clone(), "mst-x");
        let _ = p.complete(&sample_prompt("mistral/large")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.mistral.ai"));
    }

    #[tokio::test]
    async fn together_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = together(mock.clone(), "tg-x");
        let _ = p.complete(&sample_prompt("together/llama")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.together.xyz"));
    }

    #[tokio::test]
    async fn xai_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = xai(mock.clone(), "xai-x");
        let _ = p.complete(&sample_prompt("xai/grok-2")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.x.ai"));
    }

    #[tokio::test]
    async fn perplexity_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = perplexity(mock.clone(), "pplx-x");
        let _ = p.complete(&sample_prompt("perplexity/llama")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.perplexity.ai"));
    }

    #[tokio::test]
    async fn vllm_local_uses_localhost() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = vllm(mock.clone());
        let _ = p.complete(&sample_prompt("vllm/llama")).await.unwrap();
        assert!(mock.seen()[0].url.contains("localhost:8000"));
    }

    #[tokio::test]
    async fn tgi_local_uses_localhost() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = tgi(mock.clone());
        let _ = p.complete(&sample_prompt("tgi/llama")).await.unwrap();
        assert!(mock.seen()[0].url.contains("localhost:3000"));
    }

    #[tokio::test]
    async fn octoai_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = octoai(mock.clone(), "octo-x");
        let _ = p.complete(&sample_prompt("octoai/mixtral")).await.unwrap();
        assert!(mock.seen()[0].url.contains("text.octoai.run"));
    }

    #[tokio::test]
    async fn anyscale_uses_correct_url() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = anyscale(mock.clone(), "any-x");
        let _ = p.complete(&sample_prompt("anyscale/llama")).await.unwrap();
        assert!(mock.seen()[0].url.contains("api.endpoints.anyscale.com"));
    }

    #[tokio::test]
    async fn custom_auth_header_is_honoured() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x")]));
        let p = OpenAICompatProvider::new(mock.clone(), "custom", "http://x", "secret")
            .with_auth_header("x-custom-token")
            .with_auth_scheme("Token ");
        let _ = p.complete(&sample_prompt("custom/m")).await.unwrap();
        let h = &mock.seen()[0].headers;
        assert!(h.iter().any(|(k, v)| k == "x-custom-token" && v == "Token secret"));
    }
}
