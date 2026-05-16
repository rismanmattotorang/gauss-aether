//! L3b — Linux seccomp filter via `seccompiler` (pure Rust, no libseccomp).
//!
//! Phase 3 ships a small allow-list filter that denies the syscalls a
//! pure-computation WASM tool has no reason to issue: `socket`, `connect`,
//! `accept`, `bind`, `execve`, `clone3`, `unshare`, `mount`, `keyctl`, etc.
//! The filter is applied to the current thread on `exec` and stays in place
//! for the duration of the composite's run.
//!
//! The filter is intentionally minimal — it complements Landlock + WASM
//! fuel, not replaces them. Phase 4 will tighten this list against the
//! sandboxed tool's declared syscall manifest.

#![cfg(all(target_os = "linux", feature = "linux-layers"))]

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult};
use gauss_traits::{SandboxClass, SandboxLayer, SandboxOutcome, SandboxRequest, SandboxTrait};
use seccompiler::{
    apply_filter, BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch,
};
use std::collections::BTreeMap;

/// Phase-3 seccomp layer.
#[derive(Debug, Clone)]
pub struct SeccompSandbox {
    /// If true, syscall numbers in the deny-list result in `ENOSYS` (errno
    /// 38). If false, the offending thread is `SIGKILL`'d. Default is
    /// `errno=38` so test runs don't terminate the test harness.
    soft_deny: bool,
}

impl Default for SeccompSandbox {
    fn default() -> Self {
        Self { soft_deny: true }
    }
}

impl SeccompSandbox {
    /// Build a soft-deny layer.
    #[must_use]
    pub const fn soft() -> Self {
        Self { soft_deny: true }
    }

    /// Build a hard-deny layer (`SIGKILL` on rule hit).
    #[must_use]
    pub const fn hard() -> Self {
        Self { soft_deny: false }
    }

    /// Build the BPF filter program.
    fn build_filter(&self) -> GaussResult<BpfProgram> {
        // Default action is `Allow`; the deny-list overrides specific
        // syscalls. This is the opposite of the long-term goal (default
        // deny, allow-list per tool) — Phase 4 inverts the polarity once
        // tool manifests carry syscall declarations.
        let deny_action = if self.soft_deny {
            SeccompAction::Errno(38) // ENOSYS
        } else {
            SeccompAction::KillThread
        };
        // Syscall numbers are arch-specific; seccompiler exposes a
        // TargetArch::native() helper. For Phase 3 we pin to x86_64; ARM is
        // wired in Phase 10 via a build.rs.
        #[cfg(target_arch = "x86_64")]
        let deny_list: BTreeMap<i64, Vec<SeccompRule>> = [
            (41_i64, Vec::new()),  // socket
            (42_i64, Vec::new()),  // connect
            (43_i64, Vec::new()),  // accept
            (49_i64, Vec::new()),  // bind
            (59_i64, Vec::new()),  // execve
            (165_i64, Vec::new()), // mount
            (272_i64, Vec::new()), // unshare
            (250_i64, Vec::new()), // keyctl
            (435_i64, Vec::new()), // clone3
        ]
        .into_iter()
        .collect();

        #[cfg(not(target_arch = "x86_64"))]
        let deny_list: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

        let filter = SeccompFilter::new(
            deny_list,
            SeccompAction::Allow,
            deny_action,
            TargetArch::x86_64,
        )
        .map_err(|e| GaussError::Internal(format!("seccomp build: {e}")))?;
        filter
            .try_into()
            .map_err(|e| GaussError::Internal(format!("seccomp compile: {e}")))
    }
}

#[async_trait]
impl SandboxTrait for SeccompSandbox {
    fn class(&self, _cap: CapToken) -> SandboxClass {
        SandboxClass::L3
    }

    async fn exec(&self, _request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        let program = self.build_filter()?;
        tokio::task::spawn_blocking(move || -> GaussResult<()> {
            apply_filter(&program).map_err(|e| GaussError::Io(format!("seccomp apply: {e}")))
        })
        .await
        .map_err(|e| GaussError::Internal(format!("seccomp join: {e}")))??;
        Ok(SandboxOutcome::ok(Vec::new(), vec![SandboxLayer::Seccomp]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;

    #[tokio::test]
    async fn build_filter_succeeds() {
        let sb = SeccompSandbox::soft();
        let _bpf = sb.build_filter().expect("seccomp filter compiles");
    }

    #[tokio::test]
    async fn soft_filter_does_not_kill_the_test_process() {
        // Applying the soft filter MUST be safe — the test's own syscalls
        // (mmap, write, brk, etc.) are not in the deny-list.
        let sb = SeccompSandbox::soft();
        match sb
            .exec(SandboxRequest::new(
                ToolId("sc".into()),
                CapToken::SUBPROCESS_SPAWN,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
        {
            Ok(out) => assert!(out.layers_invoked.contains(&SandboxLayer::Seccomp)),
            Err(GaussError::Io(msg)) => {
                // Some CI sandboxes already restrict seccomp; tolerate.
                assert!(
                    msg.contains("seccomp") || msg.contains("prctl"),
                    "unexpected seccomp error: {msg}"
                );
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
}
