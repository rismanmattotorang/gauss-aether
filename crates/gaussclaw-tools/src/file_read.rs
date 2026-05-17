//! [`FileReadTool`] — read a UTF-8 file. Requires `fs:read` capability.
//!
//! ## Hermes-superior contract
//!
//! Hermes upstream's `file_read` runs as a Python function in the
//! agent's main process: any path the OS lets the process read is
//! readable, no taint awareness, raw bytes flow into the next prompt.
//!
//! `FileReadTool`:
//!
//! - Refuses to dispatch unless the kernel grants `FILESYSTEM_READ`
//!   under the session's current taint floor.
//! - Returns a typed `{path, content, bytes_read}` shape, schema-gated.
//! - Output taint defaults to `User` (declared by the manifest) so a
//!   filesystem read can't be silently upgraded to `Trusted`.
//! - When wrapped by [`gauss_hwca::WorkerSpawner::spawn_and_invoke`],
//!   the raw read content stays inside the worker; only the schema-
//!   validated value survives the worker drop.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "file_read"
description = "Read a UTF-8 file from disk. Returns {path, content, bytes_read}."
usage       = "Use when the user asks to read a file or include its contents."
caps        = ["fs:read"]
taint       = "user"
reversible  = true
persistent  = false

[cost]
tokens_per_call  = 200
wallclock_ms     = 10
dollars_per_call = 0.0

[guards]
no_instruction_substrings = true
max_string_len            = 1048576   # 1 MiB

[schema]
type = "object"
"#;

/// Filesystem-read tool. Requires `fs:read`.
pub struct FileReadTool {
    manifest: ToolManifest,
}

impl FileReadTool {
    /// Build a new `FileReadTool`.
    ///
    /// # Panics
    /// Build-time only on embedded manifest parse failure.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("file_read".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for FileReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for FileReadTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `path`".into()))?;
        // The kernel admit gate ALREADY guarded this call. The
        // path-scoping policy (e.g. "only under ./data") lands with
        // Phase 3 slice 4's per-capability scope binding; until then
        // `fs:read` grants global filesystem read.
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| GaussError::Internal(format!("read {path}: {e}")))?;
        let bytes_read = content.len();
        Ok(serde_json::json!({
            "path": path,
            "content": content,
            "bytes_read": bytes_read,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_a_real_file() {
        let path = std::env::temp_dir().join("gaussclaw_file_read_test.txt");
        tokio::fs::write(&path, "hello from gaussclaw").await.unwrap();
        let t = FileReadTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "path": path.to_string_lossy() }))
            .await
            .unwrap();
        assert_eq!(out["content"], "hello from gaussclaw");
        assert_eq!(out["bytes_read"], 20);
        tokio::fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn missing_file_is_tool_error() {
        let t = FileReadTool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "path": "/nope/definitely/missing/file" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_fs_read_cap() {
        let t = FileReadTool::new();
        assert_eq!(
            t.manifest().cap_required.bits(),
            gauss_core::CapToken::FILESYSTEM_READ.bits()
        );
    }
}
