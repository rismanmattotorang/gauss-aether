//! `gauss-memory` — Trinity Memory Substrate.
//!
//! Phase 1 ships:
//!
//! * The Phase-0 `InMemoryMonoid` (synchronous, `MemoryMonoid` trait).
//! * A new asynchronous backend that implements
//!   [`gauss_traits::MemoryBackend`] over **`SurrealDB`** (embedded engine).
//! * The `SurrealQL` schema declared up-front — one append-only `turn_record`
//!   table with derived indices: a UNIQUE index on `turn_id`, an HNSW vector
//!   index reserved for Phase-6 hybrid recall, an analyzer + FTS index for
//!   keyword recall, and `RELATE`-based graph lineage between turns.
//! * A SHA-256 chain head materialised alongside the log so the receipt-chain
//!   work in Phase 5 can layer Ed25519 signatures on top without touching
//!   the storage schema again.
//!
//! Later phases close out FTS recall (Phase 6), HNSW vector recall (Phase 6),
//! K-LRU prefix-tree caching (Phase 6), and the Merkle-anchoring API
//! (Phase 5).

use gauss_core::{GaussError, GaussResult, TurnId};
use std::sync::Mutex;

pub mod hybrid;
pub mod klru;
pub mod schema;
pub mod snapshot;

#[cfg(feature = "surrealdb-embedded")]
pub mod surreal;

#[cfg(feature = "surrealdb-embedded")]
pub use surreal::SurrealMemory;

pub use hybrid::{
    cosine_dot, hash_bucket_embedding, l2_normalise, tokenize, HybridMemory, DEFAULT_EMBED_DIM,
};
pub use klru::{Node, PrefixTree, Stats as PrefixStats, DEFAULT_CAPACITY, DEFAULT_K};
pub use schema::{Schema, TURN_RECORD_TABLE};
pub use snapshot::myers;

/// A single append-log entry. Phase 0 is opaque bytes; Phases 2/5 attach the
/// receipt + delta.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// The turn that wrote this entry.
    pub turn_id: TurnId,
    /// Opaque record body. Phase 2 will replace `Vec<u8>` with a typed delta.
    pub body: Vec<u8>,
}

/// The Phase-0 memory monoid abstraction (synchronous).
///
/// `gauss_traits::MemoryBackend` is the async equivalent and is the surface
/// used by the `SurrealDB` backend.
pub trait MemoryMonoid: Send + Sync {
    /// Append a record. Must be durable before any side-effect commits
    /// (see Axiom 1 / `gauss-turn` for the WAL barrier discipline).
    fn append(&self, entry: LogEntry) -> GaussResult<()>;

    /// Number of records currently in the log.
    fn len(&self) -> usize;

    /// True if the log is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// In-memory backend, useful for tests and Phase-0 development.
#[derive(Debug, Default)]
pub struct InMemoryMonoid {
    entries: Mutex<Vec<LogEntry>>,
}

impl InMemoryMonoid {
    /// Construct an empty in-memory monoid.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl MemoryMonoid for InMemoryMonoid {
    fn append(&self, entry: LogEntry) -> GaussResult<()> {
        self.entries
            .lock()
            .map_err(|e| GaussError::Internal(format!("memory mutex poisoned: {e}")))?
            .push(entry);
        Ok(())
    }

    fn len(&self) -> usize {
        self.entries.lock().map(|g| g.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_increments_length() {
        let m = InMemoryMonoid::new();
        assert!(m.is_empty());
        m.append(LogEntry {
            turn_id: TurnId::new(1),
            body: vec![1, 2, 3],
        })
        .unwrap();
        assert_eq!(m.len(), 1);
    }
}
