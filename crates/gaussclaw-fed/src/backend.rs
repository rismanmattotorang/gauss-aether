//! Pluggable pool backend.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;

/// Backend-side errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PoolError {
    /// Object already exists at this key (publish refused duplicate).
    #[error("object already exists: {0}")]
    Duplicate(String),
    /// Object not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Backend I/O / transport failure.
    #[error("backend: {0}")]
    Backend(String),
    /// Envelope verification failed during admission.
    #[error("envelope verification: {0}")]
    Verification(String),
    /// Admission policy refused the envelope.
    #[error("admission: {0}")]
    Admission(String),
}

/// Convenience alias.
pub type PoolResult<T> = Result<T, PoolError>;

/// One pool entry: the canonical object key and the envelope bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolEntry {
    /// Canonical object key (`{org}/{chain_head_hex}/{turn_id}.env.json`).
    pub key: String,
    /// Serialised [`gaussclaw_export::Envelope`].
    pub bytes: Vec<u8>,
}

/// Pluggable storage backend for [`crate::FederatedPool`].
///
/// Implementors guarantee:
///
/// - `put` is idempotent at the **key + bytes** level: a re-put of the
///   same key with the **same bytes** returns `Ok(())`; a re-put with
///   **different bytes** returns [`PoolError::Duplicate`].
/// - `get` returns the most-recently-put bytes for a key, or
///   [`PoolError::NotFound`].
/// - `list` returns entries with keys starting with `prefix` in some
///   deterministic order (lexicographic for the in-memory + filesystem
///   backends).
#[async_trait]
pub trait PoolBackend: Send + Sync {
    /// Put one entry. Idempotent on (key, bytes); rejects key collision
    /// with different bytes.
    async fn put(&self, entry: PoolEntry) -> PoolResult<()>;

    /// Fetch bytes by key.
    async fn get(&self, key: &str) -> PoolResult<Vec<u8>>;

    /// List entries whose key starts with `prefix`.
    async fn list(&self, prefix: &str) -> PoolResult<Vec<PoolEntry>>;
}

/// `Arc<Mutex<HashMap>>`-backed reference impl. Used by tests and the
/// desktop "Federated Preview" pane.
#[derive(Debug, Default)]
pub struct InMemoryPoolBackend {
    inner: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl InMemoryPoolBackend {
    /// Build a fresh empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of entries.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    /// True iff the backend is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

#[async_trait]
impl PoolBackend for InMemoryPoolBackend {
    async fn put(&self, entry: PoolEntry) -> PoolResult<()> {
        let mut g = self.inner.lock().await;
        if let Some(existing) = g.get(&entry.key) {
            if existing == &entry.bytes {
                return Ok(());
            }
            return Err(PoolError::Duplicate(entry.key));
        }
        g.insert(entry.key, entry.bytes);
        Ok(())
    }

    async fn get(&self, key: &str) -> PoolResult<Vec<u8>> {
        self.inner
            .lock()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| PoolError::NotFound(key.into()))
    }

    async fn list(&self, prefix: &str) -> PoolResult<Vec<PoolEntry>> {
        let g = self.inner.lock().await;
        let mut out: Vec<PoolEntry> = g
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| PoolEntry {
                key: k.clone(),
                bytes: v.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_round_trips() {
        let b = InMemoryPoolBackend::new();
        assert!(b.is_empty().await);
        b.put(PoolEntry {
            key: "org/aa/1.env.json".into(),
            bytes: vec![1, 2, 3],
        })
        .await
        .unwrap();
        assert_eq!(b.len().await, 1);
        assert_eq!(b.get("org/aa/1.env.json").await.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn duplicate_with_same_bytes_is_idempotent() {
        let b = InMemoryPoolBackend::new();
        b.put(PoolEntry {
            key: "k".into(),
            bytes: vec![1],
        })
        .await
        .unwrap();
        b.put(PoolEntry {
            key: "k".into(),
            bytes: vec![1],
        })
        .await
        .unwrap();
        assert_eq!(b.len().await, 1);
    }

    #[tokio::test]
    async fn duplicate_with_different_bytes_is_rejected() {
        let b = InMemoryPoolBackend::new();
        b.put(PoolEntry {
            key: "k".into(),
            bytes: vec![1],
        })
        .await
        .unwrap();
        let err = b
            .put(PoolEntry {
                key: "k".into(),
                bytes: vec![2],
            })
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::Duplicate(_)));
    }

    #[tokio::test]
    async fn list_filters_by_prefix_and_sorts() {
        let b = InMemoryPoolBackend::new();
        for (k, v) in [("a/3", 3), ("a/1", 1), ("b/2", 2), ("a/2", 4)] {
            b.put(PoolEntry {
                key: k.into(),
                bytes: vec![v],
            })
            .await
            .unwrap();
        }
        let a = b.list("a/").await.unwrap();
        assert_eq!(a.len(), 3);
        let keys: Vec<&str> = a.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["a/1", "a/2", "a/3"]);
    }

    #[tokio::test]
    async fn get_unknown_key_returns_not_found() {
        let b = InMemoryPoolBackend::new();
        let err = b.get("nope").await.unwrap_err();
        assert!(matches!(err, PoolError::NotFound(_)));
    }
}
