//! HTTP tool family — `http_get`, `http_post`, `http_head`.
//!
//! Hermes ships `http_get` (and friends) as a thin wrapper around
//! `requests.get(url)` with the agent's full process credentials and
//! no taint marking. The output of a Hermes `http_get` is fed back to
//! the next prompt verbatim — the canonical prompt-injection vector.
//!
//! GaussClaw's HTTP family closes four structural gaps:
//!
//! 1. **Capability-gated.** `http_get` / `http_head` require
//!    `network:http_get`; `http_post` requires `network:http_post`.
//!    Tools cannot escalate.
//! 2. **Taint defaults to `Web`.** Output crosses the worker boundary
//!    only after the schema gate strips instruction-substring poisoning;
//!    the typed `ValidatedValue` carries the `Web` taint up to the
//!    next prompt, so the declass map can refuse downstream
//!    `subprocess:spawn` / `network:http_post` requests informed by
//!    web content (Axiom A6).
//! 3. **Injectable transport.** [`HttpClient`] is a trait; the default
//!    implementation refuses every call until the runtime wires in a
//!    real backend (typically a `reqwest`-backed impl behind a
//!    feature flag). Tests pass [`MockHttpClient`] with deterministic
//!    fixtures. Hermes hardcodes `requests`.
//! 4. **Header allowlist + body size cap.** A typed [`HttpToolPolicy`]
//!    bounds which request headers are forwarded and the maximum
//!    response body size. Hermes forwards arbitrary headers and
//!    returns whatever the server sent.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use serde::{Deserialize, Serialize};

// ─── trait + policy ────────────────────────────────────────────────────────

/// Pluggable HTTP transport. Mirrors the providers crate's
/// [`gaussclaw_providers::backend::HttpBackend`] pattern but lives in
/// the tools crate so the dependency graph stays unidirectional.
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Perform one HTTP request. Implementations should respect the
    /// [`HttpToolPolicy`] (header allowlist + max body size) — they
    /// receive an already-filtered request, but the policy is the
    /// canonical record of what the operator agreed to.
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpClientError>;
}

/// One HTTP request, normalised.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HttpRequest {
    /// `GET`, `POST`, `HEAD`.
    pub method: HttpMethod,
    /// Absolute URL.
    pub url: String,
    /// Request headers, post-allowlist filtering.
    pub headers: BTreeMap<String, String>,
    /// Optional UTF-8 body for `POST`.
    pub body: Option<String>,
}

/// One HTTP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HttpResponse {
    /// 3-digit status code.
    pub status: u16,
    /// Response headers.
    pub headers: BTreeMap<String, String>,
    /// Response body, truncated to the policy's `max_body_bytes`.
    pub body: String,
    /// `true` when [`HttpResponse::body`] was truncated by the policy.
    pub truncated: bool,
}

/// Supported request methods. `HEAD` and `GET` use the same cap;
/// `POST` is its own cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum HttpMethod {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
    /// HTTP HEAD.
    Head,
}

impl HttpMethod {
    /// Stable verb string.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Head => "HEAD",
        }
    }
}

/// HTTP client error surface.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpClientError {
    /// Transport-level failure (DNS, TLS, socket).
    #[error("transport: {0}")]
    Transport(String),
    /// Server returned an HTTP status outside `2xx`.
    #[error("server returned {status}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Response body text.
        body: String,
    },
    /// Operator policy refused the request (header outside allowlist,
    /// body too big, URL scheme disallowed).
    #[error("policy denied: {0}")]
    PolicyDenied(String),
    /// Default backend is not wired — production deployments must
    /// inject a real [`HttpClient`].
    #[error(
        "HttpClient not configured for this deployment; inject one before invoking http_* tools"
    )]
    NotConfigured,
}

/// Operator-controlled policy applied at tool-construction time.
#[derive(Debug, Clone)]
pub struct HttpToolPolicy {
    /// Request-header names that the tool will forward (lower-cased
    /// during the filter). Default: empty (no headers forwarded).
    pub header_allowlist: Vec<String>,
    /// Maximum response-body bytes returned to the agent. Default 64 KiB.
    pub max_body_bytes: usize,
    /// URL schemes the tool accepts. Default: `["https"]`.
    pub allowed_schemes: Vec<String>,
}

impl Default for HttpToolPolicy {
    fn default() -> Self {
        Self {
            header_allowlist: vec![],
            max_body_bytes: 64 * 1024,
            allowed_schemes: vec!["https".into()],
        }
    }
}

impl HttpToolPolicy {
    /// Apply the policy to a candidate request. Returns the filtered
    /// request, or a [`HttpClientError::PolicyDenied`] when the URL
    /// scheme is outside the allow-list.
    ///
    /// # Errors
    /// [`HttpClientError::PolicyDenied`] when the URL scheme is not
    /// in [`Self::allowed_schemes`].
    pub fn filter(&self, mut req: HttpRequest) -> Result<HttpRequest, HttpClientError> {
        let scheme = req
            .url
            .split_once("://")
            .map(|(s, _)| s.to_ascii_lowercase())
            .unwrap_or_default();
        if !self
            .allowed_schemes
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&scheme))
        {
            return Err(HttpClientError::PolicyDenied(format!(
                "scheme `{scheme}` not in allowlist {:?}",
                self.allowed_schemes
            )));
        }
        let allow: std::collections::BTreeSet<String> = self
            .header_allowlist
            .iter()
            .map(|h| h.to_ascii_lowercase())
            .collect();
        req.headers
            .retain(|k, _| allow.contains(&k.to_ascii_lowercase()));
        Ok(req)
    }

    /// Truncate the body to [`Self::max_body_bytes`]. Sets the
    /// `truncated` flag when truncation occurs.
    pub fn truncate(&self, mut resp: HttpResponse) -> HttpResponse {
        if resp.body.len() > self.max_body_bytes {
            // Truncate on a char boundary closest to the cap.
            let mut end = self.max_body_bytes;
            while end > 0 && !resp.body.is_char_boundary(end) {
                end -= 1;
            }
            resp.body.truncate(end);
            resp.truncated = true;
        }
        resp
    }
}

// ─── default + mock clients ────────────────────────────────────────────────

/// Default [`HttpClient`] used when the runtime doesn't inject a real
/// backend. Every call returns [`HttpClientError::NotConfigured`]; this
/// keeps the tool registry uniform without making the workspace depend
/// on `reqwest` until a deployment actually needs it.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnconfiguredHttpClient;

#[async_trait]
impl HttpClient for UnconfiguredHttpClient {
    async fn request(&self, _req: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        Err(HttpClientError::NotConfigured)
    }
}

/// Deterministic in-process [`HttpClient`] used by tests.
#[derive(Debug, Default, Clone)]
pub struct MockHttpClient {
    /// Map of `<METHOD> <URL>` → response.
    responses: std::sync::Arc<std::sync::Mutex<BTreeMap<String, HttpResponse>>>,
    /// Calls observed in order. Useful for assertions.
    pub calls: std::sync::Arc<std::sync::Mutex<Vec<HttpRequest>>>,
}

impl MockHttpClient {
    /// Construct an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a response keyed by `<METHOD> <URL>`.
    pub fn expect(&self, method: HttpMethod, url: impl Into<String>, response: HttpResponse) {
        let key = format!("{} {}", method.verb(), url.into());
        self.responses.lock().unwrap().insert(key, response);
    }

    /// Snapshot every observed request.
    #[must_use]
    pub fn observed(&self) -> Vec<HttpRequest> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl HttpClient for MockHttpClient {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpClientError> {
        self.calls.lock().unwrap().push(req.clone());
        let key = format!("{} {}", req.method.verb(), req.url);
        self.responses
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or_else(|| HttpClientError::Transport(format!("no mock for `{key}`")))
    }
}

// ─── manifests ─────────────────────────────────────────────────────────────

const HTTP_GET_TOML: &str = r#"
name        = "http_get"
description = "HTTP GET an allow-listed URL. Output taint=Web; schema-gated before re-entering the prompt."
usage       = "Use for read-only fetches over HTTPS. Args: {url: string, headers?: map}."
caps        = ["network:http_get"]
taint       = "web"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

const HTTP_POST_TOML: &str = r#"
name        = "http_post"
description = "HTTP POST a JSON body to an allow-listed URL. Output taint=Web."
usage       = "Use for write-side calls (forms, webhooks). Args: {url: string, body?: string, headers?: map}."
caps        = ["network:http_post"]
taint       = "web"
reversible  = false
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

const HTTP_HEAD_TOML: &str = r#"
name        = "http_head"
description = "HTTP HEAD probe (no body). Returns status + headers."
usage       = "Use to check reachability / content-type / Last-Modified."
caps        = ["network:http_get"]
taint       = "web"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 8192

[schema]
type = "object"
"#;

// ─── tools ─────────────────────────────────────────────────────────────────

/// One HTTP tool. Three constructors — [`HttpTool::get`], [`HttpTool::post`],
/// [`HttpTool::head`] — share a single backing implementation.
pub struct HttpTool {
    manifest: ToolManifest,
    method: HttpMethod,
    client: Arc<dyn HttpClient>,
    policy: HttpToolPolicy,
}

impl HttpTool {
    fn build(
        name: &'static str,
        toml: &str,
        method: HttpMethod,
        client: Arc<dyn HttpClient>,
    ) -> Self {
        let skill = SkillManifest::from_toml(toml).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId(name.into()))
            .expect("embedded skill compiles");
        Self {
            manifest,
            method,
            client,
            policy: HttpToolPolicy::default(),
        }
    }

    /// `http_get` constructor.
    #[must_use]
    pub fn get(client: Arc<dyn HttpClient>) -> Self {
        Self::build("http_get", HTTP_GET_TOML, HttpMethod::Get, client)
    }

    /// `http_post` constructor.
    #[must_use]
    pub fn post(client: Arc<dyn HttpClient>) -> Self {
        Self::build("http_post", HTTP_POST_TOML, HttpMethod::Post, client)
    }

    /// `http_head` constructor.
    #[must_use]
    pub fn head(client: Arc<dyn HttpClient>) -> Self {
        Self::build("http_head", HTTP_HEAD_TOML, HttpMethod::Head, client)
    }

    /// Override the operator policy.
    #[must_use]
    pub fn with_policy(mut self, policy: HttpToolPolicy) -> Self {
        self.policy = policy;
        self
    }
}

#[async_trait]
impl ToolTrait for HttpTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `url`".into()))?
            .to_owned();
        let body = args.get("body").and_then(|v| v.as_str()).map(str::to_owned);
        let headers = args
            .get("headers")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect::<BTreeMap<String, String>>()
            })
            .unwrap_or_default();

        let req = HttpRequest {
            method: self.method,
            url,
            headers,
            body,
        };
        let filtered = self
            .policy
            .filter(req)
            .map_err(|e| GaussError::Internal(format!("http policy: {e}")))?;
        let response = self
            .client
            .request(filtered)
            .await
            .map_err(|e| GaussError::Internal(format!("http: {e}")))?;
        let truncated = self.policy.truncate(response);

        let mut out = serde_json::json!({
            "status":    truncated.status,
            "headers":   truncated.headers,
            "truncated": truncated.truncated,
        });
        // HEAD omits the body field by convention.
        if !matches!(self.method, HttpMethod::Head) {
            out["body"] = serde_json::Value::String(truncated.body);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: BTreeMap::new(),
            body: body.into(),
            truncated: false,
        }
    }

    #[tokio::test]
    async fn http_get_returns_body() {
        let mock = MockHttpClient::new();
        mock.expect(HttpMethod::Get, "https://example.test/", resp(200, "hello"));
        let tool = HttpTool::get(Arc::new(mock.clone()));
        let out = tool
            .invoke_raw(serde_json::json!({ "url": "https://example.test/" }))
            .await
            .unwrap();
        assert_eq!(out["status"], 200);
        assert_eq!(out["body"], "hello");
        assert_eq!(out["truncated"], false);
        assert_eq!(mock.observed().len(), 1);
    }

    #[tokio::test]
    async fn http_post_forwards_body() {
        let mock = MockHttpClient::new();
        mock.expect(
            HttpMethod::Post,
            "https://example.test/echo",
            resp(201, "{\"ok\":true}"),
        );
        let tool = HttpTool::post(Arc::new(mock.clone()));
        tool.invoke_raw(serde_json::json!({
            "url": "https://example.test/echo",
            "body": "{\"x\":1}",
        }))
        .await
        .unwrap();
        let calls = mock.observed();
        assert_eq!(calls[0].body.as_deref(), Some("{\"x\":1}"));
    }

    #[tokio::test]
    async fn http_head_omits_body_field() {
        let mock = MockHttpClient::new();
        let mut r = resp(200, "");
        r.headers.insert("content-type".into(), "text/html".into());
        mock.expect(HttpMethod::Head, "https://example.test/", r);
        let tool = HttpTool::head(Arc::new(mock));
        let out = tool
            .invoke_raw(serde_json::json!({ "url": "https://example.test/" }))
            .await
            .unwrap();
        assert!(!out.as_object().unwrap().contains_key("body"));
        assert_eq!(out["headers"]["content-type"], "text/html");
    }

    #[tokio::test]
    async fn policy_filters_disallowed_scheme() {
        let mock = MockHttpClient::new();
        let tool = HttpTool::get(Arc::new(mock));
        let err = tool
            .invoke_raw(serde_json::json!({ "url": "http://example.test/" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn policy_filters_headers_not_in_allowlist() {
        let mock = MockHttpClient::new();
        mock.expect(HttpMethod::Get, "https://example.test/", resp(200, "x"));
        let tool = HttpTool::get(Arc::new(mock.clone())).with_policy(HttpToolPolicy {
            header_allowlist: vec!["accept".into()],
            ..Default::default()
        });
        tool.invoke_raw(serde_json::json!({
            "url": "https://example.test/",
            "headers": { "Accept": "text/html", "Cookie": "leak=1" },
        }))
        .await
        .unwrap();
        let req = mock.observed().into_iter().next().unwrap();
        assert!(req.headers.contains_key("Accept"));
        assert!(!req.headers.contains_key("Cookie"));
    }

    #[tokio::test]
    async fn policy_truncates_oversized_body() {
        let mock = MockHttpClient::new();
        let big: String = "a".repeat(100_000);
        mock.expect(HttpMethod::Get, "https://example.test/", resp(200, &big));
        let tool = HttpTool::get(Arc::new(mock)).with_policy(HttpToolPolicy {
            max_body_bytes: 1024,
            ..Default::default()
        });
        let out = tool
            .invoke_raw(serde_json::json!({ "url": "https://example.test/" }))
            .await
            .unwrap();
        assert_eq!(out["truncated"], true);
        assert_eq!(out["body"].as_str().unwrap().len(), 1024);
    }

    #[tokio::test]
    async fn unconfigured_client_refuses() {
        let tool = HttpTool::get(Arc::new(UnconfiguredHttpClient));
        let err = tool
            .invoke_raw(serde_json::json!({ "url": "https://example.test/" }))
            .await
            .unwrap_err();
        match err {
            GaussError::Internal(msg) => assert!(msg.contains("not configured")),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
