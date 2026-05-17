//! Canvas widget tree types.

use serde::{Deserialize, Serialize};

/// Stable identifier for a canvas node. Surfaces reconcile updates by
/// matching incoming `NodeId`s against the local tree.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    /// Construct a node ID.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Widget kinds named in SPECS §XIII.B. New kinds extend the enum at
/// semver-minor (the enum is `#[non_exhaustive]`).
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WidgetKind {
    /// Plain text block.
    Text,
    /// Push button.
    Button,
    /// Two-column key/value table.
    KeyValueTable,
    /// Single image / chart.
    Image,
    /// Human approval prompt — surfaces the Phase-7 `ApprovalRequest`
    /// inline alongside Approve / Deny buttons.
    ApprovalPrompt,
    /// Generic container; rendered as a section in the surface.
    Container,
    /// Markdown block (subset rendered by the surface).
    Markdown,
    /// Free-form JSON payload for custom surface adapters.
    Custom,
}

/// A canvas node.
///
/// Nodes form a tree via [`Self::children`]; props are typed at the
/// `WidgetKind` level (each kind documents its expected props in the
/// SPECS).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CanvasNode {
    /// Stable identifier.
    pub id: NodeId,
    /// Widget kind.
    pub kind: WidgetKind,
    /// Widget properties (kind-specific schema).
    pub props: serde_json::Value,
    /// Child nodes, rendered top-to-bottom.
    pub children: Vec<NodeId>,
}

impl CanvasNode {
    /// Construct a leaf node (no children).
    #[must_use]
    pub const fn leaf(id: NodeId, kind: WidgetKind, props: serde_json::Value) -> Self {
        Self {
            id,
            kind,
            props,
            children: Vec::new(),
        }
    }

    /// Construct a node with children.
    #[must_use]
    pub const fn with_children(
        id: NodeId,
        kind: WidgetKind,
        props: serde_json::Value,
        children: Vec<NodeId>,
    ) -> Self {
        Self {
            id,
            kind,
            props,
            children,
        }
    }
}

/// One reconciliation command emitted to the surface.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
#[non_exhaustive]
pub enum CanvasUpdate {
    /// Insert a new node.
    Insert {
        /// The node to insert.
        node: CanvasNode,
        /// Optional parent ID; `None` means top-level root.
        parent: Option<NodeId>,
    },
    /// Replace an existing node's props.
    Update {
        /// The node to update.
        id: NodeId,
        /// New props.
        props: serde_json::Value,
    },
    /// Remove a node + its descendants.
    Delete {
        /// The node to remove.
        id: NodeId,
    },
    /// Reorder children of a parent.
    Reorder {
        /// Parent whose children are being reordered.
        parent: NodeId,
        /// New children order (must be a permutation of the current
        /// children).
        children: Vec<NodeId>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_serde() {
        let n = CanvasNode::leaf(
            NodeId::new("hello"),
            WidgetKind::Text,
            serde_json::json!({ "body": "hi" }),
        );
        let s = serde_json::to_string(&n).unwrap();
        let back: CanvasNode = serde_json::from_str(&s).unwrap();
        assert_eq!(back, n);
    }

    #[test]
    fn update_tag_is_kebab_for_wire_format() {
        let u = CanvasUpdate::Insert {
            node: CanvasNode::leaf(
                NodeId::new("x"),
                WidgetKind::Markdown,
                serde_json::Value::Null,
            ),
            parent: None,
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(s.contains("\"op\":\"insert\""));
    }
}
