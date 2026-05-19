//! Background review — per-turn autosave hook (Sprint 5 §7).
//!
//! Hermes's `agent/background_review.py` forks a memory-only loop
//! after each user/assistant turn to (a) extract candidate
//! skills / memories from the turn, (b) write them into the Honcho
//! map. We ship the structural primitive:
//!
//! `BackgroundReviewer::record_turn(...)` accepts a turn's user input
//! plus assistant output plus an optional summary string. It builds a
//! `ReviewEntry` and persists it through the `CrossSessionStore`.
//!
//! The agent loop's `LoopSink` plumbs into this — every `Assistant`
//! event fires `record_turn`, and the result is a per-peer rolling
//! transcript stored in [`crate::cross_session::Namespace::scratch`].

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::cross_session::{CrossSessionStore, MemoryError, MemoryRecord, Namespace, PeerId};

/// One reviewed turn — what gets written to the cross-session store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEntry {
    /// Session that produced this turn.
    pub session_id: String,
    /// Turn id within that session.
    pub turn_id: u64,
    /// What the user said (verbatim).
    pub user_input: String,
    /// What the assistant returned (verbatim).
    pub assistant_output: String,
    /// Optional one-line summary — populated by the reviewer's LLM
    /// pass when available.
    pub summary: Option<String>,
    /// UNIX seconds the entry was recorded.
    pub recorded_at: i64,
}

/// Reviewer-side error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ReviewError {
    /// Underlying store rejected the write.
    #[error("store: {0}")]
    Store(#[from] MemoryError),
}

/// Background reviewer. Wraps a [`CrossSessionStore`] so the host
/// can plug in any backend.
pub struct BackgroundReviewer {
    store: Arc<dyn CrossSessionStore>,
}

impl BackgroundReviewer {
    /// Build a reviewer over a store.
    #[must_use]
    pub fn new(store: Arc<dyn CrossSessionStore>) -> Self {
        Self { store }
    }

    /// Record one turn's user + assistant text into the cross-session
    /// store. The record is written under
    /// `(peer, Namespace::scratch(), "session/<session_id>/turn/<turn_id>")`.
    ///
    /// # Errors
    /// Returns [`ReviewError::Store`] on backend failure.
    pub async fn record_turn(
        &self,
        peer: PeerId,
        entry: ReviewEntry,
    ) -> Result<MemoryRecord, ReviewError> {
        let key = format!("session/{}/turn/{}", entry.session_id, entry.turn_id);
        let value = serde_json::to_value(&entry).unwrap_or(serde_json::Value::Null);
        let record = MemoryRecord::new(peer, Namespace::scratch(), key, value, entry.recorded_at);
        self.store.put(record.clone()).await?;
        Ok(record)
    }

    /// Borrow the underlying store (for tests + composition).
    #[must_use]
    pub fn store(&self) -> &Arc<dyn CrossSessionStore> {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross_session::InMemoryStore;

    fn sample_entry(turn: u64) -> ReviewEntry {
        ReviewEntry {
            session_id: "sess-1".into(),
            turn_id: turn,
            user_input: "hello".into(),
            assistant_output: "hi".into(),
            summary: None,
            recorded_at: 100,
        }
    }

    #[tokio::test]
    async fn record_turn_writes_to_scratch_namespace() {
        let store: Arc<dyn CrossSessionStore> = Arc::new(InMemoryStore::new());
        let r = BackgroundReviewer::new(store.clone());
        let written = r
            .record_turn(PeerId::new("alice"), sample_entry(1))
            .await
            .unwrap();
        assert_eq!(written.peer.as_str(), "alice");
        assert_eq!(written.namespace.as_str(), "scratch");
        assert_eq!(written.key, "session/sess-1/turn/1");
    }

    #[tokio::test]
    async fn record_turn_is_idempotent_on_same_key() {
        let store: Arc<dyn CrossSessionStore> = Arc::new(InMemoryStore::new());
        let r = BackgroundReviewer::new(store);
        r.record_turn(PeerId::new("alice"), sample_entry(1))
            .await
            .unwrap();
        // Second record for the same turn id overwrites.
        let mut e2 = sample_entry(1);
        e2.summary = Some("updated".into());
        let again = r.record_turn(PeerId::new("alice"), e2).await.unwrap();
        let payload: ReviewEntry = serde_json::from_value(again.value.clone()).unwrap();
        assert_eq!(payload.summary.as_deref(), Some("updated"));
    }

    #[tokio::test]
    async fn record_multiple_turns_round_trip() {
        let store: Arc<dyn CrossSessionStore> = Arc::new(InMemoryStore::new());
        let r = BackgroundReviewer::new(store.clone());
        for i in 1..=3 {
            r.record_turn(PeerId::new("alice"), sample_entry(i))
                .await
                .unwrap();
        }
        let all = store
            .list(&PeerId::new("alice"), &Namespace::scratch())
            .await
            .unwrap();
        assert_eq!(all.len(), 3);
    }
}
