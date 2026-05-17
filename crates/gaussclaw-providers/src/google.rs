//! [`GoogleProvider`] — Google Gemini `generateContent` leaf driver.
//!
//! Wire shape:
//!
//! - `POST /v1beta/models/{model}:generateContent?key=…`
//! - Body: `{"contents":[{"role","parts":[{"text"}]}], "generationConfig":{"maxOutputTokens","temperature"}}`
//! - Response: `{"candidates":[{"content":{"parts":[{"text"}]}, "finishReason"}], "usageMetadata":{"promptTokenCount","candidatesTokenCount"}}`
//!
//! Gemini uses Google's `role` set: `user` and `model` (instead of
//! OpenAI's `assistant`). The driver maps GaussClaw's `assistant`
//! role onto `model` on outbound; system messages are flattened to
//! the top-level `systemInstruction` field. `finishReason` values
//! (`STOP`, `MAX_TOKENS`, `SAFETY`, `RECITATION`, `OTHER`) map onto
//! the canonical postcondition set.

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_VERSION: &str = "v1beta";

/// Google Gemini generateContent leaf driver.
pub struct GoogleProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
    api_key: String,
    version: String,
}

impl GoogleProvider {
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

    /// Override the base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override the API version (default `v1beta`).
    #[must_use]
    pub fn with_version(mut self, v: impl Into<String>) -> Self {
        self.version = v.into();
        self
    }

    /// Build the request body. System messages are hoisted to the
    /// top-level `systemInstruction`; the `assistant` role maps onto
    /// Gemini's `model`.
    fn build_body(prompt: &Prompt) -> Vec<u8> {
        let mut system_parts: Vec<String> = Vec::new();
        let mut contents: Vec<Value> = Vec::new();
        for m in &prompt.messages {
            match m.role.as_str() {
                "system" => system_parts.push(m.content.clone()),
                _ => {
                    let role = if m.role == "assistant" {
                        "model"
                    } else {
                        "user"
                    };
                    contents.push(serde_json::json!({
                        "role": role,
                        "parts": [{"text": m.content}],
                    }));
                }
            }
        }
        let mut body = serde_json::json!({ "contents": contents });
        if !system_parts.is_empty() {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": system_parts.join("\n\n")}],
            });
        }
        let mut gen_cfg = serde_json::Map::new();
        if let Some(mt) = prompt.max_tokens {
            gen_cfg.insert("maxOutputTokens".into(), Value::from(mt));
        }
        if let Some(t) = prompt.temperature {
            gen_cfg.insert("temperature".into(), Value::from(t));
        }
        if !gen_cfg.is_empty() {
            body["generationConfig"] = Value::Object(gen_cfg);
        }
        serde_json::to_vec(&body).unwrap_or_default()
    }

    /// Strip the `google/` vendor prefix if present (Gemini doesn't
    /// use it internally; e.g. `google/gemini-1.5-pro` → `gemini-1.5-pro`).
    fn model_id_for_url(model: &str) -> &str {
        model.strip_prefix("google/").unwrap_or(model)
    }

    fn parse_response(model: &str, raw: &[u8]) -> ProviderResult<Completion> {
        let v: Value = serde_json::from_slice(raw).map_err(|e| ProviderError::Upstream {
            code: 0,
            message: format!("google response parse: {e}"),
        })?;
        let text = v["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let finish_reason = match v["candidates"][0]["finishReason"]
            .as_str()
            .unwrap_or("STOP")
        {
            "STOP" => "stop",
            "MAX_TOKENS" => "length",
            "SAFETY" | "RECITATION" => "content_filter",
            _ => "stop",
        }
        .to_string();
        let prompt_tokens = v["usageMetadata"]["promptTokenCount"].as_u64().unwrap_or(0);
        let completion_tokens = v["usageMetadata"]["candidatesTokenCount"]
            .as_u64()
            .unwrap_or(0);
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
impl ProviderHandle for GoogleProvider {
    fn name(&self) -> &'static str {
        "google"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let model_for_url = Self::model_id_for_url(&prompt.model);
        let req = HttpRequest {
            url: format!(
                "{}/{}/models/{}:generateContent?key={}",
                self.base_url, self.version, model_for_url, self.api_key
            ),
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

    fn mock_response(text: &str, finish: &str) -> HttpResponse {
        let body = serde_json::json!({
            "candidates": [{
                "content": {"parts": [{"text": text}], "role": "model"},
                "finishReason": finish,
            }],
            "usageMetadata": {
                "promptTokenCount": 4,
                "candidatesTokenCount": 6,
                "totalTokenCount": 10,
            },
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt::new(
            "google/gemini-1.5-pro",
            vec![
                Message::new("system", "you are a wire test fixture"),
                Message::new("user", "hello"),
                Message::new("assistant", "hi"),
            ],
        )
    }

    #[tokio::test]
    async fn build_body_extracts_system_instruction_and_maps_assistant_role() {
        let body = GoogleProvider::build_body(&sample_prompt());
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["systemInstruction"]["parts"][0]["text"],
            "you are a wire test fixture"
        );
        let contents = v["contents"].as_array().unwrap();
        // System role removed from contents; user + assistant remain.
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model"); // assistant → model
    }

    #[tokio::test]
    async fn complete_round_trips_through_mock() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response(
            "hi from gemini",
            "STOP",
        )]));
        let p = GoogleProvider::new(mock.clone(), "gk-test-key");
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from gemini");
        assert_eq!(c.finish_reason, "stop");
        assert_eq!(c.usage.prompt, 4);
        assert_eq!(c.usage.completion, 6);
        check_postconditions(&c, Some(1024)).unwrap();
    }

    #[tokio::test]
    async fn url_carries_api_key_and_canonical_model() {
        let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x", "STOP")]));
        let p = GoogleProvider::new(mock.clone(), "secret-key");
        let _ = p.complete(&sample_prompt()).await.unwrap();
        let seen = &mock.seen()[0];
        assert!(seen.url.contains("models/gemini-1.5-pro:generateContent"));
        assert!(seen.url.contains("key=secret-key"));
        // Vendor prefix stripped — Gemini's URL doesn't carry "google/".
        assert!(!seen.url.contains("google/"));
    }

    #[tokio::test]
    async fn finish_reasons_map_correctly() {
        for (raw, canonical) in [
            ("STOP", "stop"),
            ("MAX_TOKENS", "length"),
            ("SAFETY", "content_filter"),
            ("RECITATION", "content_filter"),
        ] {
            let mock = Arc::new(MockHttpBackend::new(vec![mock_response("x", raw)]));
            let p = GoogleProvider::new(mock, "k");
            let c = p.complete(&sample_prompt()).await.unwrap();
            assert_eq!(c.finish_reason, canonical, "raw={raw}");
        }
    }
}
