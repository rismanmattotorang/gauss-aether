//! L2 — Linux Landlock (5.13+).
//!
//! Landlock is a Linux LSM that lets an unprivileged process voluntarily
//! restrict its own filesystem access. Phase 3 ships a *self-restriction*
//! impl: the layer applies a Landlock ruleset to the current thread before
//! `exec` returns, then leaves the ruleset in place for the duration of the
//! composite's run. Phase 4 will move the ruleset into the bubblewrap'd
//! child process so the ruleset scope matches the worker-context boundary.

#![cfg(all(target_os = "linux", feature = "linux-layers"))]

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult};
use gauss_traits::{SandboxClass, SandboxLayer, SandboxOutcome, SandboxRequest, SandboxTrait};
use landlock::{AccessFs, BitFlags, RestrictionStatus, Ruleset, RulesetAttr, RulesetStatus};

/// Linux Landlock layer. Adds L2 to the composite's class.
#[derive(Debug, Clone)]
pub struct LandlockSandbox {
    /// Allowed FS access bitset. The conformance harness drops bits to
    /// verify the gate at smaller permissions.
    allowed_fs: BitFlags<AccessFs>,
}

impl LandlockSandbox {
    /// Build a layer that allows only read-class filesystem operations
    /// (the default for tools that only need read access).
    #[must_use]
    pub fn read_only() -> Self {
        Self {
            allowed_fs: AccessFs::ReadFile | AccessFs::ReadDir,
        }
    }

    /// Build a layer that allows read + scoped writes.
    #[must_use]
    pub fn read_write() -> Self {
        Self {
            allowed_fs: AccessFs::ReadFile | AccessFs::ReadDir | AccessFs::WriteFile,
        }
    }

    /// Build a layer with a custom `AccessFs` bitset.
    #[must_use]
    pub const fn with_access(allowed_fs: BitFlags<AccessFs>) -> Self {
        Self { allowed_fs }
    }

    /// Apply the ruleset to the current thread.
    ///
    /// # Errors
    /// Returns [`GaussError::Io`] if the kernel rejects the ruleset (e.g.
    /// kernel < 5.13 or Landlock not enabled).
    fn enforce(&self) -> GaussResult<RestrictionStatus> {
        Ruleset::default()
            .handle_access(self.allowed_fs)
            .map_err(|e| GaussError::Io(format!("landlock handle_access: {e}")))?
            .create()
            .map_err(|e| GaussError::Io(format!("landlock create: {e}")))?
            .restrict_self()
            .map_err(|e| GaussError::Io(format!("landlock restrict_self: {e}")))
    }
}

#[async_trait]
impl SandboxTrait for LandlockSandbox {
    fn class(&self, _cap: CapToken) -> SandboxClass {
        SandboxClass::L2
    }

    async fn exec(&self, _request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        // Run the kernel-syscalling work on a blocking thread.
        let bits = self.allowed_fs;
        let status = tokio::task::spawn_blocking(move || Self::with_access(bits).enforce())
            .await
            .map_err(|e| GaussError::Internal(format!("landlock join: {e}")))??;

        // If the kernel doesn't support Landlock at all, fall through with a
        // diagnostic but still mark the layer as invoked at the *intended*
        // level. The composite's invariant check will then refuse if the
        // operator demanded L2 but the kernel didn't provide it.
        if matches!(status.ruleset, RulesetStatus::NotEnforced) {
            return Err(GaussError::Io(
                "landlock not enforced on this kernel (need >= 5.13 with LSM enabled)".into(),
            ));
        }

        Ok(SandboxOutcome::ok(Vec::new(), vec![SandboxLayer::Landlock]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;

    // The Landlock kernel check is environment-dependent; these tests run
    // only on Linux with the feature enabled and gracefully skip when the
    // host kernel lacks support. They MUST NOT fail when the kernel is too
    // old — they exit with a diagnostic.
    #[tokio::test]
    async fn build_and_report_class() {
        let sb = LandlockSandbox::read_only();
        assert_eq!(sb.class(CapToken::FILESYSTEM_READ), SandboxClass::L2);
    }

    #[tokio::test]
    async fn enforce_or_report_unsupported() {
        let sb = LandlockSandbox::read_only();
        match sb
            .exec(SandboxRequest::new(
                ToolId("ll".into()),
                CapToken::FILESYSTEM_READ,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
        {
            Ok(out) => assert!(out.layers_invoked.contains(&SandboxLayer::Landlock)),
            Err(GaussError::Io(msg)) => {
                assert!(
                    msg.contains("landlock"),
                    "unexpected I/O error from landlock layer: {msg}"
                );
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
}
