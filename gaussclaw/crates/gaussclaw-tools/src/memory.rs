//! `memory_read` + `memory_write` tools (Sprint 7 §4).
//!
//! Both tools wrap [`gauss_curator::CrossSessionStore`] — the Honcho-
//! parity per-peer memory map from Sprint 5 §5. The split is
//! cap-aligned:
//!
//! - `memory_read` declares `cap:memory:read`. Hermes's default
//!   declass map refuses this under `Adversarial` taint so an
//!   adversarial-tainted message can't query the operator's history.
//! - `memory_write` declares a new `cap:memory:write` (aliased to
//!   `MEMORY_READ` in the parser for now; a dedicated bit will mint
//!   in a future sprint when operators need to grant read without
//!   write).
//!
//! Hermes's equivalent (`memory_tool.py`) runs without either gate.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_curator::{CrossSessionStore, MemoryRecord, Namespace, PeerId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const READ_MANIFEST: &str = r#"
name        = "memory_read"
description = "Read a record from the cross-session memory map by (peer, namespace, key)."
usage       = "Args: {peer, namespace?, key}. Returns {value, created_at, last_touched_at}."
caps        = ["memory:read"]
taint       = "user"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

const WRITE_MANIFEST: &str = r#"
name        = "memory_write"
description = "Insert or replace a record in the cross-session memory map."
usage       = "Args: {peer, namespace?, key, value, ttl_seconds?}. Returns {kind: 'memory_written'}."
caps        = ["memory:write"]
taint       = "user"
reversible  = true
persistent  = true

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// `memory_read` tool.
pub struct MemoryReadTool {
    manifest: ToolManifest,
    store: Arc<dyn CrossSessionStore>,
}

impl MemoryReadTool {
    /// Build over a shared store.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(store: Arc<dyn CrossSessionStore>) -> Self {
        let skill = SkillManifest::from_toml(READ_MANIFEST).expect("toml");
        let manifest = skill
            .compile(ToolId("memory_read".into()))
            .expect("compile");
        Self { manifest, store }
    }
}

#[async_trait]
impl ToolTrait for MemoryReadTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let peer = args
            .get("peer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `peer`".into()))?;
        let namespace = args
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("scratch");
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `key`".into()))?;
        let now = now_unix();
        let record = self
            .store
            .get(&PeerId::new(peer), &Namespace::new(namespace), key, now)
            .await
            .map_err(|e| GaussError::Internal(format!("memory: {e}")))?;
        match record {
            Some(r) => Ok(serde_json::json!({
                "kind":            "memory_record",
                "peer":            r.peer.as_str(),
                "namespace":       r.namespace.as_str(),
                "key":             r.key,
                "value":           r.value,
                "created_at":      r.created_at,
                "last_touched_at": r.last_touched_at,
            })),
            None => Ok(serde_json::json!({
                "kind": "memory_record",
                "peer": peer,
                "namespace": namespace,
                "key":  key,
                "value": serde_json::Value::Null,
            })),
        }
    }
}

/// `memory_write` tool.
pub struct MemoryWriteTool {
    manifest: ToolManifest,
    store: Arc<dyn CrossSessionStore>,
}

impl MemoryWriteTool {
    /// Build over a shared store.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(store: Arc<dyn CrossSessionStore>) -> Self {
        let skill = SkillManifest::from_toml(WRITE_MANIFEST).expect("toml");
        let manifest = skill
            .compile(ToolId("memory_write".into()))
            .expect("compile");
        Self { manifest, store }
    }
}

#[async_trait]
impl ToolTrait for MemoryWriteTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let peer = args
            .get("peer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `peer`".into()))?;
        let namespace = args
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("scratch");
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `key`".into()))?;
        let value = args
            .get("value")
            .cloned()
            .ok_or_else(|| GaussError::Internal("missing field `value`".into()))?;
        let ttl_seconds = args.get("ttl_seconds").and_then(serde_json::Value::as_i64);
        let now = now_unix();
        let mut record = MemoryRecord::new(
            PeerId::new(peer),
            Namespace::new(namespace),
            key,
            value,
            now,
        );
        record.ttl_seconds = ttl_seconds;
        self.store
            .put(record)
            .await
            .map_err(|e| GaussError::Internal(format!("memory: {e}")))?;
        Ok(serde_json::json!({
            "kind":      "memory_written",
            "peer":      peer,
            "namespace": namespace,
            "key":       key,
        }))
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// `cap:memory:write` is parsed in `gaussclaw-skill::parse_cap`.
pub use gauss_core::CapToken as Cap;

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_curator::InMemoryStore;

    fn mk_store() -> Arc<dyn CrossSessionStore> {
        Arc::new(InMemoryStore::new())
    }

    #[tokio::test]
    async fn write_then_read_round_trips() {
        let store = mk_store();
        let w = MemoryWriteTool::new(store.clone());
        let r = MemoryReadTool::new(store);
        w.invoke_raw(serde_json::json!({
            "peer": "alice",
            "key":  "fav_color",
            "value": "blue",
        }))
        .await
        .unwrap();
        let out = r
            .invoke_raw(serde_json::json!({"peer": "alice", "key": "fav_color"}))
            .await
            .unwrap();
        assert_eq!(out["kind"], "memory_record");
        assert_eq!(out["value"], "blue");
    }

    #[tokio::test]
    async fn read_returns_null_for_unknown_key() {
        let r = MemoryReadTool::new(mk_store());
        let out = r
            .invoke_raw(serde_json::json!({"peer": "x", "key": "missing"}))
            .await
            .unwrap();
        assert_eq!(out["value"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn write_with_ttl_round_trips() {
        let store = mk_store();
        let w = MemoryWriteTool::new(store.clone());
        let r = MemoryReadTool::new(store);
        w.invoke_raw(serde_json::json!({
            "peer": "x", "key": "k", "value": 1, "ttl_seconds": 86400
        }))
        .await
        .unwrap();
        let out = r
            .invoke_raw(serde_json::json!({"peer": "x", "key": "k"}))
            .await
            .unwrap();
        assert_eq!(out["value"], 1);
    }

    #[tokio::test]
    async fn read_rejects_missing_required_fields() {
        let r = MemoryReadTool::new(mk_store());
        assert!(r
            .invoke_raw(serde_json::json!({"peer": "x"}))
            .await
            .is_err());
        assert!(r.invoke_raw(serde_json::json!({"key": "k"})).await.is_err());
    }

    #[tokio::test]
    async fn write_rejects_missing_value() {
        let w = MemoryWriteTool::new(mk_store());
        let err = w
            .invoke_raw(serde_json::json!({"peer": "x", "key": "k"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn namespace_defaults_to_scratch() {
        let store = mk_store();
        let w = MemoryWriteTool::new(store.clone());
        let r = MemoryReadTool::new(store.clone());
        w.invoke_raw(serde_json::json!({"peer": "x", "key": "k", "value": "v"}))
            .await
            .unwrap();
        // Read with explicit `scratch` namespace finds it.
        let out = r
            .invoke_raw(serde_json::json!({"peer": "x", "namespace": "scratch", "key": "k"}))
            .await
            .unwrap();
        assert_eq!(out["value"], "v");
    }
}
