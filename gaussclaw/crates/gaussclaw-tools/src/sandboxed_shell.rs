//! [`SandboxedShellTool`] — `shell_sandboxed` tool routed through the
//! `gauss-sandbox` bwrap L3a layer.
//!
//! Sprint 12 of "Wire the Loop". The existing [`crate::ShellTool`]
//! runs subprocesses with `tokio::process::Command` directly, which
//! is fine for trusted operators (it carries the
//! `subprocess:spawn` cap so the declass map keeps it off web /
//! adversarial taint) but bypasses every sandbox layer the engine
//! ships. This tool closes the loop:
//!
//! 1. Takes `argv: [str]` + optional `bind_ro: [path]` from JSON.
//! 2. Builds a [`gauss_traits::SandboxRequest`] with the argv and
//!    bind list.
//! 3. Hands it to a caller-supplied [`gauss_sandbox::BwrapSandbox`]
//!    via the [`gauss_traits::SandboxTrait`] trait object.
//! 4. The Sprint-9 bwrap layer constructs the deny-by-default
//!    profile (fresh user / PID / IPC / UTS / cgroup / **net**
//!    namespaces, `--clearenv`, nobody:nogroup, tmpfs root, fresh
//!    /proc and /dev, read-only `/usr` + `/lib` + `/bin`) and
//!    `Command::spawn`s the program inside the confinement.
//! 5. Stdout + exit code are returned in the same JSON shape
//!    [`crate::ShellTool`] uses, so callers can swap one for the
//!    other.
//!
//! Carries the same cap (`subprocess:spawn`) and irreversibility
//! flag as [`crate::ShellTool`]: the sandbox lowers the blast radius
//! of an *unintended* shell call but doesn't change the trust
//! contract a session needs to ask for one.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{SandboxRequest, SandboxTrait, ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "shell_sandboxed"
description = "Run an argv-style command inside a deny-by-default bwrap sandbox (no network, no shell, nobody:nogroup, read-only host). Returns {exit_code, stdout}."
usage       = "Use when the user explicitly asks to run a system command and isolation is required. Args: {argv: [str], bind_ro?: [str]}."
caps        = ["subprocess:spawn"]
taint       = "user"
reversible  = false
persistent  = false

[cost]
tokens_per_call  = 500
wallclock_ms     = 1500
dollars_per_call = 0.0

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

const MAX_CAPTURE: usize = 65_536;

/// Sandboxed shell tool. Requires `subprocess:spawn`. **Not reversible.**
pub struct SandboxedShellTool {
    manifest: ToolManifest,
    sandbox: Arc<dyn SandboxTrait>,
}

impl SandboxedShellTool {
    /// Build the tool with the supplied sandbox layer.
    ///
    /// The expected caller is the bin: `bwrap_sandbox` is constructed
    /// once at startup (`Arc<gauss_sandbox::BwrapSandbox>`) and shared
    /// across every `shell_sandboxed` invocation. The trait object
    /// keeps this module decoupled from the concrete L3 layer so a
    /// future macOS-Seatbelt / FreeBSD-Capsicum backend slots in
    /// without touching the tool.
    ///
    /// # Panics
    /// Build-time only on embedded manifest parse failure.
    #[must_use]
    pub fn new(sandbox: Arc<dyn SandboxTrait>) -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("shell_sandboxed".into()))
            .expect("embedded skill compiles");
        Self { manifest, sandbox }
    }
}

#[async_trait]
impl ToolTrait for SandboxedShellTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let argv_value = args
            .get("argv")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| GaussError::Internal("missing array field `argv`".into()))?;
        if argv_value.is_empty() {
            return Err(GaussError::Internal("argv must be non-empty".into()));
        }
        let mut argv: Vec<String> = Vec::with_capacity(argv_value.len());
        for a in argv_value {
            let s = a
                .as_str()
                .ok_or_else(|| GaussError::Internal("argv entries must be strings".into()))?;
            argv.push(s.to_string());
        }
        // Optional bind_ro list — entries that aren't strings or aren't
        // absolute paths are dropped; the bwrap layer would reject
        // relative paths anyway, and the tool surface should be
        // forgiving rather than fail-on-first-bad-entry.
        let bind_ro: Vec<String> = args
            .get("bind_ro")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| s.starts_with('/'))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let req = SandboxRequest::new(
            ToolId("shell_sandboxed".into()),
            gauss_core::CapToken::SUBPROCESS_SPAWN,
            args.clone(),
            Vec::new(),
        )
        .with_argv(argv)
        .with_bind_ro(bind_ro);

        let outcome = self.sandbox.exec(req).await?;

        let mut stdout = String::from_utf8_lossy(&outcome.stdout).into_owned();
        let stdout_truncated = stdout.len() > MAX_CAPTURE;
        if stdout_truncated {
            stdout.truncate(MAX_CAPTURE);
        }
        Ok(serde_json::json!({
            "exit_code": outcome.exit_code,
            "stdout": stdout,
            "stdout_truncated": stdout_truncated,
            "layers_invoked": outcome
                .layers_invoked
                .iter()
                .map(|l| format!("{l:?}"))
                .collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::CapToken;
    use gauss_traits::{SandboxClass, SandboxLayer, SandboxOutcome};

    /// Test sandbox that records every request it sees and returns a
    /// canned outcome. Used by the unit tests to verify the tool
    /// constructs the SandboxRequest correctly without depending on
    /// a real `bwrap` binary.
    #[derive(Debug)]
    struct RecordingSandbox {
        seen: std::sync::Mutex<Vec<SandboxRequest>>,
        canned_stdout: Vec<u8>,
        canned_exit: i32,
    }

    impl RecordingSandbox {
        fn new(canned_stdout: &[u8], canned_exit: i32) -> Self {
            Self {
                seen: std::sync::Mutex::new(Vec::new()),
                canned_stdout: canned_stdout.to_vec(),
                canned_exit,
            }
        }
        fn snapshot(&self) -> Vec<SandboxRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SandboxTrait for RecordingSandbox {
        fn class(&self, _cap: CapToken) -> SandboxClass {
            SandboxClass::L3
        }
        async fn exec(&self, request: SandboxRequest) -> GaussResult<SandboxOutcome> {
            self.seen.lock().unwrap().push(request);
            Ok(SandboxOutcome::new(
                self.canned_stdout.clone(),
                vec![SandboxLayer::Namespace],
                self.canned_exit,
            ))
        }
    }

    fn tool_with_recorder() -> (SandboxedShellTool, Arc<RecordingSandbox>) {
        let rec = Arc::new(RecordingSandbox::new(b"hello\n", 0));
        let tool = SandboxedShellTool::new(rec.clone());
        (tool, rec)
    }

    #[tokio::test]
    async fn dispatches_argv_through_sandbox() {
        let (tool, rec) = tool_with_recorder();
        let out = tool
            .invoke_raw(serde_json::json!({
                "argv": ["/bin/echo", "hello"]
            }))
            .await
            .unwrap();
        assert_eq!(out["exit_code"], 0);
        assert_eq!(out["stdout"].as_str().unwrap().trim_end(), "hello");
        let seen = rec.snapshot();
        assert_eq!(seen.len(), 1);
        let req = &seen[0];
        assert_eq!(req.argv.as_deref(), Some(&["/bin/echo".to_string(), "hello".to_string()][..]));
        // No bind_ro requested in args → empty list.
        assert!(req.bind_ro.is_empty());
        // Tool id is the canonical one.
        assert_eq!(req.tool.0, "shell_sandboxed");
        // Layers invoked surfaces in the result JSON.
        let layers = out["layers_invoked"].as_array().unwrap();
        assert!(layers.iter().any(|l| l == "Namespace"));
    }

    #[tokio::test]
    async fn bind_ro_paths_are_forwarded_to_sandbox() {
        let (tool, rec) = tool_with_recorder();
        tool.invoke_raw(serde_json::json!({
            "argv": ["/bin/cat", "/data/file"],
            "bind_ro": ["/data", "relative/skip"]
        }))
        .await
        .unwrap();
        let req = &rec.snapshot()[0];
        assert_eq!(req.bind_ro, vec!["/data".to_string()]);
    }

    #[tokio::test]
    async fn missing_argv_is_rejected() {
        let (tool, _rec) = tool_with_recorder();
        let err = tool
            .invoke_raw(serde_json::json!({}))
            .await
            .expect_err("no argv");
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn empty_argv_is_rejected() {
        let (tool, _rec) = tool_with_recorder();
        let err = tool
            .invoke_raw(serde_json::json!({ "argv": [] }))
            .await
            .expect_err("empty argv");
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn non_string_argv_entry_is_rejected() {
        let (tool, _rec) = tool_with_recorder();
        let err = tool
            .invoke_raw(serde_json::json!({ "argv": ["/bin/x", 42] }))
            .await
            .expect_err("non-string");
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn non_zero_exit_code_propagates_to_caller() {
        let rec = Arc::new(RecordingSandbox::new(b"", 127));
        let tool = SandboxedShellTool::new(rec);
        let out = tool
            .invoke_raw(serde_json::json!({ "argv": ["/bin/false"] }))
            .await
            .unwrap();
        assert_eq!(out["exit_code"], 127);
    }

    #[test]
    fn manifest_declares_subprocess_cap_and_irreversible() {
        let rec = Arc::new(RecordingSandbox::new(b"", 0));
        let tool = SandboxedShellTool::new(rec);
        assert_eq!(
            tool.manifest().cap_required.bits(),
            CapToken::SUBPROCESS_SPAWN.bits()
        );
        assert!(!tool.manifest().reversible);
    }

    /// End-to-end with the real `BwrapSandbox` — skips silently when
    /// `bwrap` isn't on PATH (CI / macOS). This is the "the seam
    /// between Sprint 9 and Sprint 12 actually works" test.
    #[tokio::test]
    async fn end_to_end_against_real_bwrap_when_available() {
        if std::process::Command::new("bwrap")
            .arg("--version")
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            eprintln!("bwrap not available — skipping end-to-end test");
            return;
        }
        let sandbox: Arc<dyn SandboxTrait> = Arc::new(gauss_sandbox::bwrap_layer::BwrapSandbox::default());
        let tool = SandboxedShellTool::new(sandbox);
        let out = tool
            .invoke_raw(serde_json::json!({
                "argv": ["/bin/echo", "sandboxed hello"]
            }))
            .await
            .expect("e2e ok");
        assert_eq!(out["exit_code"], 0);
        assert_eq!(out["stdout"].as_str().unwrap().trim_end(), "sandboxed hello");
    }
}
