//! Pluggable job store. The in-memory impl is used by tests and the
//! standalone CLI; production wires the SurrealDB-backed Trinity
//! store via `gaussclaw-store::cron_jobs` (Sprint 5 follow-on).

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use thiserror::Error;

use crate::job::{Job, JobId};

/// Store-side error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreError {
    /// Job not found.
    #[error("unknown job: {0:?}")]
    Unknown(JobId),
    /// Backend I/O / transport failure.
    #[error("backend: {0}")]
    Backend(String),
}

/// Convenience alias.
pub type StoreResult<T> = Result<T, StoreError>;

/// Pluggable persistence.
#[async_trait]
pub trait JobStore: Send + Sync {
    /// Insert a new job. Returns the inserted [`Job`] for echo /
    /// receipt purposes.
    async fn insert(&self, job: Job) -> StoreResult<()>;
    /// Update an existing job (mutated in place by the scheduler on
    /// every fire).
    async fn update(&self, job: Job) -> StoreResult<()>;
    /// Remove a job.
    async fn remove(&self, id: JobId) -> StoreResult<()>;
    /// Fetch one job.
    async fn get(&self, id: JobId) -> StoreResult<Option<Job>>;
    /// List every job in insertion order.
    async fn list(&self) -> StoreResult<Vec<Job>>;
    /// Reserve the next monotonic id. The implementation is the
    /// authoritative allocator; `Scheduler::add` never picks ids
    /// itself.
    async fn next_id(&self) -> StoreResult<JobId>;
}

#[async_trait]
impl<T: ?Sized + JobStore> JobStore for std::sync::Arc<T> {
    async fn insert(&self, job: Job) -> StoreResult<()> {
        (**self).insert(job).await
    }
    async fn update(&self, job: Job) -> StoreResult<()> {
        (**self).update(job).await
    }
    async fn remove(&self, id: JobId) -> StoreResult<()> {
        (**self).remove(id).await
    }
    async fn get(&self, id: JobId) -> StoreResult<Option<Job>> {
        (**self).get(id).await
    }
    async fn list(&self) -> StoreResult<Vec<Job>> {
        (**self).list().await
    }
    async fn next_id(&self) -> StoreResult<JobId> {
        (**self).next_id().await
    }
}

/// Test / CLI in-memory backend.
#[derive(Debug, Default)]
pub struct InMemoryJobStore {
    inner: Mutex<Inner>,
}

#[derive(Default, Debug)]
struct Inner {
    jobs: BTreeMap<u64, Job>,
    next: u64,
}

impl InMemoryJobStore {
    /// Build a fresh empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored jobs.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("poisoned").jobs.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl JobStore for InMemoryJobStore {
    async fn insert(&self, job: Job) -> StoreResult<()> {
        let mut g = self.inner.lock().expect("poisoned");
        if g.jobs.contains_key(&job.id.0) {
            return Err(StoreError::Backend(format!(
                "duplicate job id {:?}",
                job.id
            )));
        }
        g.jobs.insert(job.id.0, job);
        Ok(())
    }
    async fn update(&self, job: Job) -> StoreResult<()> {
        let mut g = self.inner.lock().expect("poisoned");
        if !g.jobs.contains_key(&job.id.0) {
            return Err(StoreError::Unknown(job.id));
        }
        g.jobs.insert(job.id.0, job);
        Ok(())
    }
    async fn remove(&self, id: JobId) -> StoreResult<()> {
        let mut g = self.inner.lock().expect("poisoned");
        if g.jobs.remove(&id.0).is_none() {
            return Err(StoreError::Unknown(id));
        }
        Ok(())
    }
    async fn get(&self, id: JobId) -> StoreResult<Option<Job>> {
        Ok(self
            .inner
            .lock()
            .expect("poisoned")
            .jobs
            .get(&id.0)
            .cloned())
    }
    async fn list(&self) -> StoreResult<Vec<Job>> {
        Ok(self
            .inner
            .lock()
            .expect("poisoned")
            .jobs
            .values()
            .cloned()
            .collect())
    }
    async fn next_id(&self) -> StoreResult<JobId> {
        let mut g = self.inner.lock().expect("poisoned");
        g.next = g.next.saturating_add(1);
        Ok(JobId(g.next))
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

    #[tokio::test]
    async fn insert_get_round_trips() {
        let s = InMemoryJobStore::new();
        s.insert(sample_job(1)).await.unwrap();
        let back = s.get(JobId::new(1)).await.unwrap().expect("present");
        assert_eq!(back.label, "test-1");
    }

    #[tokio::test]
    async fn duplicate_insert_rejected() {
        let s = InMemoryJobStore::new();
        s.insert(sample_job(1)).await.unwrap();
        let err = s.insert(sample_job(1)).await.unwrap_err();
        assert!(matches!(err, StoreError::Backend(_)));
    }

    #[tokio::test]
    async fn update_missing_rejected() {
        let s = InMemoryJobStore::new();
        let err = s.update(sample_job(99)).await.unwrap_err();
        assert!(matches!(err, StoreError::Unknown(_)));
    }

    #[tokio::test]
    async fn remove_drops_job() {
        let s = InMemoryJobStore::new();
        s.insert(sample_job(1)).await.unwrap();
        s.remove(JobId::new(1)).await.unwrap();
        assert!(s.get(JobId::new(1)).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn next_id_is_monotone() {
        let s = InMemoryJobStore::new();
        let a = s.next_id().await.unwrap();
        let b = s.next_id().await.unwrap();
        let c = s.next_id().await.unwrap();
        assert!(a.0 < b.0 && b.0 < c.0);
    }

    #[tokio::test]
    async fn list_returns_all_jobs() {
        let s = InMemoryJobStore::new();
        for i in 1..=3 {
            s.insert(sample_job(i)).await.unwrap();
        }
        let all = s.list().await.unwrap();
        assert_eq!(all.len(), 3);
    }
}
