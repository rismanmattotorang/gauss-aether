//! [`ShellTool`] — run a shell command. Requires `subprocess:spawn` cap.
//!
//! ## Hermes-superior contract
//!
//! Hermes upstream's `shell` runs `subprocess.Popen(cmd, shell=True)`
//! — full POSIX shell with the agent's process privileges. Any tool
//! output that reaches the shell argument list as a string is a
//! command-injection vector.
//!
//! GaussClaw `ShellTool` closes four attack surfaces:
//!
//! 1. **Cap-gated dispatch.** Requires `subprocess:spawn`. The default
//!    declass map refuses `subprocess:spawn` under `Web` and
//!    `Adversarial` taint (paper §VII.B), so a tool whose output
//!    traversed `web` cannot subsequently spawn a process.
//! 2. **No shell interpreter.** Arguments arrive as a typed list
//!    `argv: [str]`, dispatched via `tokio::process::Command::new(&argv[0])
//!    .args(&argv[1..])`. No `bash -c`, no shell metachar interpretation.
//! 3. **Output size cap.** stdout / stderr captured up to `max_string_len`
//!    from the manifest; truncated beyond.
//! 4. **Worker isolation.** Raw process output dies at worker drop; only
//!    the schema-validated `{exit_code, stdout, stderr}` shape survives.
//! 5. **Irreversible.** `reversible = false` triggers the SAG approval
//!    plane (Phase 7) when the autonomy rule classifies the turn as
//!    human-supervised.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use tokio::process::Command;

const MANIFEST_TOML: &str = r#"
name        = "shell"
description = "Run an argv-style command (no shell interpreter). Returns {exit_code, stdout, stderr}."
usage       = "Use when the user explicitly asks to run a system command. Args: {argv: [str]}."
caps        = ["subprocess:spawn"]
taint       = "user"
reversible  = false
persistent  = false

[cost]
tokens_per_call  = 500
wallclock_ms     = 1000
dollars_per_call = 0.0

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

const MAX_CAPTURE: usize = 65_536;

/// Subprocess-spawn tool. Requires `subprocess:spawn`. **Not reversible.**
pub struct ShellTool {
    manifest: ToolManifest,
}

impl ShellTool {
    /// Build a new `ShellTool`.
    ///
    /// # Panics
    /// Build-time only on embedded manifest parse failure.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("shell".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for ShellTool {
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
        let mut parts = Vec::with_capacity(argv_value.len());
        for a in argv_value {
            let s = a.as_str().ok_or_else(|| {
                GaussError::Internal("argv entries must be strings".into())
            })?;
            parts.push(s.to_string());
        }
        let program = &parts[0];
        let extra_args = &parts[1..];

        let output = Command::new(program)
            .args(extra_args)
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| GaussError::Internal(format!("spawn {program}: {e}")))?;

        let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout_truncated = stdout.len() > MAX_CAPTURE;
        let stderr_truncated = stderr.len() > MAX_CAPTURE;
        if stdout_truncated {
            stdout.truncate(MAX_CAPTURE);
        }
        if stderr_truncated {
            stderr.truncate(MAX_CAPTURE);
        }
        Ok(serde_json::json!({
            "exit_code": output.status.code().unwrap_or(-1),
            "stdout": stdout,
            "stderr": stderr,
            "stdout_truncated": stdout_truncated,
            "stderr_truncated": stderr_truncated,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echoes_via_shell_program() {
        // `/bin/echo` is in PATH on every test target (Linux + macOS).
        let t = ShellTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "argv": ["/bin/echo", "hello shell"] }))
            .await
            .unwrap();
        assert_eq!(out["exit_code"], 0);
        let stdout = out["stdout"].as_str().unwrap();
        assert!(stdout.contains("hello shell"));
    }

    #[tokio::test]
    async fn missing_argv_is_rejected() {
        let t = ShellTool::new();
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn empty_argv_is_rejected() {
        let t = ShellTool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "argv": [] }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn nonexistent_command_returns_error() {
        let t = ShellTool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "argv": ["/this/does/not/exist"] }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_subprocess_cap_and_irreversible() {
        let t = ShellTool::new();
        assert_eq!(
            t.manifest().cap_required.bits(),
            gauss_core::CapToken::SUBPROCESS_SPAWN.bits()
        );
        assert!(!t.manifest().reversible);
    }
}
