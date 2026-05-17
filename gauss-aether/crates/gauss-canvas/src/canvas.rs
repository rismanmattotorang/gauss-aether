//! [`Canvas`] trait + in-memory backend.
//!
//! The trait is async because production backends (Phase-10 `SurrealDB`
//! live-query bridge, REST gateway, WebSocket pump) all return futures.
//! The in-memory impl is deterministic and used by tests + the `gauss
//! doctor` health surface.

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;

use crate::node::{CanvasNode, CanvasUpdate, NodeId};

/// Canvas error.
#[derive(Debug, Clone, Error, Serialize, Deserialize, Eq, PartialEq)]
#[non_exhaustive]
pub enum CanvasError {
    /// Node id not found in the tree.
    #[error("canvas node not found: {0}")]
    NotFound(String),
    /// Reorder children list was not a permutation of the current
    /// children.
    #[error("reorder children list is not a permutation of the existing children")]
    InvalidReorder,
    /// Backend transport / I/O failure.
    #[error("canvas backend i/o: {0}")]
    Io(String),
}

/// Async canvas backend.
#[async_trait]
pub trait Canvas: Send + Sync {
    /// Apply one update to the tree and broadcast to subscribers.
    ///
    /// # Errors
    /// Returns [`CanvasError::NotFound`] when an update references a
    /// missing node; [`CanvasError::InvalidReorder`] when a reorder is
    /// inconsistent; [`CanvasError::Io`] for transport failures.
    async fn apply(&self, update: CanvasUpdate) -> Result<(), CanvasError>;

    /// Get the current tree (defensive clone).
    async fn snapshot(&self) -> Vec<CanvasNode>;

    /// Number of nodes currently in the tree.
    async fn len(&self) -> usize;

    /// True iff the canvas is empty.
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Subscribe to the live update stream. Each subscriber sees every
    /// update produced after the subscription is created.
    fn subscribe(&self) -> broadcast::Receiver<CanvasUpdate>;
}

/// Deterministic in-process canvas.
pub struct InMemoryCanvas {
    state: Mutex<State>,
    tx: broadcast::Sender<CanvasUpdate>,
}

struct State {
    nodes: HashMap<NodeId, CanvasNode>,
    /// Top-level (no-parent) node IDs in order.
    roots: Vec<NodeId>,
    /// For convenience, the parent of each node ID.
    parent: HashMap<NodeId, NodeId>,
}

impl Default for InMemoryCanvas {
    fn default() -> Self {
        Self::with_capacity(64)
    }
}

impl core::fmt::Debug for InMemoryCanvas {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = self.state.lock();
        f.debug_struct("InMemoryCanvas")
            .field("nodes", &s.nodes.len())
            .field("roots", &s.roots.len())
            .field("subscribers", &self.tx.receiver_count())
            .finish_non_exhaustive()
    }
}

impl InMemoryCanvas {
    /// Build with a custom broadcast capacity (must be `>= 1`).
    #[must_use]
    pub fn with_capacity(broadcast_capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(broadcast_capacity.max(1));
        Self {
            state: Mutex::new(State {
                nodes: HashMap::new(),
                roots: Vec::new(),
                parent: HashMap::new(),
            }),
            tx,
        }
    }

    fn delete_recursive(state: &mut State, id: &NodeId) {
        let descendants: Vec<NodeId> = state
            .nodes
            .get(id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        for child in descendants {
            Self::delete_recursive(state, &child);
        }
        state.nodes.remove(id);
        state.parent.remove(id);
        state.roots.retain(|r| r != id);
    }
}

#[async_trait]
impl Canvas for InMemoryCanvas {
    async fn apply(&self, update: CanvasUpdate) -> Result<(), CanvasError> {
        {
            let mut s = self.state.lock();
            match &update {
                CanvasUpdate::Insert { node, parent } => {
                    s.nodes.insert(node.id.clone(), node.clone());
                    if let Some(p) = parent {
                        if !s.nodes.contains_key(p) {
                            return Err(CanvasError::NotFound(p.0.clone()));
                        }
                        s.parent.insert(node.id.clone(), p.clone());
                        if let Some(parent_node) = s.nodes.get_mut(p) {
                            parent_node.children.push(node.id.clone());
                        }
                    } else {
                        s.roots.push(node.id.clone());
                    }
                }
                CanvasUpdate::Update { id, props } => {
                    let n = s
                        .nodes
                        .get_mut(id)
                        .ok_or_else(|| CanvasError::NotFound(id.0.clone()))?;
                    n.props = props.clone();
                }
                CanvasUpdate::Delete { id } => {
                    if !s.nodes.contains_key(id) {
                        return Err(CanvasError::NotFound(id.0.clone()));
                    }
                    Self::delete_recursive(&mut s, id);
                }
                CanvasUpdate::Reorder { parent, children } => {
                    let existing: Vec<NodeId> = s
                        .nodes
                        .get(parent)
                        .ok_or_else(|| CanvasError::NotFound(parent.0.clone()))?
                        .children
                        .clone();
                    if existing.len() != children.len() {
                        return Err(CanvasError::InvalidReorder);
                    }
                    let mut sorted_a = existing;
                    let mut sorted_b = children.clone();
                    sorted_a.sort_by(|x, y| x.0.cmp(&y.0));
                    sorted_b.sort_by(|x, y| x.0.cmp(&y.0));
                    if sorted_a != sorted_b {
                        return Err(CanvasError::InvalidReorder);
                    }
                    if let Some(n) = s.nodes.get_mut(parent) {
                        n.children.clone_from(children);
                    }
                }
            }
        }
        // Broadcast; ignore "no receivers" — it just means no surface is
        // currently subscribed.
        let _ = self.tx.send(update);
        Ok(())
    }

    async fn snapshot(&self) -> Vec<CanvasNode> {
        let s = self.state.lock();
        s.roots
            .iter()
            .filter_map(|id| s.nodes.get(id).cloned())
            .collect()
    }

    async fn len(&self) -> usize {
        self.state.lock().nodes.len()
    }

    fn subscribe(&self) -> broadcast::Receiver<CanvasUpdate> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::WidgetKind;

    #[tokio::test]
    async fn insert_then_snapshot_returns_root() {
        let c = InMemoryCanvas::default();
        c.apply(CanvasUpdate::Insert {
            node: CanvasNode::leaf(
                NodeId::new("greeting"),
                WidgetKind::Text,
                serde_json::json!({"body":"hi"}),
            ),
            parent: None,
        })
        .await
        .unwrap();
        let roots = c.snapshot().await;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id.0, "greeting");
    }

    #[tokio::test]
    async fn update_changes_props() {
        let c = InMemoryCanvas::default();
        c.apply(CanvasUpdate::Insert {
            node: CanvasNode::leaf(
                NodeId::new("t"),
                WidgetKind::Text,
                serde_json::json!({"body":"old"}),
            ),
            parent: None,
        })
        .await
        .unwrap();
        c.apply(CanvasUpdate::Update {
            id: NodeId::new("t"),
            props: serde_json::json!({"body":"new"}),
        })
        .await
        .unwrap();
        let roots = c.snapshot().await;
        assert_eq!(roots[0].props["body"], "new");
    }

    #[tokio::test]
    async fn delete_recurses_through_children() {
        let c = InMemoryCanvas::default();
        c.apply(CanvasUpdate::Insert {
            node: CanvasNode::leaf(
                NodeId::new("root"),
                WidgetKind::Container,
                serde_json::Value::Null,
            ),
            parent: None,
        })
        .await
        .unwrap();
        c.apply(CanvasUpdate::Insert {
            node: CanvasNode::leaf(
                NodeId::new("child"),
                WidgetKind::Text,
                serde_json::Value::Null,
            ),
            parent: Some(NodeId::new("root")),
        })
        .await
        .unwrap();
        assert_eq!(c.len().await, 2);
        c.apply(CanvasUpdate::Delete {
            id: NodeId::new("root"),
        })
        .await
        .unwrap();
        assert_eq!(c.len().await, 0);
    }

    #[tokio::test]
    async fn reorder_rejects_non_permutations() {
        let c = InMemoryCanvas::default();
        c.apply(CanvasUpdate::Insert {
            node: CanvasNode::leaf(
                NodeId::new("root"),
                WidgetKind::Container,
                serde_json::Value::Null,
            ),
            parent: None,
        })
        .await
        .unwrap();
        for id in ["a", "b", "c"] {
            c.apply(CanvasUpdate::Insert {
                node: CanvasNode::leaf(NodeId::new(id), WidgetKind::Text, serde_json::Value::Null),
                parent: Some(NodeId::new("root")),
            })
            .await
            .unwrap();
        }
        let err = c
            .apply(CanvasUpdate::Reorder {
                parent: NodeId::new("root"),
                children: vec![NodeId::new("a"), NodeId::new("b"), NodeId::new("d")],
            })
            .await
            .unwrap_err();
        assert_eq!(err, CanvasError::InvalidReorder);
    }

    #[tokio::test]
    async fn subscribers_see_live_updates() {
        let c = InMemoryCanvas::default();
        let mut rx = c.subscribe();
        c.apply(CanvasUpdate::Insert {
            node: CanvasNode::leaf(NodeId::new("x"), WidgetKind::Text, serde_json::Value::Null),
            parent: None,
        })
        .await
        .unwrap();
        let update = rx.recv().await.unwrap();
        matches!(update, CanvasUpdate::Insert { .. });
    }
}
