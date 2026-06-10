//! `memory_md_read` / `memory_md_write` — `MEMORY.md` tools.
//!
//! Companions to the existing `memory_read`/`memory_write` pair in
//! [`super::memory`]. Where those wrap the in-memory
//! [`gauss_curator::CrossSessionStore`], this module targets the
//! durable on-disk [`gaussclaw_skill::MemoryFile`] (`MEMORY.md`) —
//! the cross-session scratchpad OpenHarness exposes to the agent for
//! self-curated learning.
//!
//! ## Design choices
//!
//! * **One file per tool instance.** Each tool is constructed with a
//!   `PathBuf` and operates on that file only. Operators wire one
//!   instance per memory file they want the agent to access (typically
//!   one global, sometimes per-namespace).
//!
//! * **Atomic writes.** Delegates to [`MemoryFile::save_to`] which
//!   already writes through a `.tmp` sibling and renames atomically.
//!
//! * **Bounded.** `MemoryFile::cap_bytes` is honoured; the response
//!   payload reports how many sections (if any) were evicted to fit.
//!
//! * **Same cap.** Both tools declare `memory:write` (via the
//!   `MEMORY_READ` bit until a dedicated cap mint). They run under
//!   the schema gate's strict guards by default.

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::{MemoryFile, SkillManifest};

const READ_MANIFEST: &str = r#"
name        = "memory_md_read"
description = "Read the agent's MEMORY.md scratchpad. Returns the rendered markdown body."
usage       = "Args: {section?: string}. If section is set, returns just that section's body."
caps        = ["memory:read"]
taint       = "user"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 262144

[schema]
type = "object"
"#;

const WRITE_MANIFEST: &str = r#"
name        = "memory_md_write"
description = "Curate the agent's MEMORY.md scratchpad. Upsert or remove one section."
usage       = "Args: {section: string, body?: string, mode: 'upsert' | 'remove'}. Returns {kind: 'memory_md_written', dropped: usize}."
caps        = ["memory:write"]
taint       = "user"
reversible  = false
persistent  = true

[guards]
no_instruction_substrings = true
max_string_len            = 262144

[schema]
type = "object"
"#;

/// `memory_md_read` — returns the rendered `MEMORY.md` body, or a
/// single section's body when `section` is supplied.
pub struct MemoryMdReadTool {
    manifest: ToolManifest,
    path: PathBuf,
}

impl MemoryMdReadTool {
    /// Build for the given file path.
    ///
    /// # Panics
    /// Only on a build-time bug in the embedded manifest.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let skill = SkillManifest::from_toml(READ_MANIFEST).expect("toml");
        let manifest = skill
            .compile(ToolId("memory_md_read".into()))
            .expect("compile");
        Self {
            manifest,
            path: path.into(),
        }
    }
}

#[async_trait]
impl ToolTrait for MemoryMdReadTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let mem = MemoryFile::load_or_default(&self.path)
            .map_err(|e| GaussError::Internal(format!("memory_md_read: {e}")))?;
        let section = args.get("section").and_then(|v| v.as_str());
        let body = if let Some(s) = section {
            match mem.section(s) {
                Some(sec) => sec.body.clone(),
                None => String::new(),
            }
        } else {
            mem.render()
        };
        Ok(serde_json::json!({
            "kind": "memory_md_read",
            "section": section,
            "body": body,
            "section_count": mem.len(),
        }))
    }
}

/// `memory_md_write` — upsert or remove one section.
///
/// The interior file load/save is serialised through a Mutex so two
/// concurrent calls don't race on the on-disk file. Atomicity of the
/// individual save is already guaranteed by `MemoryFile::save_to`.
pub struct MemoryMdWriteTool {
    manifest: ToolManifest,
    path: PathBuf,
    /// Cap propagated to every loaded `MemoryFile`. Defaults to
    /// `gaussclaw_skill::DEFAULT_MEMORY_CAP` (256 KiB).
    cap_bytes: usize,
    lock: Mutex<()>,
}

impl MemoryMdWriteTool {
    /// Build for the given file path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let skill = SkillManifest::from_toml(WRITE_MANIFEST).expect("toml");
        let manifest = skill
            .compile(ToolId("memory_md_write".into()))
            .expect("compile");
        Self {
            manifest,
            path: path.into(),
            cap_bytes: gaussclaw_skill::DEFAULT_MEMORY_CAP,
            lock: Mutex::new(()),
        }
    }

    /// Builder: tighten the per-file byte cap (oldest-section
    /// eviction fires when exceeded).
    #[must_use]
    pub fn with_cap_bytes(mut self, n: usize) -> Self {
        self.cap_bytes = n;
        self
    }
}

#[async_trait]
impl ToolTrait for MemoryMdWriteTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let section = args
            .get("section")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `section`".into()))?
            .to_owned();
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("upsert");
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        // Hold the cross-call lock for the load → mutate → save round
        // trip so two concurrent writes never interleave.
        let _g = self
            .lock
            .lock()
            .map_err(|e| GaussError::Internal(format!("memory_md_write lock poisoned: {e}")))?;
        let mut mem = MemoryFile::load_or_default(&self.path)
            .map_err(|e| GaussError::Internal(format!("memory_md_write load: {e}")))?
            .with_cap_bytes(self.cap_bytes);
        let action = match mode {
            "upsert" => {
                mem.upsert_section(section.clone(), body);
                "upsert"
            }
            "remove" => {
                let removed = mem.remove_section(&section);
                if !removed {
                    return Err(GaussError::Internal(format!(
                        "memory_md_write: section `{section}` not found for removal"
                    )));
                }
                "remove"
            }
            other => {
                return Err(GaussError::Internal(format!(
                    "memory_md_write: unknown mode `{other}` (expected 'upsert' or 'remove')"
                )));
            }
        };
        let dropped = mem
            .save_to(&self.path)
            .map_err(|e| GaussError::Internal(format!("memory_md_write save: {e}")))?;
        Ok(serde_json::json!({
            "kind": "memory_md_written",
            "section": section,
            "action": action,
            "dropped": dropped,
            "section_count": mem.len(),
        }))
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gc-memmd-tool-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[tokio::test]
    async fn write_then_read_round_trip() {
        let dir = tmpdir("rw");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone());
        let out = w
            .invoke_raw(serde_json::json!({
                "section": "User",
                "body": "Alice prefers concise replies.",
                "mode": "upsert",
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "memory_md_written");
        assert_eq!(out["action"], "upsert");
        assert_eq!(out["section_count"], 1);

        let r = MemoryMdReadTool::new(path.clone());
        let read = r
            .invoke_raw(serde_json::json!({ "section": "User" }))
            .await
            .unwrap();
        assert_eq!(read["kind"], "memory_md_read");
        assert!(read["body"].as_str().unwrap().contains("Alice"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_full_file_returns_rendered_body() {
        let dir = tmpdir("read-full");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone());
        for (s, b) in [("A", "one"), ("B", "two")] {
            w.invoke_raw(serde_json::json!({
                "section": s,
                "body": b,
                "mode": "upsert",
            }))
            .await
            .unwrap();
        }
        let r = MemoryMdReadTool::new(path.clone());
        let read = r.invoke_raw(serde_json::json!({})).await.unwrap();
        let body = read["body"].as_str().unwrap();
        assert!(body.contains("## A"));
        assert!(body.contains("## B"));
        assert_eq!(read["section_count"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn upsert_replaces_existing_section() {
        let dir = tmpdir("upsert-replace");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone());
        w.invoke_raw(serde_json::json!({ "section": "User", "body": "first", "mode": "upsert" }))
            .await
            .unwrap();
        w.invoke_raw(serde_json::json!({ "section": "User", "body": "second", "mode": "upsert" }))
            .await
            .unwrap();

        let r = MemoryMdReadTool::new(path.clone());
        let read = r
            .invoke_raw(serde_json::json!({ "section": "User" }))
            .await
            .unwrap();
        assert_eq!(read["body"], "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_section_works() {
        let dir = tmpdir("remove");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone());
        w.invoke_raw(serde_json::json!({ "section": "x", "body": "y", "mode": "upsert" }))
            .await
            .unwrap();
        let out = w
            .invoke_raw(serde_json::json!({ "section": "x", "mode": "remove" }))
            .await
            .unwrap();
        assert_eq!(out["action"], "remove");
        assert_eq!(out["section_count"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remove_missing_section_errors() {
        let dir = tmpdir("remove-missing");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone());
        let err = w
            .invoke_raw(serde_json::json!({ "section": "ghost", "mode": "remove" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unknown_mode_errors() {
        let dir = tmpdir("unknown-mode");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone());
        let err = w
            .invoke_raw(serde_json::json!({ "section": "x", "mode": "delete" }))
            .await
            .unwrap_err();
        match err {
            GaussError::Internal(m) => assert!(m.contains("unknown mode")),
            other => panic!("expected Internal, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_section_field_errors() {
        let dir = tmpdir("missing-section");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone());
        let err = w
            .invoke_raw(serde_json::json!({ "mode": "upsert", "body": "x" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cap_eviction_reports_dropped() {
        let dir = tmpdir("cap");
        let path = dir.join("MEMORY.md");
        let w = MemoryMdWriteTool::new(path.clone()).with_cap_bytes(60);
        // Write enough sections that the cap forces eviction.
        for s in ["A", "B", "C", "D"] {
            w.invoke_raw(serde_json::json!({
                "section": s,
                "body": "lorem ipsum dolor sit amet",
                "mode": "upsert",
            }))
            .await
            .unwrap();
        }
        // Last write should report some `dropped > 0`.
        let out = w
            .invoke_raw(serde_json::json!({
                "section": "E",
                "body": "final entry that triggers eviction",
                "mode": "upsert",
            }))
            .await
            .unwrap();
        assert!(out["dropped"].as_u64().unwrap_or(0) >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_missing_section_returns_empty_body() {
        let dir = tmpdir("read-missing");
        let path = dir.join("MEMORY.md");
        // Empty file.
        std::fs::File::create(&path).unwrap();
        let r = MemoryMdReadTool::new(path.clone());
        let out = r
            .invoke_raw(serde_json::json!({ "section": "Missing" }))
            .await
            .unwrap();
        assert_eq!(out["body"], "");
        assert_eq!(out["section_count"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifests_declare_memory_caps() {
        let r = MemoryMdReadTool::new("/tmp/x");
        let w = MemoryMdWriteTool::new("/tmp/x");
        assert_eq!(r.manifest().id, ToolId("memory_md_read".into()));
        assert_eq!(w.manifest().id, ToolId("memory_md_write".into()));
        // Cap bits must be non-zero (both use MEMORY_READ bit under
        // the cap-aliasing rule).
        assert!(r.manifest().cap_required.bits() > 0);
        assert!(w.manifest().cap_required.bits() > 0);
    }
}
