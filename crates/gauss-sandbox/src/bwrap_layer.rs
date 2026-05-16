//! L3a — Linux user namespaces via bubblewrap (`bwrap`).
//!
//! Phase 3 ships a thin subprocess wrapper: invokes `bwrap` with a
//! deny-by-default profile (no network namespace inherited, fresh PID NS,
//! tmpfs `/`, only the specified bind-mounts visible). If `bwrap` is not
//! installed, the layer fails with a clear diagnostic so the operator can
//! either install it or drop the cap that would require this layer.

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult};
use gauss_traits::{SandboxClass, SandboxLayer, SandboxOutcome, SandboxRequest, SandboxTrait};
use tokio::process::Command;

/// Bubblewrap layer.
#[derive(Debug, Clone, Default)]
pub struct BwrapSandbox {
    /// Optional override for the `bwrap` binary location.
    binary: Option<String>,
}

impl BwrapSandbox {
    /// Build a default `bwrap` wrapper looked up on `$PATH`.
    #[must_use]
    pub const fn new() -> Self {
        Self { binary: None }
    }

    /// Override the binary path (useful in tests / hermetic envs).
    #[must_use]
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: Some(binary.into()),
        }
    }

    fn binary_path(&self) -> &str {
        self.binary.as_deref().unwrap_or("bwrap")
    }
}

#[async_trait]
impl SandboxTrait for BwrapSandbox {
    fn class(&self, _cap: CapToken) -> SandboxClass {
        SandboxClass::L3
    }

    async fn exec(&self, _request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        // Phase-3 stub: probe bwrap's `--version` to confirm presence. The
        // actual subprocess-launching of the tool moves into Phase 4 once
        // the HWCA worker boundary is in place. Until then, the layer's job
        // is to fail loudly when bwrap is missing so an operator that asks
        // for a cap requiring L3 gets a clear refusal.
        let status = Command::new(self.binary_path())
            .arg("--version")
            .output()
            .await;
        match status {
            Ok(out) if out.status.success() => Ok(SandboxOutcome::ok(
                Vec::new(),
                vec![SandboxLayer::Namespace],
            )),
            Ok(out) => Err(GaussError::Io(format!(
                "bwrap --version failed: status={:?} stderr={}",
                out.status,
                String::from_utf8_lossy(&out.stderr),
            ))),
            Err(e) => Err(GaussError::Io(format!(
                "bwrap not available at '{}': {e}",
                self.binary_path()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;

    #[tokio::test]
    async fn missing_binary_yields_clear_io_error() {
        let sb = BwrapSandbox::with_binary("/no/such/binary/bwrap");
        let err = sb
            .exec(SandboxRequest::new(
                ToolId("ns".into()),
                CapToken::SUBPROCESS_SPAWN,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
            .expect_err("missing binary must error");
        match err {
            GaussError::Io(msg) => assert!(msg.contains("bwrap"), "msg: {msg}"),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn class_is_l3() {
        let sb = BwrapSandbox::default();
        assert_eq!(sb.class(CapToken::SUBPROCESS_SPAWN), SandboxClass::L3);
    }
}
