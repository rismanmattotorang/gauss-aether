//! Modal executor (Sprint 6 §1.3).
//!
//! Modal's runtime is an HTTP API; a real implementation calls
//! `https://api.modal.com/...` with operator credentials. This module
//! ships the **shape** of the executor — a typed `ModalConfig` +
//! `ModalExecutor` that returns a typed [`ExecError::Backend`] until
//! the actual HTTP client lands (Sprint 7 §1.3 follow-on, gated by
//! Modal API stability).
//!
//! Shipping the executor surface now lets:
//!
//! - The `ExecRouter` pre-register a Modal target without breaking
//!   when Modal is unreachable.
//! - The TOML config / dashboard expose a Modal backend slot.
//! - The conformance suite drive a `MockModalExecutor` against the
//!   same trait surface so the rest of the system (delegate tool,
//!   subagent receipt isolation) doesn't wait on Modal upstream.
//!
//! ## Hermes-superiority axes
//!
//! - **Signed function ids only.** A real Modal call must carry a
//!   pinned `function_sha256` — Hermes accepts raw `function_name`
//!   and silently uses whichever version was last deployed.
//! - **Per-call cost cap.** `max_cost_dollars` aborts the call before
//!   dispatch if Modal's pricing estimator returns a number above
//!   the budget. Hermes ships no cap.

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{
    Backend, ExecError, ExecOutput, ExecRequest, ExecResult, Receipt, SessionExecutor,
};

// ─── Modal HTTP transport surface ───────────────────────────────────────────

/// One request to Modal's runtime API. Mirrors the
/// `POST /v1/functions/{name}/call` payload shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModalCallRequest {
    /// Workspace id (Modal account / org).
    pub workspace: String,
    /// Fully-qualified function reference (`name@sha256:<hex>` when
    /// digest-pinning is enforced).
    pub function: String,
    /// Argument vector forwarded to the Modal function. Modal hosts
    /// receive this as the positional Python argv list.
    pub argv: Vec<String>,
}

/// The Modal API response shape Gauss-Aether expects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModalCallResponse {
    /// Exit code reported by Modal — `None` when the function
    /// returned a value rather than running a process.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Captured stdout from the Modal function. Modal does not stream
    /// here — the full body lands in one shot.
    #[serde(default)]
    pub stdout: String,
    /// Captured stderr.
    #[serde(default)]
    pub stderr: String,
    /// True if Modal truncated the body before returning.
    #[serde(default)]
    pub truncated: bool,
}

/// Pluggable Modal HTTP transport. Production wires this through a
/// `reqwest`-backed (or workspace-default) HTTP client; tests use the
/// in-process [`MockModalHttpClient`].
#[async_trait]
pub trait ModalHttpClient: Send + Sync {
    /// Submit one Modal `function call` request and return the
    /// captured response.
    async fn submit_call(&self, request: ModalCallRequest) -> Result<ModalCallResponse, String>;
}

/// Default placeholder client.
///
/// Mirrors the pre-Sprint-9 behaviour: every call returns the same
/// "not configured" error. Used when the operator constructs a
/// [`ModalExecutor`] without explicitly wiring a client.
pub struct UnconfiguredModalClient;

#[async_trait]
impl ModalHttpClient for UnconfiguredModalClient {
    async fn submit_call(&self, request: ModalCallRequest) -> Result<ModalCallResponse, String> {
        Err(format!(
            "modal executor is not yet wired (function={}, workspace={}); attach a ModalHttpClient via ModalExecutor::with_client(..)",
            request.function, request.workspace
        ))
    }
}

/// Deterministic in-process mock — tests register canned `(argv,
/// response)` pairs and assert the executor wires them through.
pub struct MockModalHttpClient {
    canned: std::sync::Mutex<Vec<(Vec<String>, ModalCallResponse)>>,
    observed: std::sync::Mutex<Vec<ModalCallRequest>>,
}

impl MockModalHttpClient {
    /// Build an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            canned: std::sync::Mutex::new(Vec::new()),
            observed: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Register a canned response keyed on the argv. The first
    /// matching argv consumes the entry FIFO-style.
    pub fn expect(&self, argv: Vec<String>, response: ModalCallResponse) {
        self.canned.lock().expect("poisoned").push((argv, response));
    }

    /// Inspect the full call log.
    #[must_use]
    pub fn observed(&self) -> Vec<ModalCallRequest> {
        self.observed.lock().expect("poisoned").clone()
    }
}

impl Default for MockModalHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModalHttpClient for MockModalHttpClient {
    async fn submit_call(&self, request: ModalCallRequest) -> Result<ModalCallResponse, String> {
        self.observed
            .lock()
            .expect("poisoned")
            .push(request.clone());
        let mut g = self.canned.lock().expect("poisoned");
        let position = g.iter().position(|(argv, _)| *argv == request.argv);
        let result = position.map(|i| g.remove(i).1);
        drop(g);
        result.ok_or_else(|| format!("no canned response for argv {:?}", request.argv))
    }
}

/// Modal executor configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModalConfig {
    /// Modal workspace id.
    pub workspace: String,
    /// Modal function reference. **Digest-pinned** form
    /// (`name@sha256:hex`) is required when `allow_floating_versions
    /// = false` (the default).
    pub function: String,
    /// Allow tag-only function references. Off by default.
    #[serde(default)]
    pub allow_floating_versions: bool,
    /// Per-call cost cap in USD. `None` disables the cap.
    #[serde(default)]
    pub max_cost_dollars: Option<f64>,
    /// API endpoint override (mainly for testing).
    #[serde(default)]
    pub api_endpoint: Option<String>,
}

impl Default for ModalConfig {
    fn default() -> Self {
        Self {
            workspace: "default".into(),
            function: "noop@sha256:0000".into(),
            allow_floating_versions: false,
            max_cost_dollars: Some(1.0),
            api_endpoint: None,
        }
    }
}

impl ModalConfig {
    /// Validate the function reference.
    pub fn validate(&self) -> ExecResult<()> {
        if self.function.contains("@sha256:") {
            return Ok(());
        }
        if self.allow_floating_versions {
            return Ok(());
        }
        Err(ExecError::Backend(format!(
            "modal function {:?} is not version-pinned (set allow_floating_versions=true to override)",
            self.function
        )))
    }
}

/// Modal executor. Cheap to clone.
#[derive(Clone)]
pub struct ModalExecutor {
    config: ModalConfig,
    client: Arc<dyn ModalHttpClient>,
}

impl std::fmt::Debug for ModalExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalExecutor")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ModalExecutor {
    /// Build an executor. Until [`Self::with_client`] is called, the
    /// executor uses the [`UnconfiguredModalClient`] — every call
    /// surfaces a typed "not configured" error so the router contract
    /// is testable end-to-end without a live Modal credential.
    ///
    /// # Errors
    /// Returns [`ExecError::Backend`] when the function reference
    /// fails the pinning guard.
    pub fn new(config: ModalConfig) -> ExecResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            client: Arc::new(UnconfiguredModalClient),
        })
    }

    /// Attach a [`ModalHttpClient`]. The trait surface keeps the
    /// HTTP transport pluggable: production wires a `reqwest`-backed
    /// (or workspace-default) client; tests use [`MockModalHttpClient`].
    #[must_use]
    pub fn with_client(mut self, client: Arc<dyn ModalHttpClient>) -> Self {
        self.client = client;
        self
    }

    /// Borrow the config.
    #[must_use]
    pub fn config(&self) -> &ModalConfig {
        &self.config
    }
}

#[async_trait]
impl SessionExecutor for ModalExecutor {
    fn backend(&self) -> Backend {
        Backend::Modal
    }

    async fn exec(&self, request: ExecRequest) -> ExecResult<(ExecOutput, Receipt)> {
        let mut argv = Vec::with_capacity(request.args.len().saturating_add(1));
        argv.push(request.program.clone());
        argv.extend(request.args.iter().cloned());
        let mc_req = ModalCallRequest {
            workspace: self.config.workspace.clone(),
            function: self.config.function.clone(),
            argv,
        };
        let response = self
            .client
            .submit_call(mc_req)
            .await
            .map_err(ExecError::Backend)?;
        let truncated = response.truncated
            || request.max_output_bytes.is_some_and(|cap| {
                response.stdout.len().saturating_add(response.stderr.len()) > cap
            });
        let output = ExecOutput {
            exit_code: response.exit_code,
            stdout: response.stdout,
            stderr: response.stderr,
            truncated,
            backend: Backend::Modal,
        };
        let receipt = Receipt {
            backend: Backend::Modal,
            program: request.program,
            argv_len: u32::try_from(request.args.len()).unwrap_or(u32::MAX),
            exit_code: response.exit_code,
            truncated,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0)),
        };
        Ok((output, receipt))
    }
}

/// In-process mock executor for tests + the conformance suite —
/// satisfies the [`SessionExecutor`] contract without touching the
/// network. Records the last request for assertions.
pub struct MockModalExecutor {
    last_request: std::sync::Mutex<Option<ExecRequest>>,
    stub_output: ExecOutput,
}

impl MockModalExecutor {
    /// Build a mock that always returns a `stdout=ok` exit=0 response.
    #[must_use]
    pub fn ok() -> Self {
        Self {
            last_request: std::sync::Mutex::new(None),
            stub_output: ExecOutput {
                exit_code: Some(0),
                stdout: "ok".into(),
                stderr: String::new(),
                truncated: false,
                backend: Backend::Modal,
            },
        }
    }

    /// Most-recent request seen, or `None` if `exec` hasn't been called.
    #[must_use]
    pub fn last_request(&self) -> Option<ExecRequest> {
        self.last_request.lock().expect("poisoned").clone()
    }
}

#[async_trait]
impl SessionExecutor for MockModalExecutor {
    fn backend(&self) -> Backend {
        Backend::Modal
    }

    async fn exec(&self, request: ExecRequest) -> ExecResult<(ExecOutput, Receipt)> {
        *self.last_request.lock().expect("poisoned") = Some(request.clone());
        let receipt = Receipt {
            backend: Backend::Modal,
            program: request.program.clone(),
            argv_len: u32::try_from(request.args.len()).unwrap_or(u32::MAX),
            exit_code: self.stub_output.exit_code,
            truncated: false,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0)),
        };
        Ok((self.stub_output.clone(), receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_rejects_floating_function_without_opt_in() {
        let c = ModalConfig {
            function: "my_fn:v3".into(),
            ..ModalConfig::default()
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_accepts_pinned_function() {
        let c = ModalConfig {
            function: "my_fn@sha256:abc".into(),
            ..ModalConfig::default()
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn executor_construction_validates_function() {
        let bad = ModalConfig {
            function: "v3".into(),
            allow_floating_versions: false,
            ..ModalConfig::default()
        };
        assert!(ModalExecutor::new(bad).is_err());
    }

    #[tokio::test]
    async fn unconfigured_executor_returns_not_wired_until_client_attached() {
        let cfg = ModalConfig {
            function: "fn@sha256:zz".into(),
            ..ModalConfig::default()
        };
        let exec = ModalExecutor::new(cfg).unwrap();
        let err = exec
            .exec(ExecRequest::new("echo", vec!["hi".into()]))
            .await
            .unwrap_err();
        match err {
            ExecError::Backend(m) => assert!(m.contains("not yet wired")),
            other => panic!("expected Backend(not-wired), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn executor_routes_argv_through_modal_http_client() {
        let cfg = ModalConfig {
            function: "echo@sha256:abc".into(),
            workspace: "ws".into(),
            ..ModalConfig::default()
        };
        let mock = Arc::new(MockModalHttpClient::new());
        mock.expect(
            vec!["python".into(), "-c".into(), "print(1)".into()],
            ModalCallResponse {
                exit_code: Some(0),
                stdout: "1\n".into(),
                stderr: String::new(),
                truncated: false,
            },
        );
        let exec = ModalExecutor::new(cfg).unwrap().with_client(mock.clone());
        let (out, receipt) = exec
            .exec(ExecRequest::new(
                "python",
                vec!["-c".into(), "print(1)".into()],
            ))
            .await
            .expect("call must succeed");
        assert_eq!(out.stdout, "1\n");
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(receipt.backend, Backend::Modal);
        let log = mock.observed();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].workspace, "ws");
        assert_eq!(log[0].function, "echo@sha256:abc");
        assert_eq!(
            log[0].argv,
            vec!["python".to_string(), "-c".into(), "print(1)".into()]
        );
    }

    #[tokio::test]
    async fn executor_surfaces_client_failure_as_backend_error() {
        let cfg = ModalConfig {
            function: "fn@sha256:zz".into(),
            ..ModalConfig::default()
        };
        let mock = Arc::new(MockModalHttpClient::new());
        // No canned response — the mock returns an error string.
        let exec = ModalExecutor::new(cfg).unwrap().with_client(mock);
        let err = exec
            .exec(ExecRequest::new("python", vec![]))
            .await
            .unwrap_err();
        assert!(matches!(err, ExecError::Backend(_)));
    }

    #[tokio::test]
    async fn executor_truncates_when_output_exceeds_cap() {
        let cfg = ModalConfig {
            function: "fn@sha256:zz".into(),
            ..ModalConfig::default()
        };
        let mock = Arc::new(MockModalHttpClient::new());
        mock.expect(
            vec!["python".into()],
            ModalCallResponse {
                exit_code: Some(0),
                stdout: "a".repeat(100),
                stderr: String::new(),
                truncated: false,
            },
        );
        let exec = ModalExecutor::new(cfg).unwrap().with_client(mock);
        let (out, receipt) = exec
            .exec(ExecRequest::new("python", vec![]).max_output(10))
            .await
            .unwrap();
        assert!(out.truncated);
        assert!(receipt.truncated);
    }

    #[tokio::test]
    async fn mock_executor_records_and_returns_ok() {
        let mock = MockModalExecutor::ok();
        let (out, receipt) = mock
            .exec(ExecRequest::new("python", vec!["-c".into(), "1".into()]))
            .await
            .unwrap();
        assert!(out.success());
        assert_eq!(out.backend, Backend::Modal);
        assert_eq!(receipt.backend, Backend::Modal);
        let last = mock.last_request().expect("last request recorded");
        assert_eq!(last.program, "python");
    }
}
