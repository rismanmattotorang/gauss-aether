//! Production [`HttpBackend`] backed by [`reqwest`].
//!
//! Plumbing-only crate seam: every vendor codec in this crate consumes
//! an `Arc<dyn HttpBackend>` and never touches `reqwest` directly. The
//! bin (or any embedder) constructs one [`ReqwestBackend`] per
//! deployment, hands it to [`crate::ProviderChoice::with_backend`], and
//! the rest of the provider plane wires up unchanged.
//!
//! ## Safety / robustness contract
//!
//! - Default per-request timeout: 30 s (overridable via [`ReqwestBackend::with_timeout`]).
//! - TLS chain verification: rustls + native roots (compiled in at
//!   workspace level — `default-features = false`, `features = ["rustls",
//!   "rustls-native-certs", "json"]`).
//! - Response body cap: 8 MiB (overridable). Bodies larger than the cap
//!   surface as [`HttpError::Upstream`] with a truncation marker rather
//!   than OOMing the process.
//! - Retries / backoff / circuit-breaking are explicitly **not** done
//!   here. The agent loop's fallback chain owns retry policy; the
//!   transport must be a thin honest layer.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::backend::{HttpBackend, HttpError, HttpRequest, HttpResponse};

/// Default per-request timeout. Mirrors the comment in
/// `backend.rs` ("apply a sensible default timeout (15-30 s)").
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default response body cap (8 MiB). Vendor completions are typically
/// well under 1 MiB; anything past 8 MiB is almost certainly a
/// misbehaving upstream or an abuse vector.
pub const DEFAULT_BODY_CAP: usize = 8 * 1024 * 1024;

/// Production `reqwest`-backed transport.
#[derive(Clone)]
pub struct ReqwestBackend {
    client: reqwest::Client,
    timeout: Duration,
    body_cap: usize,
}

impl std::fmt::Debug for ReqwestBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReqwestBackend")
            .field("timeout", &self.timeout)
            .field("body_cap", &self.body_cap)
            .finish()
    }
}

impl ReqwestBackend {
    /// Build a backend with sane defaults: rustls, native roots, 30 s
    /// timeout, 8 MiB body cap.
    pub fn new() -> Result<Self, HttpError> {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(30))
            .user_agent(concat!("gaussclaw/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| HttpError::Network(format!("build reqwest client: {e}")))?;
        Ok(Self {
            client,
            timeout: DEFAULT_TIMEOUT,
            body_cap: DEFAULT_BODY_CAP,
        })
    }

    /// Build an `Arc<dyn HttpBackend>` directly. Convenience for
    /// `ProviderChoice::with_backend(ReqwestBackend::shared()?)`.
    pub fn shared() -> Result<Arc<dyn HttpBackend>, HttpError> {
        Ok(Arc::new(Self::new()?))
    }

    /// Builder: override the per-request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builder: override the response body cap.
    #[must_use]
    pub fn with_body_cap(mut self, cap: usize) -> Self {
        self.body_cap = cap;
        self
    }
}

#[async_trait]
impl HttpBackend for ReqwestBackend {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| HttpError::Network(format!("bad method {:?}: {e}", req.method)))?;

        let mut builder = self.client.request(method, &req.url).timeout(self.timeout);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        if !req.body.is_empty() {
            builder = builder.body(req.body.clone());
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| HttpError::Network(format!("{e}")))?;
        let status = resp.status().as_u16();

        // Stream the body with a hard cap so a runaway upstream cannot
        // exhaust process memory. We collect chunk-by-chunk and bail as
        // soon as we exceed the cap.
        let mut body = Vec::new();
        let cap = self.body_cap;
        let mut stream = resp;
        loop {
            let chunk = match stream.chunk().await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => return Err(HttpError::Network(format!("read body: {e}"))),
            };
            if body.len().saturating_add(chunk.len()) > cap {
                // Preserve enough body for diagnostics, then signal the
                // truncation. The vendor codec will (typically) fail to
                // parse the truncated JSON and surface a clean error.
                let remaining = cap.saturating_sub(body.len());
                body.extend_from_slice(&chunk[..remaining.min(chunk.len())]);
                let snippet = String::from_utf8_lossy(&body[..body.len().min(512)]).to_string();
                return Err(HttpError::Upstream {
                    status,
                    body: format!(
                        "response body exceeded {cap} bytes (truncated); first 512B: {snippet}"
                    ),
                });
            }
            body.extend_from_slice(&chunk);
        }

        if status >= 400 {
            let snippet =
                String::from_utf8_lossy(&body[..body.len().min(2048)]).to_string();
            return Err(HttpError::Upstream {
                status,
                body: snippet,
            });
        }
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_default_backend() {
        let b = ReqwestBackend::new().expect("build");
        assert_eq!(b.timeout, DEFAULT_TIMEOUT);
        assert_eq!(b.body_cap, DEFAULT_BODY_CAP);
    }

    #[test]
    fn builders_override_defaults() {
        let b = ReqwestBackend::new()
            .expect("build")
            .with_timeout(Duration::from_secs(5))
            .with_body_cap(1024);
        assert_eq!(b.timeout, Duration::from_secs(5));
        assert_eq!(b.body_cap, 1024);
    }

    #[tokio::test]
    async fn bad_method_surfaces_as_network_error() {
        let b = ReqwestBackend::new().expect("build");
        let req = HttpRequest {
            url: "https://127.0.0.1:1/".into(),
            method: "NOT A METHOD".into(),
            headers: vec![],
            body: vec![],
        };
        let err = b.send(req).await.expect_err("bad method");
        match err {
            HttpError::Network(msg) => assert!(msg.contains("bad method")),
            other => panic!("expected Network, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unroutable_target_surfaces_as_network_error() {
        // 127.0.0.1:1 — refused connection in nearly all environments.
        let b = ReqwestBackend::new()
            .expect("build")
            .with_timeout(Duration::from_millis(250));
        let req = HttpRequest {
            url: "http://127.0.0.1:1/".into(),
            method: "POST".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        };
        let err = b.send(req).await.expect_err("unroutable");
        match err {
            HttpError::Network(_) => {}
            other => panic!("expected Network, got {other:?}"),
        }
    }
}
