//! [`FileWriteTool`] — write a UTF-8 file. Requires `fs:write` cap.
//!
//! ## Hermes-superior contract
//!
//! Hermes upstream's `file_write` runs as a Python function with the
//! agent's full filesystem privileges. GaussClaw refuses the dispatch
//! unless the kernel grants `FILESYSTEM_WRITE` under the call's taint
//! floor — and the default declassification map refuses `fs:write`
//! under `Web` and `Adversarial` taint (paper §VII.B), so a tool
//! output that traversed `web` cannot subsequently write to disk.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "file_write"
description = "Write a UTF-8 file to disk. Returns {path, bytes_written}."
usage       = "Use when the user explicitly asks to save output to a file."
caps        = ["fs:write"]
taint       = "user"
reversible  = false
persistent  = false

[cost]
tokens_per_call  = 200
wallclock_ms     = 10
dollars_per_call = 0.0

[guards]
no_instruction_substrings = true
max_string_len            = 1048576

[schema]
type = "object"
"#;

/// Filesystem-write tool. Requires `fs:write`.
///
/// **Not reversible** — the manifest's `reversible = false` triggers
/// the SAG approval-plane gate (Phase 7) whenever the autonomy rule
/// classifies a turn as `human-supervised`.
pub struct FileWriteTool {
    manifest: ToolManifest,
}

impl FileWriteTool {
    /// Build a new `FileWriteTool`.
    ///
    /// # Panics
    /// Build-time only on embedded manifest parse failure.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("file_write".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for FileWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for FileWriteTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `path`".into()))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GaussError::Internal("missing string field `content`".into())
            })?;
        // Path-traversal defence: reject `..` segments. The kernel
        // admit already gated FILESYSTEM_WRITE; this is a second layer
        // against a tool that's authorised in general but should still
        // not escape its working directory.
        if path.contains("..") {
            return Err(GaussError::Internal(
                "path traversal (`..`) refused".into(),
            ));
        }
        tokio::fs::write(path, content)
            .await
            .map_err(|e| GaussError::Internal(format!("write {path}: {e}")))?;
        let bytes_written = content.len();
        Ok(serde_json::json!({
            "path": path,
            "bytes_written": bytes_written,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_a_real_file() {
        let path = std::env::temp_dir().join("gaussclaw_file_write_test.txt");
        let t = FileWriteTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "path": path.to_string_lossy(),
                "content": "hello write",
            }))
            .await
            .unwrap();
        assert_eq!(out["bytes_written"], 11);
        let back = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(back, "hello write");
        tokio::fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn path_traversal_is_rejected() {
        let t = FileWriteTool::new();
        let err = t
            .invoke_raw(serde_json::json!({
                "path": "../../etc/passwd",
                "content": "x",
            }))
            .await
            .unwrap_err();
        match err {
            GaussError::Internal(msg) => assert!(msg.contains("traversal")),
            _ => panic!("expected Internal/traversal"),
        }
    }

    #[test]
    fn manifest_declares_fs_write_cap_and_irreversible() {
        let t = FileWriteTool::new();
        assert_eq!(
            t.manifest().cap_required.bits(),
            gauss_core::CapToken::FILESYSTEM_WRITE.bits()
        );
        assert!(!t.manifest().reversible);
    }
}
