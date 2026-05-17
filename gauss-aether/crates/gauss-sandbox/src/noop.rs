//! `NoOpSandbox` — accepts every request, executes nothing.
//!
//! Useful in two contexts:
//!
//! 1. **Tests** that exercise the surrounding plumbing (DTE wiring, conformance
//!    harness) without spending build time on real WASM execution.
//! 2. **Phase-0/1/2 backward compatibility** — the engine before Phase 3 used
//!    an in-process `apply_actions_locally` that effectively did nothing for
//!    Phase 2's text-only tests. `NoOpSandbox` is the structural replacement.
//!
//! In production builds the no-op MUST NOT be reachable; the
//! [`crate::CompositeSandbox`] enforces this by requiring at least L1.

use async_trait::async_trait;
use gauss_core::{CapToken, GaussResult};
use gauss_traits::{SandboxClass, SandboxOutcome, SandboxRequest, SandboxTrait};

/// Always-accepting sandbox. **Test / debug only.**
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpSandbox;

#[async_trait]
impl SandboxTrait for NoOpSandbox {
    fn class(&self, _cap: CapToken) -> SandboxClass {
        SandboxClass::NONE
    }

    async fn exec(&self, _request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        Ok(SandboxOutcome::ok(Vec::new(), Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;

    #[tokio::test]
    async fn noop_accepts_everything() {
        let sb = NoOpSandbox;
        let out = sb
            .exec(SandboxRequest::new(
                ToolId("echo".into()),
                CapToken::FILESYSTEM_READ,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
        assert!(out.layers_invoked.is_empty());
    }
}
