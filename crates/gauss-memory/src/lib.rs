//! `gauss-memory` — Trinity Memory Substrate (skeleton).
//!
//! Phase 0 ships the trait surface and an in-memory `Vec`-backed log so the
//! workspace compiles and the type API is testable. Phases 2 (append log),
//! 6 (FTS/HNSW/K-LRU), and the trinity hybrid recall are the real work.

use gauss_core::{GaussError, GaussResult, TurnId};
use std::sync::Mutex;

/// A single append-log entry. Phase 0 is opaque bytes; Phases 2/5 attach the
/// receipt + delta.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// The turn that wrote this entry.
    pub turn_id: TurnId,
    /// Opaque record body. Phase 2 will replace `Vec<u8>` with a typed delta.
    pub body: Vec<u8>,
}

/// The Phase-0 memory monoid abstraction. Real implementations land in the
/// later phases.
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
