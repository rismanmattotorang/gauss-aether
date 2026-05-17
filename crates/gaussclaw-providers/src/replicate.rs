//! [`ReplicateProvider`] — Replicate `predictions` API leaf driver.
//!
//! Replicate is asynchronous: a `POST /v1/predictions` returns a
//! prediction record with `status: "starting"`; the caller polls
//! `GET /v1/predictions/{id}` until `status: "succeeded"`. This
//! driver implements the poll loop with a bounded retry budget so a
//! single GaussClaw turn maps to one logical Replicate call.
//!
//! Wire shape:
//!
//! - `POST /v1/predictions` with `{"version":"<model-hash>","input":{"prompt","max_new_tokens"}}`
//! - Response (immediate): `{"id","status":"starting","urls":{"get"}}`
//! - Polled `GET <urls.get>` until `{"status":"succeeded","output":["…"]}`
//!
//! Tests use a [`MockHttpBackend`] preloaded with the create-response
//! immediately followed by the poll-response, so a single test
//! exercises both legs of the call.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gaussclaw_agent::{
    Completion, Prompt, ProviderError, ProviderHandle, ProviderResult, TokenCount,
};
use serde_json::Value;

use crate::backend::{HttpBackend, HttpRequest};

const DEFAULT_BASE_URL: &str = "https://api.replicate.com";

/// Replicate predictions-API leaf driver.
pub struct ReplicateProvider {
    backend: Arc<dyn HttpBackend>,
    base_url: String,
    api_key: String,
    poll_interval: Duration,
    max_polls: u32,
}

impl ReplicateProvider {
    /// Build a new driver with the default polling policy: 200 ms
    /// between polls, up to 300 polls (≈ 60 s).
    #[must_use]
    pub fn new(backend: Arc<dyn HttpBackend>, api_key: impl Into<String>) -> Self {
        Self {
            backend,
            base_url: DEFAULT_BASE_URL.into(),
            api_key: api_key.into(),
            poll_interval: Duration::from_millis(200),
            max_polls: 300,
        }
    }

    /// Override the polling interval.
    #[must_use]
    pub const fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Override the maximum poll count.
    #[must_use]
    pub const fn with_max_polls(mut self, n: u32) -> Self {
        self.max_polls = n;
        self
    }

    /// Override the base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn flatten_messages(prompt: &Prompt) -> String {
        prompt
            .messages
            .iter()
            .map(|m| format!("[{}]\n{}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn build_create_body(prompt: &Prompt) -> Vec<u8> {
        // Replicate calls models by version hash (`abcdef…`). Hermes
        // commonly passes the version as the part after the slash.
        let version = prompt
            .model
            .split_once('/')
            .map_or(prompt.model.as_str(), |(_, v)| v);
        let mut input = serde_json::json!({
            "prompt": Self::flatten_messages(prompt),
        });
        if let Some(mt) = prompt.max_tokens {
            input["max_new_tokens"] = Value::from(mt);
        }
        if let Some(t) = prompt.temperature {
            input["temperature"] = Value::from(t);
        }
        let body = serde_json::json!({
            "version": version,
            "input":   input,
        });
        serde_json::to_vec(&body).unwrap_or_default()
    }
}

#[async_trait]
impl ProviderHandle for ReplicateProvider {
    fn name(&self) -> &'static str {
        "replicate"
    }

    async fn complete(&self, prompt: &Prompt) -> ProviderResult<Completion> {
        let create_req = HttpRequest {
            url: format!("{}/v1/predictions", self.base_url),
            method: "POST".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("authorization".into(), format!("Token {}", self.api_key)),
            ],
            body: Self::build_create_body(prompt),
        };
        let create = self
            .backend
            .send(create_req)
            .await
            .map_err(|e| ProviderError::Transport(format!("{e}")))?;
        if !(200..300).contains(&create.status) {
            return Err(ProviderError::Upstream {
                code: create.status,
                message: String::from_utf8_lossy(&create.body).into_owned(),
            });
        }
        let v: Value =
            serde_json::from_slice(&create.body).map_err(|e| ProviderError::Upstream {
                code: 0,
                message: format!("replicate create parse: {e}"),
            })?;
        let poll_url = v["urls"]["get"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| {
                format!(
                    "{}/v1/predictions/{}",
                    self.base_url,
                    v["id"].as_str().unwrap_or("")
                )
            });

        // Poll loop. The test mock preloads a single succeeded
        // response, so the first poll succeeds; production may take
        // up to ~60 s under the default policy.
        for _ in 0..self.max_polls {
            let poll_req = HttpRequest {
                url: poll_url.clone(),
                method: "GET".into(),
                headers: vec![("authorization".into(), format!("Token {}", self.api_key))],
                body: Vec::new(),
            };
            let poll = self
                .backend
                .send(poll_req)
                .await
                .map_err(|e| ProviderError::Transport(format!("{e}")))?;
            if !(200..300).contains(&poll.status) {
                return Err(ProviderError::Upstream {
                    code: poll.status,
                    message: String::from_utf8_lossy(&poll.body).into_owned(),
                });
            }
            let pv: Value =
                serde_json::from_slice(&poll.body).map_err(|e| ProviderError::Upstream {
                    code: 0,
                    message: format!("replicate poll parse: {e}"),
                })?;
            match pv["status"].as_str().unwrap_or("starting") {
                "succeeded" => {
                    // `output` is typically a Vec<String> we join.
                    let text = if let Some(arr) = pv["output"].as_array() {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("")
                    } else {
                        pv["output"].as_str().unwrap_or("").to_string()
                    };
                    let completion_tokens = u32::try_from(text.len() / 4).unwrap_or(u32::MAX);
                    return Ok(Completion::new(
                        text,
                        prompt.model.clone(),
                        "stop",
                        TokenCount::new(0, completion_tokens),
                    ));
                }
                "failed" | "canceled" => {
                    return Err(ProviderError::Upstream {
                        code: 500,
                        message: pv["error"]
                            .as_str()
                            .unwrap_or("replicate prediction failed")
                            .to_string(),
                    });
                }
                _ => {
                    tokio::time::sleep(self.poll_interval).await;
                }
            }
        }
        Err(ProviderError::Transport(format!(
            "replicate poll budget exhausted after {} attempts",
            self.max_polls
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{HttpResponse, MockHttpBackend};
    use crate::check_postconditions;
    use gaussclaw_agent::Message;

    fn create_resp(id: &str) -> HttpResponse {
        let body = serde_json::json!({
            "id":     id,
            "status": "starting",
            "urls":   {"get": format!("https://api.replicate.com/v1/predictions/{id}")},
        });
        HttpResponse {
            status: 201,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn succeeded_resp(id: &str, parts: &[&str]) -> HttpResponse {
        let body = serde_json::json!({
            "id":     id,
            "status": "succeeded",
            "output": parts,
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn failed_resp(id: &str, why: &str) -> HttpResponse {
        let body = serde_json::json!({
            "id":     id,
            "status": "failed",
            "error":  why,
        });
        HttpResponse {
            status: 200,
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn sample_prompt() -> Prompt {
        Prompt::new(
            "replicate/abcd1234efgh5678ijklmnop",
            vec![Message::new("user", "hi")],
        )
    }

    #[tokio::test]
    async fn create_then_poll_round_trips() {
        let mock = Arc::new(MockHttpBackend::new(vec![
            create_resp("p1"),
            succeeded_resp("p1", &["hi", " from", " replicate"]),
        ]));
        let p = ReplicateProvider::new(mock, "rep-key")
            .with_poll_interval(Duration::from_millis(1))
            .with_max_polls(3);
        let c = p.complete(&sample_prompt()).await.unwrap();
        assert_eq!(c.text, "hi from replicate");
        assert_eq!(c.finish_reason, "stop");
        check_postconditions(&c, Some(4096)).unwrap();
    }

    #[tokio::test]
    async fn failed_prediction_surfaces_as_upstream_error() {
        let mock = Arc::new(MockHttpBackend::new(vec![
            create_resp("p2"),
            failed_resp("p2", "model OOM"),
        ]));
        let p =
            ReplicateProvider::new(mock, "rep-key").with_poll_interval(Duration::from_millis(1));
        let err = p.complete(&sample_prompt()).await.unwrap_err();
        match err {
            ProviderError::Upstream { message, .. } => assert!(message.contains("OOM")),
            other => panic!("expected Upstream, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_budget_exhausts_to_transport_error() {
        // 1 create + 2 "starting" polls — budget = 2, so the third
        // attempt isn't made; transport error returned.
        let starting = HttpResponse {
            status: 200,
            body: serde_json::to_vec(&serde_json::json!({
                "id": "p3", "status": "starting",
                "urls": {"get": "https://api.replicate.com/v1/predictions/p3"}
            }))
            .unwrap(),
        };
        let mock = Arc::new(MockHttpBackend::new(vec![
            create_resp("p3"),
            starting.clone(),
            starting,
        ]));
        let p = ReplicateProvider::new(mock, "rep-key")
            .with_poll_interval(Duration::from_millis(1))
            .with_max_polls(2);
        let err = p.complete(&sample_prompt()).await.unwrap_err();
        assert!(matches!(err, ProviderError::Transport(_)));
    }

    #[tokio::test]
    async fn create_request_carries_token_auth() {
        let mock = Arc::new(MockHttpBackend::new(vec![
            create_resp("p4"),
            succeeded_resp("p4", &["x"]),
        ]));
        let p = ReplicateProvider::new(mock.clone(), "rep-secret")
            .with_poll_interval(Duration::from_millis(1));
        let _ = p.complete(&sample_prompt()).await.unwrap();
        let seen = mock.seen();
        assert!(seen[0]
            .headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Token rep-secret"));
    }
}
