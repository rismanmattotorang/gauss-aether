//! `todo` tool (Sprint 7 §4).
//!
//! In-memory todo list keyed by `(peer, list_id)`. The tool exposes
//! `add` / `list` / `set_status` / `remove`. State lives in a
//! `Mutex<BTreeMap>` inside the tool instance — production
//! deployments wire it through the cross-session memory map for
//! persistence, but the wire shape is identical so the swap is a
//! constructor change.
//!
//! Hermes ships `todo_tool.py` with raw JSON-file persistence and
//! no cap declaration. GaussClaw declares `cap:todo:write` so a
//! session that loses the cap mid-run can't silently mutate.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use serde::{Deserialize, Serialize};

const MANIFEST_TOML: &str = r#"
name        = "todo"
description = "Manage a per-peer todo list. Actions: add, list, set_status, remove."
usage       = "Args: {action: 'add'|'list'|'set_status'|'remove', peer, list_id?, text?, id?, status?}."
caps        = ["todo:write"]
taint       = "user"
reversible  = true
persistent  = true

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// Todo item status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TodoStatus {
    /// Not yet started.
    Pending,
    /// In progress.
    InProgress,
    /// Completed.
    Done,
}

impl TodoStatus {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "in_progress" | "in-progress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            _ => None,
        }
    }
}

/// One todo item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    /// Stable monotonic id (per `(peer, list_id)`).
    pub id: u64,
    /// Item text.
    pub text: String,
    /// Lifecycle status.
    pub status: TodoStatus,
    /// UNIX seconds of creation.
    pub created_at: i64,
}

/// `todo` tool.
pub struct TodoTool {
    manifest: ToolManifest,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    lists: BTreeMap<(String, String), Vec<TodoItem>>,
    next_id: u64,
}

impl TodoTool {
    /// Build a fresh tool.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("toml");
        let manifest = skill.compile(ToolId("todo".into())).expect("compile");
        Self {
            manifest,
            inner: Mutex::new(Inner::default()),
        }
    }
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for TodoTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `action`".into()))?;
        let peer = args
            .get("peer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `peer`".into()))?;
        let list_id = args
            .get("list_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let key = (peer.to_string(), list_id.to_string());

        match action {
            "add" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| GaussError::Internal("missing string field `text`".into()))?;
                let mut g = self.inner.lock().expect("poisoned");
                g.next_id = g.next_id.saturating_add(1);
                let id = g.next_id;
                let now = now_unix();
                g.lists.entry(key).or_default().push(TodoItem {
                    id,
                    text: text.into(),
                    status: TodoStatus::Pending,
                    created_at: now,
                });
                Ok(serde_json::json!({"kind": "todo_added", "id": id}))
            }
            "list" => {
                let g = self.inner.lock().expect("poisoned");
                let items = g.lists.get(&key).cloned().unwrap_or_default();
                Ok(serde_json::json!({
                    "kind":  "todo_list",
                    "items": items,
                }))
            }
            "set_status" => {
                let id = args
                    .get("id")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| GaussError::Internal("missing uint field `id`".into()))?;
                let status_str = args
                    .get("status")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| GaussError::Internal("missing string field `status`".into()))?;
                let new_status = TodoStatus::parse(status_str).ok_or_else(|| {
                    GaussError::Internal(format!(
                        "unknown todo status {status_str:?} (try pending/in_progress/done)"
                    ))
                })?;
                let mut g = self.inner.lock().expect("poisoned");
                let list = g.lists.get_mut(&key).ok_or_else(|| {
                    GaussError::Internal(format!("no todo list for {peer}/{list_id}"))
                })?;
                let item = list.iter_mut().find(|i| i.id == id).ok_or_else(|| {
                    GaussError::Internal(format!("no todo item {id} in {peer}/{list_id}"))
                })?;
                item.status = new_status;
                Ok(serde_json::json!({"kind": "todo_status_set", "id": id}))
            }
            "remove" => {
                let id = args
                    .get("id")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| GaussError::Internal("missing uint field `id`".into()))?;
                let mut g = self.inner.lock().expect("poisoned");
                let list = g.lists.get_mut(&key).ok_or_else(|| {
                    GaussError::Internal(format!("no todo list for {peer}/{list_id}"))
                })?;
                let before = list.len();
                list.retain(|i| i.id != id);
                if list.len() == before {
                    return Err(GaussError::Internal(format!(
                        "no todo item {id} in {peer}/{list_id}"
                    )));
                }
                Ok(serde_json::json!({"kind": "todo_removed", "id": id}))
            }
            other => Err(GaussError::Internal(format!(
                "unknown todo action {other:?}"
            ))),
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_then_list_round_trip() {
        let t = TodoTool::new();
        let r = t
            .invoke_raw(serde_json::json!({
                "action": "add", "peer": "alice", "text": "ship it"
            }))
            .await
            .unwrap();
        let id = r["id"].as_u64().unwrap();
        let listed = t
            .invoke_raw(serde_json::json!({"action": "list", "peer": "alice"}))
            .await
            .unwrap();
        let items = listed["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], id);
        assert_eq!(items[0]["text"], "ship it");
        assert_eq!(items[0]["status"], "pending");
    }

    #[tokio::test]
    async fn set_status_updates_item() {
        let t = TodoTool::new();
        let r = t
            .invoke_raw(serde_json::json!({
                "action": "add", "peer": "x", "text": "task"
            }))
            .await
            .unwrap();
        let id = r["id"].as_u64().unwrap();
        t.invoke_raw(serde_json::json!({
            "action": "set_status", "peer": "x", "id": id, "status": "done"
        }))
        .await
        .unwrap();
        let listed = t
            .invoke_raw(serde_json::json!({"action": "list", "peer": "x"}))
            .await
            .unwrap();
        assert_eq!(listed["items"][0]["status"], "done");
    }

    #[tokio::test]
    async fn remove_drops_item() {
        let t = TodoTool::new();
        let r = t
            .invoke_raw(serde_json::json!({
                "action": "add", "peer": "x", "text": "a"
            }))
            .await
            .unwrap();
        let id = r["id"].as_u64().unwrap();
        t.invoke_raw(serde_json::json!({"action": "remove", "peer": "x", "id": id}))
            .await
            .unwrap();
        let listed = t
            .invoke_raw(serde_json::json!({"action": "list", "peer": "x"}))
            .await
            .unwrap();
        assert_eq!(listed["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_id_isolates_lists() {
        let t = TodoTool::new();
        t.invoke_raw(serde_json::json!({
            "action": "add", "peer": "x", "list_id": "work", "text": "PR review"
        }))
        .await
        .unwrap();
        t.invoke_raw(serde_json::json!({
            "action": "add", "peer": "x", "list_id": "home", "text": "groceries"
        }))
        .await
        .unwrap();
        let work = t
            .invoke_raw(serde_json::json!({"action": "list", "peer": "x", "list_id": "work"}))
            .await
            .unwrap();
        let home = t
            .invoke_raw(serde_json::json!({"action": "list", "peer": "x", "list_id": "home"}))
            .await
            .unwrap();
        assert_eq!(work["items"].as_array().unwrap().len(), 1);
        assert_eq!(home["items"].as_array().unwrap().len(), 1);
        assert_eq!(work["items"][0]["text"], "PR review");
    }

    #[tokio::test]
    async fn unknown_action_rejected() {
        let t = TodoTool::new();
        let err = t
            .invoke_raw(serde_json::json!({"action": "fnord", "peer": "x"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn set_status_unknown_id_errs() {
        let t = TodoTool::new();
        // Ensure the list exists with at least one item.
        t.invoke_raw(serde_json::json!({
            "action": "add", "peer": "x", "text": "a"
        }))
        .await
        .unwrap();
        let err = t
            .invoke_raw(serde_json::json!({
                "action": "set_status", "peer": "x", "id": 99, "status": "done"
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }
}
