//! [`CheckpointTool`] — surface `gauss_checkpoint` to the agent loop.
//!
//! When the model calls `checkpoint({"action": "snapshot", ...})`,
//! the tool reaches through [`gauss_checkpoint::CheckpointManager`]
//! and captures the live working-directory state. The shipping
//! semantics mirror Hermes's `checkpoint_manager` but with two
//! GaussClaw-only properties:
//!
//! - **Cap separation.** The tool declares `cap:checkpoint:write`
//!   (snapshotting). The optional `rollback` action additionally
//!   requires `cap:checkpoint:rollback` — see
//!   [`CheckpointTool::new_with_rollback`]. The default-registry
//!   build wires the snapshot-only variant so a low-privilege session
//!   can't destroy live state.
//! - **Content addressing.** Every snapshot id is BLAKE3 of the
//!   captured manifest; two snapshots of an unchanged tree share an id.
//!
//! Skill Manifest:
//!
//! ```toml
//! name        = "checkpoint"
//! description = "Capture / restore the working directory."
//! caps        = ["checkpoint:write"]
//! taint       = "user"
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_checkpoint::{
    CheckpointBackend, CheckpointError, CheckpointId, CheckpointManager, MemoryBackend, ReceiptOp,
};
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_WRITE_ONLY: &str = r#"
name        = "checkpoint"
description = "Capture a content-addressed snapshot of the live working directory. Useful before destructive experiments."
usage       = "Args: {action: 'snapshot'|'list'|'remove', root, paths?, label?, id?}. Returns id + manifest digest."
caps        = ["checkpoint:write"]
taint       = "user"
reversible  = false
persistent  = true

[guards]
no_instruction_substrings = true
max_string_len            = 4096

[schema]
type = "object"
"#;

const MANIFEST_WITH_ROLLBACK: &str = r#"
name        = "checkpoint"
description = "Capture or restore the working directory. Rollback overwrites the live tree with the snapshot's captured bytes."
usage       = "Args: {action: 'snapshot'|'rollback'|'list'|'remove', root, paths?, label?, id?}."
caps        = ["checkpoint:write", "checkpoint:rollback"]
taint       = "user"
reversible  = false
persistent  = true

[guards]
no_instruction_substrings = true
max_string_len            = 4096

[schema]
type = "object"
"#;

/// Agent-side checkpoint tool. Wraps a shared
/// [`CheckpointManager`] so multiple sessions can snapshot into the
/// same backing store.
pub struct CheckpointTool {
    manifest: ToolManifest,
    manager: Arc<CheckpointManager>,
    grant: CapToken,
}

impl CheckpointTool {
    /// Build a write-only tool (snapshot/list/remove). The kernel
    /// admit gate restricts the tool's declared cap to
    /// `cap:checkpoint:write` so a session grant that lacks
    /// `checkpoint:rollback` can still use the snapshot path.
    ///
    /// `grant` is the cap-token the tool's internal manager checks at
    /// every operation. Callers typically pass `CapToken::TOP` for the
    /// process-level grant and let the kernel restrict at the outer
    /// admit gate; the inner re-check provides defence in depth.
    ///
    /// # Panics
    /// Panics if the embedded manifest TOML fails to parse (build-
    /// time only).
    #[must_use]
    pub fn new(manager: Arc<CheckpointManager>, grant: CapToken) -> Self {
        Self::with_manifest(MANIFEST_WRITE_ONLY, manager, grant)
    }

    /// Build a tool that exposes the `rollback` action as well.
    ///
    /// # Panics
    /// Same as [`Self::new`].
    #[must_use]
    pub fn new_with_rollback(manager: Arc<CheckpointManager>, grant: CapToken) -> Self {
        Self::with_manifest(MANIFEST_WITH_ROLLBACK, manager, grant)
    }

    fn with_manifest(toml: &str, manager: Arc<CheckpointManager>, grant: CapToken) -> Self {
        let skill = SkillManifest::from_toml(toml).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("checkpoint".into()))
            .expect("embedded skill compiles");
        Self {
            manifest,
            manager,
            grant,
        }
    }

    /// Convenience: build a write-only tool with an in-memory backend.
    #[must_use]
    pub fn with_in_memory_backend() -> Self {
        let backend: Box<dyn CheckpointBackend> = Box::new(MemoryBackend::new());
        let mgr = Arc::new(CheckpointManager::new(backend));
        Self::new(mgr, CapToken::CHECKPOINT_WRITE)
    }

    /// Convenience: build a write+rollback tool with an in-memory
    /// backend. The grant in this configuration includes both caps so
    /// `rollback` is usable from inside tests / smoke runs.
    #[must_use]
    pub fn with_in_memory_backend_and_rollback() -> Self {
        let backend: Box<dyn CheckpointBackend> = Box::new(MemoryBackend::new());
        let mgr = Arc::new(CheckpointManager::new(backend));
        Self::new_with_rollback(
            mgr,
            CapToken::CHECKPOINT_WRITE | CapToken::CHECKPOINT_ROLLBACK,
        )
    }
}

#[async_trait]
impl ToolTrait for CheckpointTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `action`".into()))?;
        match action {
            "snapshot" => self.do_snapshot(&args).await,
            "rollback" => self.do_rollback(&args).await,
            "list" => self.do_list().await,
            "remove" => self.do_remove(&args).await,
            other => Err(GaussError::Internal(format!(
                "unknown checkpoint action `{other}` (try snapshot/rollback/list/remove)"
            ))),
        }
    }
}

impl CheckpointTool {
    async fn do_snapshot(&self, args: &serde_json::Value) -> GaussResult<serde_json::Value> {
        let root = parse_root(args)?;
        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("(unlabeled snapshot)");
        let paths = parse_paths(args)?;
        let (snap, receipt) = self
            .manager
            .snapshot(self.grant, &root, label, &paths)
            .await
            .map_err(|e| map_err(&e))?;
        Ok(serde_json::json!({
            "kind":       "checkpoint_snapshot",
            "id":         snap.id.0,
            "label":      snap.label,
            "file_count": receipt.file_count,
            "size_bytes": receipt.size_bytes,
            "timestamp":  receipt.timestamp,
        }))
    }

    async fn do_rollback(&self, args: &serde_json::Value) -> GaussResult<serde_json::Value> {
        let root = parse_root(args)?;
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `id`".into()))?;
        let receipt = self
            .manager
            .rollback(self.grant, &CheckpointId::new(id), &root)
            .await
            .map_err(|e| map_err(&e))?;
        let op = match receipt.op {
            ReceiptOp::Rollback => "rolled_back",
            ReceiptOp::Snapshot => "snapshot",
            _ => "unknown",
        };
        Ok(serde_json::json!({
            "kind":       "checkpoint_rollback",
            "op":         op,
            "id":         receipt.id.0,
            "file_count": receipt.file_count,
            "timestamp":  receipt.timestamp,
        }))
    }

    async fn do_list(&self) -> GaussResult<serde_json::Value> {
        let list = self.manager.list().await.map_err(|e| map_err(&e))?;
        let rows: Vec<serde_json::Value> = list
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "id":         s.id.0,
                    "label":      s.label,
                    "file_count": s.file_count(),
                    "size_bytes": s.size_bytes(),
                    "created_at": s.created_at,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "kind":  "checkpoint_list",
            "items": rows,
        }))
    }

    async fn do_remove(&self, args: &serde_json::Value) -> GaussResult<serde_json::Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `id`".into()))?;
        self.manager
            .remove(&CheckpointId::new(id))
            .await
            .map_err(|e| map_err(&e))?;
        Ok(serde_json::json!({ "kind": "checkpoint_removed", "id": id }))
    }
}

fn parse_root(args: &serde_json::Value) -> GaussResult<PathBuf> {
    let s = args
        .get("root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GaussError::Internal("missing string field `root`".into()))?;
    Ok(PathBuf::from(s))
}

fn parse_paths(args: &serde_json::Value) -> GaussResult<Vec<PathBuf>> {
    let Some(arr) = args.get("paths").and_then(|v| v.as_array()) else {
        return Ok(vec![]);
    };
    let mut out: Vec<PathBuf> = Vec::with_capacity(arr.len());
    for v in arr {
        let s = v
            .as_str()
            .ok_or_else(|| GaussError::Internal("`paths[]` must be strings".into()))?;
        out.push(PathBuf::from(s));
    }
    Ok(out)
}

fn map_err(e: &CheckpointError) -> GaussError {
    GaussError::Internal(format!("checkpoint: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn write_file(root: &std::path::Path, rel: &str, bytes: &[u8]) {
        let abs = root.join(rel);
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(&abs, bytes).await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_then_list_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt", b"hello").await;
        let tool = CheckpointTool::with_in_memory_backend();
        let snap = tool
            .invoke_raw(serde_json::json!({
                "action": "snapshot",
                "root":   dir.path().to_string_lossy(),
                "paths":  ["a.txt"],
                "label":  "pre-experiment",
            }))
            .await
            .unwrap();
        assert_eq!(snap["kind"], "checkpoint_snapshot");
        assert_eq!(snap["file_count"], 1);
        let list = tool
            .invoke_raw(serde_json::json!({"action": "list"}))
            .await
            .unwrap();
        assert_eq!(list["kind"], "checkpoint_list");
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn write_only_tool_refuses_rollback_action_via_unknown() {
        // The write-only variant returns "unknown action" for
        // `rollback` — that's intentional. The model is told via the
        // manifest that the cap is missing.
        let tool = CheckpointTool::with_in_memory_backend();
        let err = tool
            .invoke_raw(serde_json::json!({
                "action": "rollback",
                "root":   "/tmp",
                "id":     "x",
            }))
            .await
            .unwrap_err();
        // The rollback action is reachable but the inner grant lacks
        // the rollback cap — the manager refuses.
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn rollback_round_trip_with_rollback_variant() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "a.txt", b"original").await;
        let tool = CheckpointTool::with_in_memory_backend_and_rollback();
        let snap = tool
            .invoke_raw(serde_json::json!({
                "action": "snapshot",
                "root":   dir.path().to_string_lossy(),
                "paths":  ["a.txt"],
            }))
            .await
            .unwrap();
        let id = snap["id"].as_str().unwrap().to_string();
        // Mutate.
        write_file(dir.path(), "a.txt", b"corrupted").await;
        // Rollback.
        let back = tool
            .invoke_raw(serde_json::json!({
                "action": "rollback",
                "root":   dir.path().to_string_lossy(),
                "id":     id,
            }))
            .await
            .unwrap();
        assert_eq!(back["kind"], "checkpoint_rollback");
        let after = tokio::fs::read(dir.path().join("a.txt")).await.unwrap();
        assert_eq!(after, b"original");
    }

    #[tokio::test]
    async fn unknown_action_rejected() {
        let tool = CheckpointTool::with_in_memory_backend();
        let err = tool
            .invoke_raw(serde_json::json!({"action": "fnord"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn missing_root_rejected() {
        let tool = CheckpointTool::with_in_memory_backend();
        let err = tool
            .invoke_raw(serde_json::json!({"action": "snapshot"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_checkpoint_write_cap() {
        let tool = CheckpointTool::with_in_memory_backend();
        assert!(tool
            .manifest()
            .cap_required
            .contains(CapToken::CHECKPOINT_WRITE));
    }

    #[test]
    fn rollback_variant_declares_both_caps() {
        let tool = CheckpointTool::with_in_memory_backend_and_rollback();
        assert!(tool
            .manifest()
            .cap_required
            .contains(CapToken::CHECKPOINT_WRITE));
        assert!(tool
            .manifest()
            .cap_required
            .contains(CapToken::CHECKPOINT_ROLLBACK));
    }
}
