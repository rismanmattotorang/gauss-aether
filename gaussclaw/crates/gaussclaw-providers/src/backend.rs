//! [`HttpBackend`] — minimal trait separating wire shape from transport.
//!
//! Every leaf vendor driver in this crate owns its **canonical request
//! / response codec** (e.g. `AnthropicProvider` knows the Messages API
//! shape) and delegates the transport to an `Arc<dyn HttpBackend>`.
//!
//! ## Why this seam exists
//!
//! Hermes upstream hardcodes the Python `requests` library inside each
//! `backends/*.py` driver. Swapping HTTP libraries means touching every
//! file. GaussClaw moves the wire shape into the driver and the
//! transport behind an async trait — production builds plug `reqwest`
//! into one place; tests plug [`MockHttpBackend`] into the same place.
//!
//! ## Production wiring
//!
//! The `reqwest`-backed implementation lives in `gaussclaw-http` as
//! [`ReqwestProviderBackend`](https://docs.rs/gaussclaw-http) and is
//! wired into the vendor codecs by `gaussclaw-bin` (see
//! `build_provider_choice`). It honours a per-request timeout, rustls
//! TLS verification against the OS root store, and a response-body cap.
//! Tests plug [`MockHttpBackend`] into the same seam.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use thiserror::Error;

/// HTTP transport errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HttpError {
    /// Network / DNS / socket failure.
    #[error("network: {0}")]
    Network(String),
    /// Vendor returned a non-2xx response.
    #[error("upstream {status}: {body}")]
    Upstream {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated to a sensible cap by the transport).
        body: String,
    },
    /// Request body / response body failed to parse as JSON.
    #[error("serde: {0}")]
    Serde(String),
    /// Test-only: the mock backend ran out of canned responses.
    #[error("mock exhausted at request #{0}")]
    MockExhausted(u32),
}

/// One HTTP request.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// Target URL (full, including scheme).
    pub url: String,
    /// HTTP method (`"POST"`, `"GET"`).
    pub method: String,
    /// Request headers as `(name, value)`. Vendor drivers add `Authorization`
    /// + `Content-Type` here.
    pub headers: Vec<(String, String)>,
    /// JSON body (already serialised).
    pub body: Vec<u8>,
}

/// One HTTP response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body bytes (typically JSON).
    pub body: Vec<u8>,
}

/// Async HTTP transport trait. Drivers depend only on this — never on
/// `reqwest`/`hyper`/`ureq` directly.
#[async_trait]
pub trait HttpBackend: Send + Sync {
    /// Send `req` and return the response (or an error). Backends are
    /// expected to:
    ///
    /// - apply a sensible default timeout (15-30 s).
    /// - verify TLS chains.
    /// - leave retry / backoff / circuit-breaking to the caller.
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
}

// ─── mock backend (deterministic for CI) ───────────────────────────────────

/// Deterministic in-memory backend. Tests construct one with a queue
/// of canned `HttpResponse` values; each `send` pops the next.
///
/// The mock records every request so tests can assert wire-shape
/// invariants (the vendor-specific `Content-Type` / `Authorization`
/// header, the JSON body fields, etc.).
pub struct MockHttpBackend {
    state: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    canned: VecDeque<Result<HttpResponse, HttpError>>,
    seen: Vec<HttpRequest>,
    served: u32,
}

impl MockHttpBackend {
    /// Build a mock with a queue of canned responses (in order).
    #[must_use]
    pub fn new<I: IntoIterator<Item = HttpResponse>>(canned: I) -> Self {
        Self {
            state: Mutex::new(MockState {
                canned: canned.into_iter().map(Ok).collect(),
                seen: Vec::new(),
                served: 0,
            }),
        }
    }

    /// Build a mock that always returns the same response.
    #[must_use]
    pub fn always(response: HttpResponse) -> Self {
        // We can't repeat infinitely in a finite queue; instead a single
        // canned entry plus a clone-on-pop trick. For simplicity here,
        // we pre-seed 1024 copies — enough for any sensible test.
        let mut canned: VecDeque<Result<HttpResponse, HttpError>> = VecDeque::with_capacity(1024);
        for _ in 0..1024 {
            canned.push_back(Ok(response.clone()));
        }
        Self {
            state: Mutex::new(MockState {
                canned,
                seen: Vec::new(),
                served: 0,
            }),
        }
    }

    /// Append additional canned responses.
    pub fn push(&self, response: HttpResponse) {
        self.state.lock().unwrap().canned.push_back(Ok(response));
    }

    /// Return the list of requests the backend has seen so far.
    pub fn seen(&self) -> Vec<HttpRequest> {
        self.state.lock().unwrap().seen.clone()
    }

    /// Number of requests served.
    pub fn served(&self) -> u32 {
        self.state.lock().unwrap().served
    }
}

#[async_trait]
impl HttpBackend for MockHttpBackend {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut st = self.state.lock().unwrap();
        st.seen.push(req);
        let served = st.served;
        st.served = served.saturating_add(1);
        match st.canned.pop_front() {
            Some(r) => r,
            None => Err(HttpError::MockExhausted(served)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_serves_canned_responses_in_order() {
        let mock = MockHttpBackend::new(vec![
            HttpResponse {
                status: 200,
                body: b"first".to_vec(),
            },
            HttpResponse {
                status: 200,
                body: b"second".to_vec(),
            },
        ]);
        let req = HttpRequest {
            url: "http://example.com".into(),
            method: "POST".into(),
            headers: vec![],
            body: vec![],
        };
        let r1 = mock.send(req.clone()).await.unwrap();
        assert_eq!(r1.body, b"first");
        let r2 = mock.send(req.clone()).await.unwrap();
        assert_eq!(r2.body, b"second");
        // Exhausted on third call.
        let err = mock.send(req).await.unwrap_err();
        assert!(matches!(err, HttpError::MockExhausted(2)));
    }

    #[tokio::test]
    async fn mock_records_seen_requests() {
        let mock = MockHttpBackend::new(vec![HttpResponse {
            status: 200,
            body: vec![],
        }]);
        let req = HttpRequest {
            url: "http://x".into(),
            method: "POST".into(),
            headers: vec![("authorization".into(), "Bearer xyz".into())],
            body: br#"{"k":"v"}"#.to_vec(),
        };
        let _ = mock.send(req).await;
        let seen = mock.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, "http://x");
        assert_eq!(seen[0].headers[0].1, "Bearer xyz");
    }
}
