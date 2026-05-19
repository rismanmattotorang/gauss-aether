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

use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{
    Backend, ExecError, ExecOutput, ExecRequest, ExecResult, Receipt, SessionExecutor,
};

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
#[derive(Debug, Clone)]
pub struct ModalExecutor {
    config: ModalConfig,
}

impl ModalExecutor {
    /// Build an executor.
    ///
    /// # Errors
    /// Returns [`ExecError::Backend`] when the function reference
    /// fails the pinning guard.
    pub fn new(config: ModalConfig) -> ExecResult<Self> {
        config.validate()?;
        Ok(Self { config })
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

    async fn exec(&self, _request: ExecRequest) -> ExecResult<(ExecOutput, Receipt)> {
        // The real HTTP client lands in a Sprint 7 follow-on. Until
        // then we surface a typed "not configured" error so the
        // router contract is testable end-to-end.
        Err(ExecError::Backend(format!(
            "modal executor is not yet wired (function={}, workspace={}); HTTP client lands in Sprint 7",
            self.config.function, self.config.workspace
        )))
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
    async fn real_executor_returns_not_wired_until_sprint7() {
        let cfg = ModalConfig {
            function: "fn@sha256:zz".into(),
            ..ModalConfig::default()
        };
        let exec = ModalExecutor::new(cfg).unwrap();
        let err = exec
            .exec(ExecRequest::new("echo", vec!["hi".into()]))
            .await
            .unwrap_err();
        assert!(matches!(err, ExecError::Backend(_)));
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
