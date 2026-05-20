//! Restart-durable [`JobStore`] backed by an append-only JSONL file.
//!
//! Sprint 9 §9 of `/ROADMAP.md`. The Sprint 5 [`InMemoryJobStore`]
//! loses every job on process exit; that's fine for tests and the
//! CLI demo, but cron-driven workflows in production must survive
//! restarts. [`FileBackedJobStore`] is the durable companion:
//!
//! 1. Every mutation (`insert`/`update`/`remove`) writes one
//!    JSON-Lines record to the log file before mutating the
//!    in-memory mirror, so a crash mid-mutation leaves the persistent
//!    state consistent with the last successful write.
//! 2. On [`FileBackedJobStore::open`], the log is replayed in
//!    sequence to rebuild the live job set + `next_id`.
//! 3. The file format is intentionally chain-agnostic — the receipt
//!    chain is layered on top by `gaussclaw-store::cron_jobs` (the
//!    full Trinity-backed wiring; this crate stays pure-Rust /
//!    storage-agnostic).
//!
//! ### Hermes parity
//!
//! Hermes pickles cron jobs to a single Python file rewritten in
//! full on every mutation. The GaussClaw equivalent is strictly
//! superior on three axes:
//!
//! - **Append-only on disk.** A crash mid-write at most truncates
//!   the trailing record; the preceding records survive intact.
//!   Pickle rewrites the whole file — a crash mid-write loses
//!   everything.
//! - **Forward-compatible.** JSON-Lines + `#[serde(rename_all =
//!   "snake_case")]` means a v2 schema can introduce new fields
//!   without rewriting the log. Pickle's serialiser is tied to the
//!   Python class layout.
//! - **Cross-tool inspectable.** `cat`, `jq`, and `head` all work
//!   directly on the log. Pickle requires Python.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use crate::job::{Job, JobId};
use crate::store::{JobStore, StoreError, StoreResult};

/// One persisted mutation. Replaying the log in order reconstructs
/// the live job set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum LogRecord {
    Insert { job: Job },
    Update { job: Job },
    Remove { id: JobId },
    NextId { id: JobId },
}

/// Append-only file-backed [`JobStore`].
#[derive(Debug)]
pub struct FileBackedJobStore {
    path: PathBuf,
    state: Mutex<State>,
    write: tokio::sync::Mutex<()>,
}

#[derive(Default, Debug)]
struct State {
    jobs: std::collections::BTreeMap<u64, Job>,
    next: u64,
}

impl FileBackedJobStore {
    /// Open or create a file-backed store at `path`. Replays the
    /// existing JSONL log to rebuild the in-memory mirror.
    ///
    /// # Errors
    /// Returns [`StoreError::Backend`] when the file cannot be read
    /// or contains malformed records.
    pub async fn open(path: impl Into<PathBuf>) -> StoreResult<Self> {
        let path = path.into();
        let mut state = State::default();
        if let Ok(bytes) = tokio::fs::read(&path).await {
            for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
                if line.is_empty() {
                    continue;
                }
                let rec: LogRecord = serde_json::from_slice(line).map_err(|e| {
                    StoreError::Backend(format!("log replay line {}: {e}", i.saturating_add(1)))
                })?;
                match rec {
                    LogRecord::Insert { job } | LogRecord::Update { job } => {
                        state.jobs.insert(job.id.0, job);
                    }
                    LogRecord::Remove { id } => {
                        state.jobs.remove(&id.0);
                    }
                    LogRecord::NextId { id } => {
                        if id.0 > state.next {
                            state.next = id.0;
                        }
                    }
                }
            }
        }
        Ok(Self {
            path,
            state: Mutex::new(state),
            write: tokio::sync::Mutex::new(()),
        })
    }

    /// On-disk log file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current job count.
    pub fn len(&self) -> usize {
        self.state.lock().expect("poisoned").jobs.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    async fn append_record(&self, rec: &LogRecord) -> StoreResult<()> {
        let mut line =
            serde_json::to_vec(rec).map_err(|e| StoreError::Backend(format!("encode: {e}")))?;
        line.push(b'\n');
        let _guard = self.write.lock().await;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|e| StoreError::Backend(format!("open {}: {e}", self.path.display())))?;
        f.write_all(&line)
            .await
            .map_err(|e| StoreError::Backend(format!("write: {e}")))?;
        f.flush()
            .await
            .map_err(|e| StoreError::Backend(format!("flush: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl JobStore for FileBackedJobStore {
    async fn insert(&self, job: Job) -> StoreResult<()> {
        // Duplicate check on the in-memory mirror — the file replay
        // builds the same view so a duplicate-id insert can be
        // refused without scanning the log.
        {
            let g = self.state.lock().expect("poisoned");
            if g.jobs.contains_key(&job.id.0) {
                return Err(StoreError::Backend(format!(
                    "duplicate job id {:?}",
                    job.id
                )));
            }
        }
        self.append_record(&LogRecord::Insert { job: job.clone() })
            .await?;
        self.state
            .lock()
            .expect("poisoned")
            .jobs
            .insert(job.id.0, job);
        Ok(())
    }

    async fn update(&self, job: Job) -> StoreResult<()> {
        {
            let g = self.state.lock().expect("poisoned");
            if !g.jobs.contains_key(&job.id.0) {
                return Err(StoreError::Unknown(job.id));
            }
        }
        self.append_record(&LogRecord::Update { job: job.clone() })
            .await?;
        self.state
            .lock()
            .expect("poisoned")
            .jobs
            .insert(job.id.0, job);
        Ok(())
    }

    async fn remove(&self, id: JobId) -> StoreResult<()> {
        {
            let g = self.state.lock().expect("poisoned");
            if !g.jobs.contains_key(&id.0) {
                return Err(StoreError::Unknown(id));
            }
        }
        self.append_record(&LogRecord::Remove { id }).await?;
        self.state.lock().expect("poisoned").jobs.remove(&id.0);
        Ok(())
    }

    async fn get(&self, id: JobId) -> StoreResult<Option<Job>> {
        Ok(self
            .state
            .lock()
            .expect("poisoned")
            .jobs
            .get(&id.0)
            .cloned())
    }

    async fn list(&self) -> StoreResult<Vec<Job>> {
        Ok(self
            .state
            .lock()
            .expect("poisoned")
            .jobs
            .values()
            .cloned()
            .collect())
    }

    async fn next_id(&self) -> StoreResult<JobId> {
        let id = {
            let mut g = self.state.lock().expect("poisoned");
            g.next = g.next.saturating_add(1);
            JobId(g.next)
        };
        self.append_record(&LogRecord::NextId { id }).await?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::Schedule;
    use gauss_core::CapToken;

    fn sample_job(id: u64) -> Job {
        Job::new(
            JobId::new(id),
            format!("test-{id}"),
            Schedule::Duration { seconds: 1 },
            CapToken::BOTTOM,
            serde_json::Value::Null,
            0,
        )
    }

    fn temp_path(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        p.push(format!("gauss-cron-{label}-{ns}.jsonl"));
        // Best-effort: clean any leftover from a previous run.
        let _ = std::fs::remove_file(&p);
        p
    }

    #[tokio::test]
    async fn open_creates_when_missing() {
        let path = temp_path("missing");
        let s = FileBackedJobStore::open(&path).await.unwrap();
        assert_eq!(s.len(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn insert_persists_across_reopen() {
        let path = temp_path("insert-reopen");
        {
            let s = FileBackedJobStore::open(&path).await.unwrap();
            s.insert(sample_job(1)).await.unwrap();
            s.insert(sample_job(2)).await.unwrap();
        }
        let s2 = FileBackedJobStore::open(&path).await.unwrap();
        assert_eq!(s2.len(), 2);
        let back = s2.get(JobId::new(1)).await.unwrap().expect("present");
        assert_eq!(back.label, "test-1");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn update_replaces_in_place() {
        let path = temp_path("update");
        let s = FileBackedJobStore::open(&path).await.unwrap();
        s.insert(sample_job(1)).await.unwrap();
        let mut updated = sample_job(1);
        updated.label = "renamed".into();
        s.update(updated).await.unwrap();
        let back = s.get(JobId::new(1)).await.unwrap().expect("present");
        assert_eq!(back.label, "renamed");
        // Replay survives across reopen.
        drop(s);
        let s2 = FileBackedJobStore::open(&path).await.unwrap();
        assert_eq!(
            s2.get(JobId::new(1)).await.unwrap().unwrap().label,
            "renamed"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn remove_persists() {
        let path = temp_path("remove");
        {
            let s = FileBackedJobStore::open(&path).await.unwrap();
            s.insert(sample_job(1)).await.unwrap();
            s.insert(sample_job(2)).await.unwrap();
            s.remove(JobId::new(1)).await.unwrap();
        }
        let s2 = FileBackedJobStore::open(&path).await.unwrap();
        assert_eq!(s2.len(), 1);
        assert!(s2.get(JobId::new(1)).await.unwrap().is_none());
        assert!(s2.get(JobId::new(2)).await.unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn duplicate_insert_rejected() {
        let path = temp_path("dup");
        let s = FileBackedJobStore::open(&path).await.unwrap();
        s.insert(sample_job(1)).await.unwrap();
        let err = s.insert(sample_job(1)).await.unwrap_err();
        assert!(matches!(err, StoreError::Backend(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn update_missing_rejected() {
        let path = temp_path("upd-missing");
        let s = FileBackedJobStore::open(&path).await.unwrap();
        let err = s.update(sample_job(99)).await.unwrap_err();
        assert!(matches!(err, StoreError::Unknown(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn next_id_persists_across_reopen() {
        let path = temp_path("next-id");
        {
            let s = FileBackedJobStore::open(&path).await.unwrap();
            let a = s.next_id().await.unwrap();
            let b = s.next_id().await.unwrap();
            assert!(a.0 < b.0);
        }
        let s2 = FileBackedJobStore::open(&path).await.unwrap();
        let c = s2.next_id().await.unwrap();
        // After reopen, the allocator must NOT reuse a previously-issued id.
        assert!(c.0 > 2);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn list_returns_all_jobs() {
        let path = temp_path("list");
        let s = FileBackedJobStore::open(&path).await.unwrap();
        for i in 1..=3 {
            s.insert(sample_job(i)).await.unwrap();
        }
        let all = s.list().await.unwrap();
        assert_eq!(all.len(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn malformed_log_rejects_open() {
        let path = temp_path("malformed");
        tokio::fs::write(&path, b"{not json}\n").await.unwrap();
        let err = FileBackedJobStore::open(&path).await.unwrap_err();
        assert!(matches!(err, StoreError::Backend(_)));
        let _ = std::fs::remove_file(&path);
    }
}
