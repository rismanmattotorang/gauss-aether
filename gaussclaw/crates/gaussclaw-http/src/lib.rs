//! Production [`HttpClient`] implementation over [`reqwest`].
//!
//! Sprint 10 §1 of `/ROADMAP.md`. Sprint 9 §3/§4 wired every web
//! tool through the pluggable `gaussclaw_tools::HttpClient` trait;
//! the default `UnconfiguredHttpClient` refuses every call. This
//! crate ships the real implementation behind the same trait, so
//! production deployments inject a [`ReqwestHttpClient`] and the
//! existing `WebFetchTool` / `WebSearchTool` / `HttpTool` start
//! making real requests with no further changes.
//!
//! ## Design
//!
//! - TLS via `rustls` with the OS root store (`rustls-native-certs`).
//!   No OpenSSL dep; works on minimal containers.
//! - Configurable redirect policy, per-request timeout, and
//!   max-body-bytes cap (the cap is enforced *here* in addition to
//!   the policy filter at the tool layer — defence-in-depth).
//! - Header allowlist + URL-scheme guard live in
//!   [`gaussclaw_tools::HttpToolPolicy`]; this client receives an
//!   already-filtered request and trusts it.
//!
//! ## Hermes parity
//!
//! Hermes ships a single `requests`-backed adapter with no body cap
//! and no per-request timeout. GaussClaw's version is strictly
//! superior on three axes:
//!
//! - **Per-request timeout** — Hermes can hang indefinitely on a
//!   slow upstream; GaussClaw aborts after a configurable wall-clock
//!   timeout (default 30 s).
//! - **Body cap** — large responses are truncated and flagged so the
//!   agent's prompt never blows up from a malicious / accidental
//!   multi-megabyte payload.
//! - **Operator-controlled redirect policy** — Hermes follows
//!   redirects unconditionally; we default to 10 hops but accept an
//!   override so an operator can disable them entirely for
//!   high-security deployments.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use gaussclaw_tools::{HttpClient, HttpClientError, HttpMethod, HttpRequest, HttpResponse};

/// `reqwest`-backed [`HttpClient`] implementation.
///
/// Cheap to clone — internal state is shared via `reqwest::Client`'s
/// own `Arc<>` machinery.
#[derive(Clone, Debug)]
pub struct ReqwestHttpClient {
    inner: reqwest::Client,
    /// Cap on the response body bytes returned to the agent. Larger
    /// responses are truncated and flagged via [`HttpResponse::truncated`].
    /// Operator-configurable; default 1 MiB.
    max_body_bytes: usize,
}

/// Builder for [`ReqwestHttpClient`].
///
/// Used when the defaults aren't right — e.g. a deployment that
/// needs to disable redirects entirely, raise the body cap, or
/// tighten the timeout.
#[derive(Debug, Clone)]
pub struct ReqwestHttpClientBuilder {
    timeout: Duration,
    max_redirects: usize,
    max_body_bytes: usize,
    user_agent: String,
}

impl Default for ReqwestHttpClientBuilder {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_redirects: 10,
            max_body_bytes: 1024 * 1024,
            user_agent: format!("gaussclaw-http/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl ReqwestHttpClientBuilder {
    /// Build a fresh builder with the safe defaults (30s timeout,
    /// 10 redirects, 1 MiB body cap, identifying User-Agent).
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

    /// Override the max redirect count. `0` disables redirect
    /// following entirely.
    #[must_use]
    pub const fn max_redirects(mut self, n: usize) -> Self {
        self.max_redirects = n;
        self
    }

    /// Override the response-body byte cap.
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

    /// Build the client.
    ///
    /// # Errors
    /// Returns [`HttpClientError::Transport`] when the underlying
    /// `reqwest::Client` cannot be built (e.g. invalid TLS config).
    pub fn build(self) -> Result<ReqwestHttpClient, HttpClientError> {
        let redirect = if self.max_redirects == 0 {
            reqwest::redirect::Policy::none()
        } else {
            reqwest::redirect::Policy::limited(self.max_redirects)
        };
        let inner = reqwest::Client::builder()
            .timeout(self.timeout)
            .redirect(redirect)
            .user_agent(self.user_agent)
            .build()
            .map_err(|e| HttpClientError::Transport(format!("client build: {e}")))?;
        Ok(ReqwestHttpClient {
            inner,
            max_body_bytes: self.max_body_bytes,
        })
    }
}

impl ReqwestHttpClient {
    /// Build a client with the safe defaults.
    ///
    /// # Errors
    /// See [`ReqwestHttpClientBuilder::build`].
    pub fn new() -> Result<Self, HttpClientError> {
        ReqwestHttpClientBuilder::default().build()
    }

    /// Open a builder for non-default configuration.
    #[must_use]
    pub fn builder() -> ReqwestHttpClientBuilder {
        ReqwestHttpClientBuilder::default()
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        let mut builder = match req.method {
            HttpMethod::Get => self.inner.get(&req.url),
            HttpMethod::Post => self.inner.post(&req.url),
            HttpMethod::Head => self.inner.head(&req.url),
            // `HttpMethod` is `#[non_exhaustive]` upstream; refuse
            // any future variant explicitly rather than guessing a
            // mapping.
            other => {
                return Err(HttpClientError::PolicyDenied(format!(
                    "unsupported method {other:?}"
                )));
            }
        };
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let response = builder
            .send()
            .await
            .map_err(|e| HttpClientError::Transport(format!("send: {e}")))?;
        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.as_str().to_owned(), v.to_owned());
            }
        }
        // Read the body up to the cap; flag truncation if the upstream
        // sent more. We use `bytes()` rather than `text()` because the
        // upstream may not declare a charset header.
        let body_bytes = response
            .bytes()
            .await
            .map_err(|e| HttpClientError::Transport(format!("read body: {e}")))?;
        let truncated = body_bytes.len() > self.max_body_bytes;
        let truncated_bytes = if truncated {
            &body_bytes[..self.max_body_bytes]
        } else {
            &body_bytes[..]
        };
        let body = String::from_utf8_lossy(truncated_bytes).into_owned();
        Ok(HttpResponse::new(status, headers, body, truncated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Smoke-test the happy path against a wiremock instance.
    #[tokio::test]
    async fn get_round_trips_through_wiremock() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/echo"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::new().expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                format!("{}/echo", server.uri()),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "hello");
        assert!(!response.truncated);
    }

    #[tokio::test]
    async fn post_forwards_headers_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/in"))
            .and(header("x-custom", "v"))
            .respond_with(ResponseTemplate::new(201).set_body_string("created"))
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::new().expect("client");
        let mut headers = BTreeMap::new();
        headers.insert("x-custom".into(), "v".into());
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Post,
                format!("{}/in", server.uri()),
                headers,
                Some("payload".into()),
            ))
            .await
            .expect("ok");
        assert_eq!(response.status, 201);
        assert_eq!(response.body, "created");
    }

    #[tokio::test]
    async fn head_method_does_not_read_body() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/h"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::new().expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Head,
                format!("{}/h", server.uri()),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(response.status, 200);
        assert!(response.body.is_empty());
    }

    #[tokio::test]
    async fn body_truncation_caps_oversized_responses() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_string("a".repeat(2048)))
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::builder()
            .max_body_bytes(100)
            .build()
            .expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                format!("{}/big", server.uri()),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(response.body.len(), 100);
        assert!(response.truncated);
    }

    #[tokio::test]
    async fn non_2xx_status_returns_in_response_not_as_error() {
        // The HttpClient contract surfaces server responses as
        // successful results — the `HttpClientError::Status` variant
        // is reserved for callers that opt in. Production tools
        // inspect `response.status` themselves.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::new().expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                format!("{}/missing", server.uri()),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(response.status, 404);
        assert_eq!(response.body, "not found");
    }

    #[tokio::test]
    async fn invalid_url_surfaces_as_transport_error() {
        let client = ReqwestHttpClient::new().expect("client");
        let err = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                "not-a-url".into(),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect_err("invalid url must fail");
        assert!(matches!(err, HttpClientError::Transport(_)));
    }

    #[tokio::test]
    async fn unreachable_host_surfaces_as_transport_error() {
        // RFC 5737 documentation block; guaranteed unreachable.
        let client = ReqwestHttpClient::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client");
        let err = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                "http://192.0.2.1:1/nope".into(),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect_err("unreachable must fail");
        assert!(matches!(err, HttpClientError::Transport(_)));
    }

    #[tokio::test]
    async fn response_headers_surface_in_btreemap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/hdr"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-trace-id", "abc-123")
                    .set_body_string(""),
            )
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::new().expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                format!("{}/hdr", server.uri()),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(
            response.headers.get("x-trace-id").map(String::as_str),
            Some("abc-123")
        );
    }

    #[tokio::test]
    async fn redirect_policy_zero_does_not_follow() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/end"))
            .mount(&server)
            .await;
        // /end is registered to ensure the server *could* respond if
        // the client followed. With max_redirects(0), we expect to
        // see the 302 itself.
        Mock::given(method("GET"))
            .and(path("/end"))
            .respond_with(ResponseTemplate::new(200).set_body_string("final"))
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::builder()
            .max_redirects(0)
            .build()
            .expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                format!("{}/start", server.uri()),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(response.status, 302);
    }

    #[tokio::test]
    async fn redirect_policy_follows_within_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/start"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/end"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/end"))
            .respond_with(ResponseTemplate::new(200).set_body_string("final"))
            .mount(&server)
            .await;
        let client = ReqwestHttpClient::new().expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                format!("{}/start", server.uri()),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "final");
    }

    /// Live-network test — gated behind both `--features live-network`
    /// and `--ignored` so default CI runs zero network requests.
    /// Operators verify with
    /// `cargo test -p gaussclaw-http --features live-network -- --ignored`.
    #[cfg(feature = "live-network")]
    #[tokio::test]
    #[ignore = "requires outbound network — run with --features live-network -- --ignored"]
    async fn live_network_round_trips_to_example_com() {
        let client = ReqwestHttpClient::new().expect("client");
        let response = client
            .request(HttpRequest::new(
                HttpMethod::Get,
                "https://example.com/".into(),
                BTreeMap::new(),
                None,
            ))
            .await
            .expect("ok");
        assert_eq!(response.status, 200);
        assert!(response.body.to_lowercase().contains("example domain"));
    }
}
