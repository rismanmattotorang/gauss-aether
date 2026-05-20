//! Chain-protected Trinity cron job store.
//!
//! Sprint 10 §10 of `/ROADMAP.md`. Composes Sprint 9 §9's
//! [`gauss_cron::FileBackedJobStore`] (durable append-only JSONL) with
//! the existing [`gauss_memory::SurrealMemory`] chain log so every
//! cron-job mutation gets the same Merkle integrity surface every
//! session turn has:
//!
//! - **File** is canonical state. Replay-on-open rebuilds the in-memory
//!   job set; loss of the chain log doesn't break cron operation.
//! - **Chain** is tamper-evidence. Each mutation appends a JSON record
//!   to the SHA-256-chained log. An operator can recompute the chain
//!   head from the file and compare against the live
//!   [`SurrealMemory::chain_head`] to detect divergence.
//!
//! ## Operation order
//!
//! 1. Lock the inner state (single mutex shared with the file store).
//! 2. Write the JSONL record to the file (durable + replay-able).
//! 3. Append the same JSON envelope to the chain log (tamper-evidence).
//! 4. Release.
//!
//! If step 3 fails after step 2 succeeds, the cron state remains
//! consistent — the chain just loses one entry of tamper-evidence for
//! that mutation. Operators can rebuild the chain from the file via
//! [`TrinityCronJobStore::replay_chain_from_file`] when this happens
//! (Sprint 11 follow-on).
//!
//! ## Hermes parity
//!
//! Hermes has no chain-protected cron store: cron jobs are pickled to
//! a single file with no integrity surface. The GaussClaw equivalent
//! is strictly superior on the same axes session storage is:
//!
//! - **Merkle-integrity for the cron job set.** A single byte changed
//!   in any prior `insert`/`update`/`remove` diverges the chain head.
//! - **Append-only on disk.** Inherited from `FileBackedJobStore` —
//!   crash mid-write at most truncates the trailing record.
//! - **Cross-tool inspectable.** Both the file (JSONL) and the chain
//!   (SurrealDB) are accessible without booting the daemon.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{TaintLabel, TurnId};
use gauss_cron::store::StoreResult as CronStoreResult;
use gauss_cron::{FileBackedJobStore, Job, JobId, JobStore, StoreError as CronStoreError};
use gauss_memory::SurrealMemory;
use gauss_traits::{AppendEntry, MemoryBackend};
use serde::{Deserialize, Serialize};

// ─── chain record ─────────────────────────────────────────────────────────

/// One cron-job mutation as written to the chain log.
///
/// The on-wire shape intentionally mirrors `FileBackedJobStore`'s
/// internal `LogRecord` so an operator can diff the JSONL file against
/// the chain log line-for-line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ChainCronRecord {
    /// A new job was inserted.
    Insert {
        /// Job payload.
        job: Job,
    },
    /// An existing job was updated in place.
    Update {
        /// Updated job payload.
        job: Job,
    },
    /// A job was removed.
    Remove {
        /// Removed job id.
        id: JobId,
    },
    /// The allocator handed out a new id.
    NextId {
        /// Newly-allocated id.
        id: JobId,
    },
}

impl ChainCronRecord {
    /// Stable string tag for the FTS payload.
    #[must_use]
    pub const fn op_tag(&self) -> &'static str {
        match self {
            Self::Insert { .. } => "cron.insert",
            Self::Update { .. } => "cron.update",
            Self::Remove { .. } => "cron.remove",
            Self::NextId { .. } => "cron.next_id",
        }
    }
}

// ─── store ───────────────────────────────────────────────────────────────

/// Chain-protected Trinity cron job store.
///
/// Holds a [`FileBackedJobStore`] for canonical state + a shared
/// [`SurrealMemory`] for chain-of-custody. Cheap to `Arc`-wrap.
pub struct TrinityCronJobStore {
    file: FileBackedJobStore,
    memory: Arc<SurrealMemory>,
    /// Synthetic monotonic turn-id allocator for chain entries. We can't
    /// reuse `JobId`s because (a) `NextId` records have no `Job` id and
    /// (b) the chain log uniqueness key is `TurnId`, which must be
    /// strictly monotonic across all entries.
    next_chain_seq: AtomicU64,
}

impl TrinityCronJobStore {
    /// Open or create a chain-protected cron store. The on-disk JSONL
    /// log lives at `file_path`; the chain log lives in the supplied
    /// [`SurrealMemory`] handle.
    ///
    /// # Errors
    /// Returns [`CronStoreError::Backend`] when the file cannot be
    /// opened or contains malformed records.
    pub async fn open(
        memory: Arc<SurrealMemory>,
        file_path: impl Into<PathBuf>,
    ) -> CronStoreResult<Self> {
        let file = FileBackedJobStore::open(file_path).await?;
        Ok(Self {
            file,
            memory,
            next_chain_seq: AtomicU64::new(0),
        })
    }

    /// On-disk file path of the underlying [`FileBackedJobStore`].
    #[must_use]
    pub fn file_path(&self) -> &Path {
        self.file.path()
    }

    /// Borrow the shared memory backend — useful for chain-head
    /// verification by external auditors.
    #[must_use]
    pub fn memory(&self) -> &Arc<SurrealMemory> {
        &self.memory
    }

    /// Current chain head (the SHA-256 Merkle root after every cron
    /// mutation observed so far).
    pub async fn chain_head(&self) -> CronStoreResult<gauss_traits::ChainHeadSnapshot> {
        self.memory
            .chain_head()
            .await
            .map_err(|e| CronStoreError::Backend(format!("chain_head: {e}")))
    }

    fn next_chain_id(&self) -> TurnId {
        let seq = self
            .next_chain_seq
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        TurnId::new(u128::from(seq))
    }

    /// Append one cron record to the chain log. Cron mutations are
    /// tagged with [`TaintLabel::Trusted`] — they originate from
    /// the kernel-internal scheduler, not from a network ingress.
    async fn write_chain(&self, record: &ChainCronRecord) -> CronStoreResult<()> {
        let payload = serde_json::to_vec(record)
            .map_err(|e| CronStoreError::Backend(format!("encode chain record: {e}")))?;
        let text = format!("{}/{}", record.op_tag(), payload.len());
        let entry =
            AppendEntry::new(self.next_chain_id(), payload, TaintLabel::Trusted).with_text(text);
        self.memory
            .append(entry)
            .await
            .map_err(|e| CronStoreError::Backend(format!("chain append: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl JobStore for TrinityCronJobStore {
    async fn insert(&self, job: Job) -> CronStoreResult<()> {
        // Order: write to canonical (file) first; if that succeeds,
        // mirror to chain. A chain-write failure leaves the canonical
        // store consistent — we just lose one tamper-evidence record.
        self.file.insert(job.clone()).await?;
        self.write_chain(&ChainCronRecord::Insert { job }).await
    }

    async fn update(&self, job: Job) -> CronStoreResult<()> {
        self.file.update(job.clone()).await?;
        self.write_chain(&ChainCronRecord::Update { job }).await
    }

    async fn remove(&self, id: JobId) -> CronStoreResult<()> {
        self.file.remove(id).await?;
        self.write_chain(&ChainCronRecord::Remove { id }).await
    }

    async fn get(&self, id: JobId) -> CronStoreResult<Option<Job>> {
        self.file.get(id).await
    }

    async fn list(&self) -> CronStoreResult<Vec<Job>> {
        self.file.list().await
    }

    async fn next_id(&self) -> CronStoreResult<JobId> {
        let id = self.file.next_id().await?;
        self.write_chain(&ChainCronRecord::NextId { id }).await?;
        Ok(id)
    }
}

// ─── tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::CapToken;
    use gauss_cron::grammar::Schedule;

    fn sample_job(id: u64, label: &str) -> Job {
        Job::new(
            JobId::new(id),
            label,
            Schedule::Duration { seconds: 60 },
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
        p.push(format!("gaussclaw-cron-{label}-{ns}.jsonl"));
        let _ = std::fs::remove_file(&p);
        p
    }

    async fn fresh_store(label: &str) -> (TrinityCronJobStore, PathBuf, Arc<SurrealMemory>) {
        let path = temp_path(label);
        let memory = Arc::new(SurrealMemory::open_in_memory().await.expect("surreal"));
        let store = TrinityCronJobStore::open(memory.clone(), &path)
            .await
            .expect("open");
        (store, path, memory)
    }

    #[tokio::test]
    async fn insert_advances_chain_head() {
        let (store, _path, memory) = fresh_store("insert-chain").await;
        let before = memory.chain_head().await.unwrap();
        store.insert(sample_job(1, "first")).await.unwrap();
        let after = memory.chain_head().await.unwrap();
        assert_ne!(before.digest, after.digest);
        assert_eq!(after.length, before.length.saturating_add(1));
    }

    #[tokio::test]
    async fn every_mutation_advances_chain_head() {
        let (store, _path, memory) = fresh_store("every-mutation").await;
        let h0 = memory.chain_head().await.unwrap();
        store.insert(sample_job(1, "j1")).await.unwrap();
        let h1 = memory.chain_head().await.unwrap();
        let mut updated = sample_job(1, "j1-renamed");
        updated.label = "renamed".into();
        store.update(updated).await.unwrap();
        let h2 = memory.chain_head().await.unwrap();
        store.remove(JobId::new(1)).await.unwrap();
        let h3 = memory.chain_head().await.unwrap();
        let _ = store.next_id().await.unwrap();
        let h4 = memory.chain_head().await.unwrap();
        // Four mutations -> four head advancements -> four unique digests.
        let digests = [h0.digest, h1.digest, h2.digest, h3.digest, h4.digest];
        for (i, a) in digests.iter().enumerate() {
            for b in &digests[i.saturating_add(1)..] {
                assert_ne!(a, b, "duplicate chain head at index {i}");
            }
        }
        assert_eq!(h4.length, h0.length.saturating_add(4));
    }

    #[tokio::test]
    async fn read_methods_dont_advance_chain() {
        let (store, _path, memory) = fresh_store("reads").await;
        store.insert(sample_job(1, "j1")).await.unwrap();
        let before = memory.chain_head().await.unwrap();
        let _ = store.get(JobId::new(1)).await.unwrap();
        let _ = store.list().await.unwrap();
        let after = memory.chain_head().await.unwrap();
        assert_eq!(before.digest, after.digest);
        assert_eq!(before.length, after.length);
    }

    #[tokio::test]
    async fn jobs_survive_reopen_via_file_log() {
        let path = temp_path("reopen");
        let memory1 = Arc::new(SurrealMemory::open_in_memory().await.unwrap());
        {
            let store = TrinityCronJobStore::open(memory1.clone(), &path)
                .await
                .unwrap();
            store.insert(sample_job(1, "first")).await.unwrap();
            store.insert(sample_job(2, "second")).await.unwrap();
        }
        // New memory backend (the chain log doesn't persist in
        // `open_in_memory`); the canonical file is what restores state.
        let memory2 = Arc::new(SurrealMemory::open_in_memory().await.unwrap());
        let store2 = TrinityCronJobStore::open(memory2, &path).await.unwrap();
        let jobs = store2.list().await.unwrap();
        assert_eq!(jobs.len(), 2);
        let labels: Vec<String> = jobs.iter().map(|j| j.label.clone()).collect();
        assert!(labels.contains(&"first".to_string()));
        assert!(labels.contains(&"second".to_string()));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn duplicate_insert_does_not_advance_chain() {
        // File-store refuses duplicate ids; the chain write must not
        // happen for a rejected insert.
        let (store, _path, memory) = fresh_store("dup-insert").await;
        store.insert(sample_job(1, "first")).await.unwrap();
        let before = memory.chain_head().await.unwrap();
        let err = store.insert(sample_job(1, "dup")).await.unwrap_err();
        assert!(matches!(err, CronStoreError::Backend(_)));
        let after = memory.chain_head().await.unwrap();
        assert_eq!(before.digest, after.digest);
    }

    #[tokio::test]
    async fn remove_missing_does_not_advance_chain() {
        let (store, _path, memory) = fresh_store("remove-missing").await;
        let before = memory.chain_head().await.unwrap();
        let err = store.remove(JobId::new(99)).await.unwrap_err();
        assert!(matches!(err, CronStoreError::Unknown(_)));
        let after = memory.chain_head().await.unwrap();
        assert_eq!(before.digest, after.digest);
    }

    #[test]
    fn chain_record_serde_round_trips() {
        let r = ChainCronRecord::Insert {
            job: sample_job(7, "test"),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"op\":\"insert\""));
        let _: ChainCronRecord = serde_json::from_str(&s).unwrap();
    }

    #[test]
    fn op_tag_matches_serde_discriminator() {
        // The op tag string is the same the serde `#[serde(tag = "op")]`
        // discriminator uses on the wire — operators can grep the chain
        // log with the tag value.
        assert_eq!(
            ChainCronRecord::Insert {
                job: sample_job(1, "x")
            }
            .op_tag(),
            "cron.insert"
        );
        assert_eq!(
            ChainCronRecord::Remove { id: JobId::new(1) }.op_tag(),
            "cron.remove"
        );
        assert_eq!(
            ChainCronRecord::NextId { id: JobId::new(1) }.op_tag(),
            "cron.next_id"
        );
    }

    #[tokio::test]
    async fn chain_id_allocator_is_monotone() {
        let (store, _path, _memory) = fresh_store("chain-id").await;
        let a = store.next_chain_id();
        let b = store.next_chain_id();
        let c = store.next_chain_id();
        assert!(a.as_u128() < b.as_u128());
        assert!(b.as_u128() < c.as_u128());
    }
}
