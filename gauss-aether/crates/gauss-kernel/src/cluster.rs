//! Consistent-hash cluster routing (paper §XIV.B, Theorem T6).
//!
//! Phase 10 wires a small consistent-hash ring keyed by `SessionId` so a
//! Gauss-Aether cluster can horizontally scale stateless turn execution:
//! each session is pinned to one node, and a node addition / removal
//! re-routes only `O(1/N)` of the sessions on average.
//!
//! The implementation uses SHA-256 prefixes for the hash and `V`
//! virtual nodes per physical node (default 128) so the ring stays
//! roughly balanced even at small `N`. The ring is a `parking_lot::
//! Mutex<BTreeMap<u64, NodeId>>` — read-mostly, with `add_node` /
//! `remove_node` taking the lock briefly.

use std::collections::BTreeMap;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Cluster node identifier — opaque string, e.g. `"gauss-1.eu-west-1"`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    /// Construct.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// Default virtual-node count per physical node.
pub const DEFAULT_VNODES: u16 = 128;

/// Consistent-hash ring.
#[derive(Debug)]
pub struct ConsistentHashRing {
    inner: Mutex<Inner>,
    vnodes_per_node: u16,
}

#[derive(Debug)]
struct Inner {
    /// Hash → node-id; the next-greater key wins.
    ring: BTreeMap<u64, NodeId>,
    /// All known node ids (for diagnostics + removal).
    nodes: Vec<NodeId>,
}

impl Default for ConsistentHashRing {
    fn default() -> Self {
        Self::new(DEFAULT_VNODES)
    }
}

impl ConsistentHashRing {
    /// Build a ring with `vnodes_per_node` virtual nodes per physical
    /// node (must be `>= 1`; `0` is normalised to 1).
    #[must_use]
    pub fn new(vnodes_per_node: u16) -> Self {
        Self {
            inner: Mutex::new(Inner {
                ring: BTreeMap::new(),
                nodes: Vec::new(),
            }),
            vnodes_per_node: vnodes_per_node.max(1),
        }
    }

    /// Add a node. Idempotent: re-adding an existing node is a no-op.
    pub fn add_node(&self, node: NodeId) {
        let mut inner = self.inner.lock();
        if inner.nodes.iter().any(|n| n == &node) {
            return;
        }
        for v in 0..self.vnodes_per_node {
            let key = vnode_hash(&node, v);
            inner.ring.insert(key, node.clone());
        }
        inner.nodes.push(node);
    }

    /// Remove a node. No-op if not present.
    pub fn remove_node(&self, node: &NodeId) {
        let mut inner = self.inner.lock();
        if !inner.nodes.iter().any(|n| n == node) {
            return;
        }
        for v in 0..self.vnodes_per_node {
            let key = vnode_hash(node, v);
            inner.ring.remove(&key);
        }
        inner.nodes.retain(|n| n != node);
    }

    /// Number of physical nodes currently in the ring.
    pub fn node_count(&self) -> usize {
        self.inner.lock().nodes.len()
    }

    /// Number of virtual nodes (= `node_count * vnodes_per_node`).
    pub fn vnode_count(&self) -> usize {
        self.inner.lock().ring.len()
    }

    /// Route `key` to a node. Returns `None` iff the ring is empty.
    pub fn route(&self, key: &[u8]) -> Option<NodeId> {
        let h = hash_u64(key);
        let inner = self.inner.lock();
        if inner.ring.is_empty() {
            return None;
        }
        // First key >= h, or wrap around.
        inner.ring.range(h..).next().map_or_else(
            || inner.ring.values().next().cloned(),
            |(_, n)| Some(n.clone()),
        )
    }

    /// Convenience: route a session id (as bytes).
    pub fn route_session(&self, session_id: &str) -> Option<NodeId> {
        self.route(session_id.as_bytes())
    }

    /// Snapshot the configured nodes.
    pub fn nodes(&self) -> Vec<NodeId> {
        self.inner.lock().nodes.clone()
    }
}

fn vnode_hash(node: &NodeId, v: u16) -> u64 {
    let mut h = Sha256::new();
    h.update(node.0.as_bytes());
    h.update(b"#");
    h.update(v.to_le_bytes());
    let out = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&out[..8]);
    u64::from_le_bytes(b)
}

fn hash_u64(bytes: &[u8]) -> u64 {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&out[..8]);
    u64::from_le_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ring_routes_to_none() {
        let r = ConsistentHashRing::default();
        assert!(r.route(b"any").is_none());
    }

    #[test]
    fn single_node_owns_every_key() {
        let r = ConsistentHashRing::default();
        let n = NodeId::new("only");
        r.add_node(n.clone());
        for k in [b"a".as_ref(), b"b", b"c", b"d", b"e"] {
            assert_eq!(r.route(k), Some(n.clone()));
        }
    }

    #[test]
    fn adding_a_node_reroutes_at_most_one_over_n_keys() {
        let r = ConsistentHashRing::new(64);
        for name in ["one", "two", "three"] {
            r.add_node(NodeId::new(name));
        }
        // Sample 1000 session ids.
        let keys: Vec<String> = (0..1000_u64).map(|i| format!("session-{i}")).collect();
        let before: Vec<NodeId> = keys.iter().map(|k| r.route_session(k).unwrap()).collect();

        // Add a fourth node.
        r.add_node(NodeId::new("four"));

        let after: Vec<NodeId> = keys.iter().map(|k| r.route_session(k).unwrap()).collect();

        let moved = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        // Theory: with 4 nodes and uniform hash, ~25 % of keys move. We
        // assert a loose upper bound to accommodate the small vnode count.
        assert!(
            moved < 400,
            "too many keys moved after adding a 4th node: {moved}/1000"
        );
    }

    #[test]
    fn idempotent_add_node() {
        let r = ConsistentHashRing::default();
        r.add_node(NodeId::new("a"));
        r.add_node(NodeId::new("a"));
        r.add_node(NodeId::new("a"));
        assert_eq!(r.node_count(), 1);
    }

    #[test]
    fn remove_node_actually_removes() {
        let r = ConsistentHashRing::new(16);
        r.add_node(NodeId::new("a"));
        r.add_node(NodeId::new("b"));
        assert_eq!(r.node_count(), 2);
        r.remove_node(&NodeId::new("a"));
        assert_eq!(r.node_count(), 1);
        // All keys now route to "b".
        for k in [b"x".as_ref(), b"y", b"z"] {
            assert_eq!(r.route(k).unwrap().0, "b");
        }
    }

    #[test]
    fn vnode_count_scales_with_nodes_and_vnodes_per_node() {
        let r = ConsistentHashRing::new(8);
        r.add_node(NodeId::new("a"));
        r.add_node(NodeId::new("b"));
        assert_eq!(r.vnode_count(), 16);
    }

    #[test]
    fn balance_is_within_an_order_of_magnitude_of_uniform() {
        let r = ConsistentHashRing::new(128);
        for name in ["a", "b", "c", "d"] {
            r.add_node(NodeId::new(name));
        }
        let mut counts = std::collections::HashMap::<NodeId, u32>::new();
        for i in 0..10_000_u64 {
            let key = format!("session-{i}");
            *counts.entry(r.route_session(&key).unwrap()).or_insert(0) += 1;
        }
        let min = counts.values().min().copied().unwrap_or(0);
        let max = counts.values().max().copied().unwrap_or(0);
        // 4 nodes × 10k samples → ~2500 each. Allow a 4x spread.
        assert!(max <= min.saturating_mul(4), "min={min} max={max}");
    }
}
