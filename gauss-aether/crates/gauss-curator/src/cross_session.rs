//! Cross-session memory map — Honcho parity (Sprint 5 §5).
//!
//! A `MemoryRecord` is keyed by `(PeerId, Namespace, String)`. Records
//! carry a free-form `serde_json::Value` payload + metadata
//! (`created_at`, `last_touched_at`, optional `ttl_seconds`). The
//! [`CrossSessionStore`] trait abstracts persistence; the in-process
//! [`InMemoryStore`] reference impl is what the test suite + CLI
//! smoke runs drive.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Identifier for a "peer" — the human (or sub-agent) the memory
/// belongs to. Distinct from a session id: peers survive session
/// resets. Hermes calls this concept the *peer* in Honcho.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PeerId(pub String);

impl PeerId {
    /// Build from a `&str` (callers should keep this small + stable).
    #[must_use]
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Logical namespace inside a peer's memory map. Hermes uses these to
/// separate identity-level memory (`identity`), session-mode memory
/// (`mode`), and arbitrary key/value scratch (`scratch`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Namespace(pub String);

impl Namespace {
    /// Build from a `&str`.
    #[must_use]
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }

    /// Common namespaces (matching Hermes's `honcho` defaults).
    #[must_use]
    pub fn identity() -> Self {
        Self("identity".into())
    }
    #[must_use]
    pub fn mode() -> Self {
        Self("mode".into())
    }
    #[must_use]
    pub fn scratch() -> Self {
        Self("scratch".into())
    }

    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One memory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// Owning peer.
    pub peer: PeerId,
    /// Namespace.
    pub namespace: Namespace,
    /// Caller-supplied key.
    pub key: String,
    /// Payload (free-form JSON).
    pub value: serde_json::Value,
    /// UNIX seconds when this record was created.
    pub created_at: i64,
    /// UNIX seconds when this record was last read or written.
    pub last_touched_at: i64,
    /// Optional TTL — when set, the curator may archive records
    /// whose `created_at + ttl_seconds < now`.
    pub ttl_seconds: Option<i64>,
}

impl MemoryRecord {
    /// Build a fresh record.
    #[must_use]
    pub fn new(
        peer: PeerId,
        namespace: Namespace,
        key: impl Into<String>,
        value: serde_json::Value,
        now: i64,
    ) -> Self {
        Self {
            peer,
            namespace,
            key: key.into(),
            value,
            created_at: now,
            last_touched_at: now,
            ttl_seconds: None,
        }
    }

    /// Mark this record as touched at `now`.
    pub fn touch(&mut self, now: i64) {
        self.last_touched_at = now;
    }

    /// Return whether `now` strictly exceeds the record's TTL window.
    #[must_use]
    pub fn is_expired(&self, now: i64) -> bool {
        match self.ttl_seconds {
            Some(ttl) => self.created_at.saturating_add(ttl) < now,
            None => false,
        }
    }
}

/// Store-side error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemoryError {
    /// No record exists with the requested key.
    #[error("unknown memory record: {peer:?} / {namespace:?} / {key}")]
    Unknown {
        /// Peer id.
        peer: PeerId,
        /// Namespace.
        namespace: Namespace,
        /// Key.
        key: String,
    },
    /// Backend-side failure.
    #[error("backend: {0}")]
    Backend(String),
}

/// Convenience alias.
pub type MemoryResult<T> = Result<T, MemoryError>;

/// Pluggable persistence for the cross-session memory map.
#[async_trait]
pub trait CrossSessionStore: Send + Sync {
    /// Insert or replace a record.
    async fn put(&self, record: MemoryRecord) -> MemoryResult<()>;

    /// Fetch one record by key. Touches `last_touched_at` if present.
    async fn get(
        &self,
        peer: &PeerId,
        namespace: &Namespace,
        key: &str,
        now: i64,
    ) -> MemoryResult<Option<MemoryRecord>>;

    /// Drop one record.
    async fn delete(&self, peer: &PeerId, namespace: &Namespace, key: &str) -> MemoryResult<()>;

    /// List every record for `peer` in `namespace`.
    async fn list(&self, peer: &PeerId, namespace: &Namespace) -> MemoryResult<Vec<MemoryRecord>>;

    /// List every record across all peers + namespaces. Used by the
    /// curator's stale-scan path.
    async fn list_all(&self) -> MemoryResult<Vec<MemoryRecord>>;
}

/// Reference in-process backend. Cheap to clone (`Arc` of inner Mutex).
#[derive(Debug, Default)]
pub struct InMemoryStore {
    inner: Mutex<BTreeMap<(PeerId, Namespace, String), MemoryRecord>>,
}

impl InMemoryStore {
    /// Build a fresh empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored records.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("poisoned").len()
    }

    /// Whether the backend is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl CrossSessionStore for InMemoryStore {
    async fn put(&self, record: MemoryRecord) -> MemoryResult<()> {
        let key = (
            record.peer.clone(),
            record.namespace.clone(),
            record.key.clone(),
        );
        self.inner.lock().expect("poisoned").insert(key, record);
        Ok(())
    }

    async fn get(
        &self,
        peer: &PeerId,
        namespace: &Namespace,
        key: &str,
        now: i64,
    ) -> MemoryResult<Option<MemoryRecord>> {
        let mut g = self.inner.lock().expect("poisoned");
        let store_key = (peer.clone(), namespace.clone(), key.to_string());
        if let Some(rec) = g.get_mut(&store_key) {
            rec.touch(now);
            Ok(Some(rec.clone()))
        } else {
            Ok(None)
        }
    }

    async fn delete(&self, peer: &PeerId, namespace: &Namespace, key: &str) -> MemoryResult<()> {
        let mut g = self.inner.lock().expect("poisoned");
        let store_key = (peer.clone(), namespace.clone(), key.to_string());
        g.remove(&store_key).ok_or_else(|| MemoryError::Unknown {
            peer: peer.clone(),
            namespace: namespace.clone(),
            key: key.into(),
        })?;
        Ok(())
    }

    async fn list(&self, peer: &PeerId, namespace: &Namespace) -> MemoryResult<Vec<MemoryRecord>> {
        let g = self.inner.lock().expect("poisoned");
        Ok(g.values()
            .filter(|r| &r.peer == peer && &r.namespace == namespace)
            .cloned()
            .collect())
    }

    async fn list_all(&self) -> MemoryResult<Vec<MemoryRecord>> {
        Ok(self
            .inner
            .lock()
            .expect("poisoned")
            .values()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(peer: &str, ns: &str, key: &str, value: &str) -> MemoryRecord {
        MemoryRecord::new(
            PeerId::new(peer),
            Namespace::new(ns),
            key,
            serde_json::json!(value),
            100,
        )
    }

    #[tokio::test]
    async fn put_then_get_round_trip() {
        let s = InMemoryStore::new();
        s.put(make_record("alice", "identity", "name", "Alice"))
            .await
            .unwrap();
        let got = s
            .get(&PeerId::new("alice"), &Namespace::identity(), "name", 200)
            .await
            .unwrap()
            .expect("record present");
        assert_eq!(got.value, serde_json::json!("Alice"));
    }

    #[tokio::test]
    async fn get_touches_last_touched_at() {
        let s = InMemoryStore::new();
        s.put(make_record("alice", "identity", "name", "Alice"))
            .await
            .unwrap();
        let _ = s
            .get(&PeerId::new("alice"), &Namespace::identity(), "name", 500)
            .await
            .unwrap();
        let again = s
            .get(&PeerId::new("alice"), &Namespace::identity(), "name", 600)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again.last_touched_at, 600);
        assert_eq!(again.created_at, 100);
    }

    #[tokio::test]
    async fn list_filters_by_peer_and_namespace() {
        let s = InMemoryStore::new();
        s.put(make_record("alice", "identity", "name", "Alice"))
            .await
            .unwrap();
        s.put(make_record("alice", "scratch", "k", "x"))
            .await
            .unwrap();
        s.put(make_record("bob", "identity", "name", "Bob"))
            .await
            .unwrap();
        let alice_id = s
            .list(&PeerId::new("alice"), &Namespace::identity())
            .await
            .unwrap();
        assert_eq!(alice_id.len(), 1);
        assert_eq!(alice_id[0].peer.as_str(), "alice");
    }

    #[tokio::test]
    async fn list_all_returns_every_record() {
        let s = InMemoryStore::new();
        s.put(make_record("a", "ns", "k1", "v")).await.unwrap();
        s.put(make_record("a", "ns", "k2", "v")).await.unwrap();
        s.put(make_record("b", "ns", "k3", "v")).await.unwrap();
        let all = s.list_all().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn delete_drops_record() {
        let s = InMemoryStore::new();
        s.put(make_record("a", "ns", "k", "v")).await.unwrap();
        s.delete(&PeerId::new("a"), &Namespace::new("ns"), "k")
            .await
            .unwrap();
        assert!(s
            .get(&PeerId::new("a"), &Namespace::new("ns"), "k", 200)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delete_unknown_returns_unknown_error() {
        let s = InMemoryStore::new();
        let err = s
            .delete(&PeerId::new("a"), &Namespace::new("ns"), "missing")
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Unknown { .. }));
    }

    #[test]
    fn is_expired_respects_ttl_and_creation_time() {
        let mut rec = MemoryRecord::new(
            PeerId::new("a"),
            Namespace::new("ns"),
            "k",
            serde_json::json!("v"),
            100,
        );
        rec.ttl_seconds = Some(50);
        assert!(!rec.is_expired(140));
        assert!(rec.is_expired(200));
    }

    #[test]
    fn record_without_ttl_never_expires() {
        let rec = MemoryRecord::new(
            PeerId::new("a"),
            Namespace::new("ns"),
            "k",
            serde_json::json!("v"),
            100,
        );
        assert!(!rec.is_expired(i64::MAX));
    }

    #[test]
    fn namespace_helpers_are_distinct() {
        assert_ne!(Namespace::identity(), Namespace::mode());
        assert_ne!(Namespace::mode(), Namespace::scratch());
    }
}
