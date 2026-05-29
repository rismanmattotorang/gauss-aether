//! Production [`HttpBackend`] implementation over [`reqwest`].
//!
//! Sprint 1 of the production-hardening track. Every vendor codec in
//! `gaussclaw-providers` (`AnthropicProvider`, `OpenAIProvider`, the
//! 18 others) delegates transport to an `Arc<dyn HttpBackend>`. Until
//! now the only impls were the test-only `MockHttpBackend` and the
//! fail-closed `UnconfiguredBackend` — so no vendor call ever reached
//! the wire. This type is the real transport: plug a
//! [`ReqwestProviderBackend`] into `ProviderChoice::with_backend` and
//! the codecs start making live requests with no further changes.
//!
//! ## Why a second reqwest type
//!
//! [`crate::ReqwestHttpClient`] implements `gaussclaw_tools::HttpClient`
//! — the *tool* boundary (web_fetch, http, web_search). It returns a
//! `String` body truncated to a cap, because tool output is fed back
//! into a prompt and must be bounded.
//!
//! Vendor codecs need the opposite: the **complete, untruncated** JSON
//! response bytes, because a truncated body is unparseable. So the two
//! seams stay distinct. Both share `reqwest`, a 30 s default timeout,
//! and rustls TLS with the OS root store.
//!
//! ## Status handling
//!
//! The backend returns `Ok(HttpResponse { status, body })` for **every
//! completed HTTP exchange**, including non-2xx — the vendor codec owns
//! the status check (it maps non-2xx to `ProviderError::Upstream` with
//! the vendor's own error envelope). The backend reserves
//! [`HttpError::Network`] for genuine transport failures (DNS, TLS,
//! connect, timeout, body read) so the codec can distinguish "the
//! vendor said no" from "we never reached the vendor".

use std::time::Duration;

use async_trait::async_trait;
use gaussclaw_providers::{HttpBackend, HttpError, HttpRequest, HttpResponse};

/// Default per-request wall-clock timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default response-body cap (64 MiB). Vendor JSON responses are small;
/// the cap exists only to bound a pathological/hostile upstream. Unlike
/// the tool client, the codec backend *errors* on overflow rather than
/// truncating, because a half-read JSON body cannot be parsed.
const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// `reqwest`-backed [`HttpBackend`] for the vendor-codec plane.
///
/// Cheap to clone — internal state is shared via `reqwest::Client`'s own
/// `Arc<>` machinery.
#[derive(Clone, Debug)]
pub struct ReqwestProviderBackend {
    inner: reqwest::Client,
    max_body_bytes: usize,
}

/// Builder for [`ReqwestProviderBackend`].
#[derive(Debug, Clone)]
pub struct ReqwestProviderBackendBuilder {
    timeout: Duration,
    max_body_bytes: usize,
    user_agent: String,
}

impl Default for ReqwestProviderBackendBuilder {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            user_agent: format!("gaussclaw-http/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl ReqwestProviderBackendBuilder {
    /// Fresh builder with the safe defaults (30 s timeout, 64 MiB body
    /// cap, identifying User-Agent).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the per-request timeout.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Override the response-body byte cap. Responses larger than this
    /// fail with [`HttpError::Network`] rather than being truncated.
    #[must_use]
    pub const fn max_body_bytes(mut self, n: usize) -> Self {
        self.max_body_bytes = n;
        self
    }

    /// Override the `User-Agent` header sent on every request.
    #[must_use]
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// Build the backend.
    ///
    /// # Errors
    /// Returns [`HttpError::Network`] when the underlying
    /// `reqwest::Client` cannot be built (e.g. invalid TLS config).
    pub fn build(self) -> Result<ReqwestProviderBackend, HttpError> {
        let inner = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(self.user_agent)
            .build()
            .map_err(|e| HttpError::Network(format!("client build: {e}")))?;
        Ok(ReqwestProviderBackend {
            inner,
            max_body_bytes: self.max_body_bytes,
        })
    }
}

impl ReqwestProviderBackend {
    /// Build a backend with the safe defaults.
    ///
    /// # Errors
    /// See [`ReqwestProviderBackendBuilder::build`].
    pub fn new() -> Result<Self, HttpError> {
        ReqwestProviderBackendBuilder::default().build()
    }

    /// Open a builder for non-default configuration.
    #[must_use]
    pub fn builder() -> ReqwestProviderBackendBuilder {
        ReqwestProviderBackendBuilder::default()
    }
}

#[async_trait]
impl HttpBackend for ReqwestProviderBackend {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        // Vendor codecs only ever emit POST / GET today, but match
        // explicitly so an unexpected method fails loudly rather than
        // silently defaulting.
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| HttpError::Network(format!("invalid method {:?}: {e}", req.method)))?;
        let mut builder = self.inner.request(method, &req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        if !req.body.is_empty() {
            builder = builder.body(req.body);
        }
        let response = builder
            .send()
            .await
            .map_err(|e| HttpError::Network(format!("send: {e}")))?;
        let status = response.status().as_u16();
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| HttpError::Network(format!("read body: {e}")))?;
        if body_bytes.len() > self.max_body_bytes {
            return Err(HttpError::Network(format!(
                "response body {} bytes exceeds cap {} bytes",
                body_bytes.len(),
                self.max_body_bytes
            )));
        }
        Ok(HttpResponse {
            status,
            body: body_bytes.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn json_req(url: String, body: &[u8]) -> HttpRequest {
        HttpRequest {
            url,
            method: "POST".into(),
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_vec(),
        }
    }

    #[tokio::test]
    async fn post_round_trips_full_json_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
            .mount(&server)
            .await;
        let backend = ReqwestProviderBackend::new().expect("backend");
        let resp = backend
            .send(json_req(format!("{}/v1/messages", server.uri()), b"{}"))
            .await
            .expect("send");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, br#"{"ok":true}"#);
    }

    #[tokio::test]
    async fn non_2xx_returns_ok_with_status_for_codec_to_handle() {
        // The codec — not the backend — maps non-2xx to Upstream. The
        // backend must surface the status + body verbatim.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(429).set_body_string(r#"{"error":"rate_limited"}"#))
            .mount(&server)
            .await;
        let backend = ReqwestProviderBackend::new().expect("backend");
        let resp = backend
            .send(json_req(format!("{}/v1/messages", server.uri()), b"{}"))
            .await
            .expect("send");
        assert_eq!(resp.status, 429);
        assert_eq!(resp.body, br#"{"error":"rate_limited"}"#);
    }

    #[tokio::test]
    async fn forwards_authorization_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/x"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let backend = ReqwestProviderBackend::new().expect("backend");
        let req = HttpRequest {
            url: format!("{}/x", server.uri()),
            method: "POST".into(),
            headers: vec![("authorization".into(), "Bearer sk-test".into())],
            body: b"{}".to_vec(),
        };
        let resp = backend.send(req).await.expect("send");
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn oversized_body_errors_rather_than_truncates() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(4096)))
            .mount(&server)
            .await;
        let backend = ReqwestProviderBackend::builder()
            .max_body_bytes(100)
            .build()
            .expect("backend");
        let err = backend
            .send(json_req(format!("{}/big", server.uri()), b"{}"))
            .await
            .expect_err("oversized body must error");
        assert!(matches!(err, HttpError::Network(msg) if msg.contains("exceeds cap")));
    }

    #[tokio::test]
    async fn unreachable_host_surfaces_as_network_error() {
        let backend = ReqwestProviderBackend::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .expect("backend");
        // RFC 5737 documentation block; guaranteed unreachable.
        let err = backend
            .send(json_req("http://192.0.2.1:1/nope".into(), b"{}"))
            .await
            .expect_err("unreachable must fail");
        assert!(matches!(err, HttpError::Network(_)));
    }

    #[tokio::test]
    async fn end_to_end_through_anthropic_codec() {
        use std::sync::Arc;

        use gaussclaw_agent::{Message, Prompt, ProviderHandle};
        use gaussclaw_providers::AnthropicProvider;

        let server = MockServer::start().await;
        let body = serde_json::json!({
            "content": [{ "type": "text", "text": "hello from the wire" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 3, "output_tokens": 4 },
        });
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let backend = Arc::new(ReqwestProviderBackend::new().expect("backend"));
        let provider = AnthropicProvider::new(backend, "sk-x").with_base_url(server.uri());
        let prompt = Prompt::new(
            "claude-3.5-sonnet",
            vec![Message::new("user", "hi".to_string())],
        );
        let completion = provider.complete(&prompt).await.expect("complete");
        assert_eq!(completion.text, "hello from the wire");
    }
}
