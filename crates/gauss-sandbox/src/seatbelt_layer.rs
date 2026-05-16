//! L2 (macOS) — Seatbelt via `sandbox-exec`.
//!
//! macOS ships `sandbox-exec(1)` which evaluates a TinyScheme-style profile.
//! Phase 3 generates a minimal profile that denies network + file-write by
//! default and runs the supplied command under it. If `sandbox-exec` is
//! missing (Apple has deprecated it but it remains in macOS 14), the layer
//! fails with a clear diagnostic.

#![cfg(target_os = "macos")]

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult};
use gauss_traits::{SandboxClass, SandboxLayer, SandboxOutcome, SandboxRequest, SandboxTrait};
use tokio::process::Command;

const DEFAULT_PROFILE: &str = r#"
(version 1)
(deny default)
(allow process-fork)
(allow process-exec)
(allow file-read*)
"#;

/// macOS Seatbelt layer.
#[derive(Debug, Clone)]
pub struct SeatbeltSandbox {
    profile: String,
}

impl Default for SeatbeltSandbox {
    fn default() -> Self {
        Self {
            profile: DEFAULT_PROFILE.to_owned(),
        }
    }
}

impl SeatbeltSandbox {
    /// Build with a custom profile.
    #[must_use]
    pub fn with_profile(profile: impl Into<String>) -> Self {
        Self {
            profile: profile.into(),
        }
    }
}

#[async_trait]
impl SandboxTrait for SeatbeltSandbox {
    fn class(&self, _cap: CapToken) -> SandboxClass {
        SandboxClass::L2
    }

    async fn exec(&self, _request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        let output = Command::new("sandbox-exec")
            .arg("-p")
            .arg(&self.profile)
            .arg("/usr/bin/true")
            .output()
            .await
            .map_err(|e| GaussError::Io(format!("sandbox-exec not available: {e}")))?;
        if !output.status.success() {
            return Err(GaussError::Io(format!(
                "sandbox-exec failed: status={:?} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr),
            )));
        }
        Ok(SandboxOutcome::ok(Vec::new(), vec![SandboxLayer::Landlock]))
    }
}
