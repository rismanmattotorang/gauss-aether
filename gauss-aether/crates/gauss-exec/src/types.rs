//! Core types: `Backend`, `ExecRequest`, `ExecOutput`, `SessionExecutor`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use gauss_core::CapToken;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which backend a request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Backend {
    /// In-process — runs commands in the host process via
    /// `tokio::process::Command`.
    Local,
    /// Docker container.
    Docker,
    /// Remote host over SSH.
    Ssh,
    /// Modal sandbox.
    Modal,
}

impl Backend {
    /// Stable string tag.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Docker => "docker",
            Self::Ssh => "ssh",
            Self::Modal => "modal",
        }
    }

    /// Required capability for this backend.
    #[must_use]
    pub const fn required_cap(self) -> CapToken {
        match self {
            Self::Local => CapToken::EXECUTOR_LOCAL,
            Self::Docker => CapToken::EXECUTOR_DOCKER,
            Self::Ssh => CapToken::EXECUTOR_SSH,
            Self::Modal => CapToken::EXECUTOR_MODAL,
        }
    }
}

/// One exec request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecRequest {
    /// Command to run.
    pub program: String,
    /// Argument vector.
    pub args: Vec<String>,
    /// Environment variables (each as `(KEY, VALUE)` pair).
    pub env: BTreeMap<String, String>,
    /// Working directory; relative to backend-default if `None`.
    pub cwd: Option<PathBuf>,
    /// Soft cap on captured stdout/stderr bytes — `None` for unlimited.
    pub max_output_bytes: Option<usize>,
}

impl ExecRequest {
    /// Build a fresh request from program + args.
    #[must_use]
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            env: BTreeMap::new(),
            cwd: None,
            max_output_bytes: Some(1 << 20),
        }
    }

    /// Set a single env var (chainable).
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set the working directory (chainable).
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set the output cap (chainable).
    #[must_use]
    pub fn max_output(mut self, bytes: usize) -> Self {
        self.max_output_bytes = Some(bytes);
        self
    }
}

/// Captured output from one exec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecOutput {
    /// Process exit code (zero on success). `None` when killed by a
    /// signal (Unix) or the backend can't surface one (e.g. SSH).
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// True if the captured output was truncated by `max_output_bytes`.
    pub truncated: bool,
    /// Backend this ran on (echoed for audit).
    pub backend: Backend,
}

impl ExecOutput {
    /// True iff the process exited normally with status 0.
    #[must_use]
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Receipt of one executor dispatch. Caller appends this to the
/// chain so the trajectory replay names every backend invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// Backend used.
    pub backend: Backend,
    /// Program echoed (for context).
    pub program: String,
    /// Argv length (the actual argv lives in audit).
    pub argv_len: u32,
    /// Exit code captured.
    pub exit_code: Option<i32>,
    /// Whether the captured output was truncated.
    pub truncated: bool,
    /// UNIX seconds when the exec completed.
    pub timestamp: i64,
}

/// Crate-wide error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecError {
    /// Caller's cap grant didn't include the required cap for this
    /// backend.
    #[error("admit refused: required cap 0x{required:016x} not in grant 0x{grant:016x}")]
    AdmitRefused {
        /// Cap bits required.
        required: u64,
        /// Cap bits the caller's grant exposes.
        grant: u64,
    },
    /// Spawn failed (e.g. binary not found).
    #[error("spawn: {0}")]
    Spawn(String),
    /// I/O failure during capture.
    #[error("io: {0}")]
    Io(String),
    /// Backend-side failure.
    #[error("backend: {0}")]
    Backend(String),
}

/// Crate-wide result alias.
pub type ExecResult<T> = Result<T, ExecError>;

impl From<std::io::Error> for ExecError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Pluggable executor.
#[async_trait]
pub trait SessionExecutor: Send + Sync {
    /// Backend tag (for routing + audit).
    fn backend(&self) -> Backend;

    /// Dispatch a request and return the captured output.
    async fn exec(&self, request: ExecRequest) -> ExecResult<(ExecOutput, Receipt)>;
}
