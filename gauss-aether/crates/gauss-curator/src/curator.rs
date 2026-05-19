//! Background curator (Sprint 5 §6).
//!
//! Hermes's `agent/curator.py` runs a daemon-plane task that:
//!
//! 1. Walks the skill / memory store every N minutes.
//! 2. Flags records untouched for ≥ 30 days as stale.
//! 3. Archives the stale set (and, in the LLM-driven variant, merges
//!    narrow skills into umbrella skills).
//!
//! This crate ships the **scan + archive** primitives. The
//! LLM-driven "consolidate" step is a thin trait
//! ([`SkillSummariser`]) so the agent loop can plug in whichever
//! provider it likes.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cross_session::{CrossSessionStore, MemoryRecord, MemoryResult};

/// One stale record found by [`Curator::scan_stale`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleRecord {
    /// The underlying memory record.
    pub record: MemoryRecord,
    /// How many seconds past the staleness threshold this record is.
    /// Always `>= 0` when surfaced.
    pub overdue_seconds: i64,
}

/// Aggregate report from one scan pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScanReport {
    /// Stale records, sorted by `overdue_seconds` descending so the
    /// caller can act on the most-overdue first.
    pub stale: Vec<StaleRecord>,
    /// Total records scanned.
    pub scanned: u64,
}

/// Outcome of [`Curator::archive_stale`] — what was actually
/// archived (i.e. deleted from the cross-session store).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArchiveOutcome {
    /// Number of records archived.
    pub archived: u64,
    /// Sum of bytes removed (serde JSON length).
    pub bytes_removed: u64,
}

/// Optional plug-point — host wires in an LLM-driven summariser to
/// merge narrow skills into umbrella skills. The scan loop calls
/// `summarise` once per stale group; without an implementation the
/// curator's "consolidate" step is a no-op (records are simply
/// archived).
pub trait SkillSummariser: Send + Sync {
    /// Produce a one-line summary for a stale set. Caller is free to
    /// no-op (return `None`); the curator treats `None` as
    /// "skip the consolidation step, just archive".
    fn summarise(&self, group: &[MemoryRecord]) -> Option<String>;
}

/// The curator. Cheap to clone (`Arc<dyn CrossSessionStore>` inside).
pub struct Curator {
    store: Arc<dyn CrossSessionStore>,
}

impl Curator {
    /// Build a curator over a store.
    #[must_use]
    pub fn new(store: Arc<dyn CrossSessionStore>) -> Self {
        Self { store }
    }

    /// Scan the store for records untouched for ≥ `max_age_seconds`.
    /// Returns the stale set ordered most-overdue first.
    ///
    /// # Errors
    /// Returns [`crate::MemoryError`] on backend failure.
    pub async fn scan_stale(&self, now: i64, max_age_seconds: i64) -> MemoryResult<ScanReport> {
        let all = self.store.list_all().await?;
        let scanned = all.len() as u64;
        let mut stale: Vec<StaleRecord> = all
            .into_iter()
            .filter_map(|r| {
                let age = now.saturating_sub(r.last_touched_at);
                if age >= max_age_seconds {
                    Some(StaleRecord {
                        overdue_seconds: age.saturating_sub(max_age_seconds),
                        record: r,
                    })
                } else {
                    None
                }
            })
            .collect();
        stale.sort_by(|a, b| b.overdue_seconds.cmp(&a.overdue_seconds));
        Ok(ScanReport { stale, scanned })
    }

    /// Archive (i.e. delete) every stale record from the store.
    /// Returns aggregate counts.
    ///
    /// # Errors
    /// Returns [`crate::MemoryError`] on backend failure.
    pub async fn archive_stale(&self, report: &ScanReport) -> MemoryResult<ArchiveOutcome> {
        let mut archived: u64 = 0;
        let mut bytes_removed: u64 = 0;
        for stale in &report.stale {
            let r = &stale.record;
            bytes_removed = bytes_removed.saturating_add(
                u64::try_from(serde_json::to_vec(&r.value).map(|b| b.len()).unwrap_or(0))
                    .unwrap_or(0),
            );
            self.store.delete(&r.peer, &r.namespace, &r.key).await?;
            archived = archived.saturating_add(1);
        }
        Ok(ArchiveOutcome {
            archived,
            bytes_removed,
        })
    }

    /// Optional second pass — feed the stale set through a
    /// [`SkillSummariser`] and write the consolidation back as a new
    /// "umbrella" memory record. Returns the new umbrella record, if
    /// any.
    ///
    /// # Errors
    /// Returns [`crate::MemoryError`] on backend failure.
    pub async fn consolidate(
        &self,
        report: &ScanReport,
        summariser: &dyn SkillSummariser,
        umbrella_peer: crate::cross_session::PeerId,
        umbrella_namespace: crate::cross_session::Namespace,
        umbrella_key: impl Into<String>,
        now: i64,
    ) -> MemoryResult<Option<MemoryRecord>> {
        if report.stale.is_empty() {
            return Ok(None);
        }
        let group: Vec<MemoryRecord> = report.stale.iter().map(|s| s.record.clone()).collect();
        let Some(summary) = summariser.summarise(&group) else {
            return Ok(None);
        };
        let key: String = umbrella_key.into();
        let umbrella = MemoryRecord::new(
            umbrella_peer,
            umbrella_namespace,
            key,
            serde_json::json!({
                "kind":   "umbrella_summary",
                "summary": summary,
                "merged_count": group.len(),
            }),
            now,
        );
        self.store.put(umbrella.clone()).await?;
        Ok(Some(umbrella))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross_session::{InMemoryStore, MemoryRecord, Namespace, PeerId};

    fn rec(peer: &str, key: &str, last_touched: i64) -> MemoryRecord {
        let mut r = MemoryRecord::new(
            PeerId::new(peer),
            Namespace::new("scratch"),
            key,
            serde_json::json!({"v": key}),
            0,
        );
        r.last_touched_at = last_touched;
        r
    }

    #[tokio::test]
    async fn scan_stale_returns_records_above_threshold() {
        let s = Arc::new(InMemoryStore::new());
        // Three records: two old, one fresh.
        s.put(rec("a", "old1", 100)).await.unwrap();
        s.put(rec("a", "old2", 200)).await.unwrap();
        s.put(rec("a", "fresh", 900)).await.unwrap();
        let c = Curator::new(s.clone());
        // now=1000, threshold=600 → old1 (age 900, overdue 300), old2 (age 800, overdue 200) stale.
        let report = c.scan_stale(1000, 600).await.unwrap();
        assert_eq!(report.scanned, 3);
        assert_eq!(report.stale.len(), 2);
        // Most-overdue first.
        assert_eq!(report.stale[0].record.key, "old1");
        assert_eq!(report.stale[0].overdue_seconds, 300);
        assert_eq!(report.stale[1].record.key, "old2");
    }

    #[tokio::test]
    async fn scan_stale_with_no_old_records_is_empty() {
        let s = Arc::new(InMemoryStore::new());
        s.put(rec("a", "fresh", 900)).await.unwrap();
        let c = Curator::new(s);
        let report = c.scan_stale(1000, 600).await.unwrap();
        assert!(report.stale.is_empty());
        assert_eq!(report.scanned, 1);
    }

    #[tokio::test]
    async fn archive_stale_deletes_records() {
        let s = Arc::new(InMemoryStore::new());
        s.put(rec("a", "old", 0)).await.unwrap();
        let c = Curator::new(s.clone());
        let report = c.scan_stale(1000, 100).await.unwrap();
        let outcome = c.archive_stale(&report).await.unwrap();
        assert_eq!(outcome.archived, 1);
        assert!(s.is_empty());
    }

    struct StaticSummariser;
    impl SkillSummariser for StaticSummariser {
        fn summarise(&self, group: &[MemoryRecord]) -> Option<String> {
            Some(format!("umbrella over {} records", group.len()))
        }
    }

    #[tokio::test]
    async fn consolidate_writes_umbrella_record() {
        let s = Arc::new(InMemoryStore::new());
        s.put(rec("a", "old1", 0)).await.unwrap();
        s.put(rec("a", "old2", 0)).await.unwrap();
        let c = Curator::new(s.clone());
        let report = c.scan_stale(1000, 100).await.unwrap();
        let summariser = StaticSummariser;
        let umbrella = c
            .consolidate(
                &report,
                &summariser,
                PeerId::new("a"),
                Namespace::new("scratch"),
                "umbrella-key",
                1000,
            )
            .await
            .unwrap()
            .expect("umbrella record returned");
        assert_eq!(umbrella.value["kind"], "umbrella_summary");
        assert_eq!(umbrella.value["merged_count"], 2);
    }

    #[tokio::test]
    async fn consolidate_empty_report_returns_none() {
        let s = Arc::new(InMemoryStore::new());
        let c = Curator::new(s);
        let report = ScanReport::default();
        let summariser = StaticSummariser;
        let res = c
            .consolidate(
                &report,
                &summariser,
                PeerId::new("a"),
                Namespace::new("ns"),
                "k",
                0,
            )
            .await
            .unwrap();
        assert!(res.is_none());
    }
}
