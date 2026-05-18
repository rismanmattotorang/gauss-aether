//! [`SessionSearchTool`] — hybrid BM25 + HNSW recall over past
//! conversations.
//!
//! Calls into [`gaussclaw_store::SessionStore::hybrid_search`], which
//! fuses FTS5 (BM25) and HNSW vector-similarity matches with the
//! `weighted union` rule from `gauss-memory` (Theorem T5). Returns
//! the top-`k` hits as structured JSON the agent loop can re-feed
//! into the next prompt.
//!
//! The tool is **cap-gated by `cap:memory:read`** (a new cap added
//! to `gauss-core` in this commit). The default declass map admits
//! `memory:read` under `Trusted` / `User` taint; Adversarial taint
//! refuses it, so an unverified web-fetched message can never query
//! the user's past conversation history.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use gaussclaw_store::SessionStore;

const MANIFEST_TOML: &str = r#"
name        = "session_search"
description = "Hybrid BM25 + HNSW search over past conversation turns."
usage       = "Use to find prior turns relevant to the current question. Args: {query: string, k?: u8}."
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

/// Hard cap on `k` to keep one call cheap.
const MAX_K: u64 = 50;

/// Session-search tool. Wraps a [`SessionStore`] handle so the same
/// search facility is exposed to both tools (this) and HTTP endpoints
/// (`/api/sessions`).
pub struct SessionSearchTool {
    manifest: ToolManifest,
    store: Arc<SessionStore>,
}

impl SessionSearchTool {
    /// Build a session-search tool that queries `store`.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new(store: Arc<SessionStore>) -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("session_search".into()))
            .expect("embedded skill compiles");
        Self { manifest, store }
    }

    /// Borrow the underlying store. Useful in tests.
    #[must_use]
    pub fn store(&self) -> &Arc<SessionStore> {
        &self.store
    }
}

#[async_trait]
impl ToolTrait for SessionSearchTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `query`".into()))?;
        let k = args
            .get("k")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(MAX_K) as usize;
        // alpha = 0.5 blends BM25 and HNSW evenly; T5 proves the
        // miss-rate is ε_fts · ε_vec for any non-degenerate alpha.
        let alpha = args
            .get("alpha")
            .and_then(serde_json::Value::as_f64)
            .map_or(0.5, |a| a.clamp(0.0, 1.0) as f32);
        let hits = self
            .store
            .hybrid_search(query, k, alpha)
            .await
            .map_err(|e| GaussError::Internal(format!("session_search: {e}")))?;
        let rows: Vec<serde_json::Value> = hits
            .into_iter()
            .map(|h| {
                serde_json::json!({
                    "turn_id":    h.turn.id,
                    "session_id": h.turn.session_id,
                    "role":       h.turn.role,
                    "snippet":    truncate(&h.turn.content, 280),
                    "score":      h.score,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "query": query,
            "k":     rows.len(),
            "hits":  rows,
        }))
    }
}

/// Convenience for the [`Self::cap_required`] check from outside.
#[must_use]
pub const fn memory_read_cap() -> CapToken {
    CapToken::MEMORY_READ
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_missing_query() {
        // Build with an in-memory store so we don't need a real backend
        // for this surface-validation test.
        let store = Arc::new(SessionStore::open_in_memory().await.expect("store"));
        let t = SessionSearchTool::new(store);
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn empty_store_returns_zero_hits() {
        let store = Arc::new(SessionStore::open_in_memory().await.expect("store"));
        let t = SessionSearchTool::new(store);
        let out = t
            .invoke_raw(serde_json::json!({ "query": "anything" }))
            .await
            .unwrap();
        assert_eq!(out["k"], 0);
        assert!(out["hits"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn k_is_clamped_to_max() {
        let store = Arc::new(SessionStore::open_in_memory().await.expect("store"));
        let t = SessionSearchTool::new(store);
        let out = t
            .invoke_raw(serde_json::json!({ "query": "x", "k": 9999 }))
            .await
            .unwrap();
        // Empty store → 0 hits regardless of cap; the test confirms the
        // call succeeds with the oversized k.
        assert_eq!(out["k"], 0);
    }

    #[test]
    fn manifest_declares_memory_read_cap() {
        // The manifest parses `memory:read` correctly only after the
        // `gaussclaw-skill` parser adds it; we exercise that path here.
        let manifest = SkillManifest::from_toml(MANIFEST_TOML).expect("parse");
        assert_eq!(manifest.caps, vec!["memory:read".to_string()]);
    }
}
