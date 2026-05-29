//! [`Scheduler`] — the tick driver.
//!
//! The scheduler is a thin orchestrator over a [`JobStore`] and a
//! [`Clock`]. It exposes three operations:
//!
//! - [`Scheduler::add`] — insert a new job. Computes the first
//!   `next_fire_at` from the schedule grammar + current clock.
//! - [`Scheduler::tick`] — find every armed job whose
//!   `next_fire_at ≤ now()`, hand each to the caller's
//!   [`Fire`] callback, then advance the job's `next_fire_at`
//!   (cron) or mark it `Completed` (duration / at). The callback
//!   is where the kernel admit gate is re-applied and the actual
//!   payload is dispatched (tool call or agent prompt).
//! - [`Scheduler::cancel`] / [`Scheduler::pause`] / [`resume`] —
//!   lifecycle mutators.
//!
//! The tick body is deterministic given a [`Clock`] + a stable
//! [`JobStore`] iteration order — the conformance suite drives it
//! with a [`crate::FixedClock`] to lock fire ordering across
//! platforms.

use std::sync::Arc;

use thiserror::Error;

use crate::clock::Clock;
use crate::grammar::Schedule;
use crate::job::{Job, JobId, JobStatus};
use crate::store::{JobStore, StoreError};

/// Scheduler-side error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SchedulerError {
    /// Backing store refused.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// The job's payload caps were not satisfied at fire time. The
    /// kernel admit gate refused; the job is marked `Failed` with
    /// this reason recorded.
    #[error("admit refused: required caps 0x{required:016x} not in grant 0x{grant:016x}")]
    AdmitRefused {
        /// Cap bits required by the job's payload.
        required: u64,
        /// Cap bits the kernel grant currently exposes.
        grant: u64,
    },
}

/// One job's fire-time outcome reported by [`Scheduler::tick`].
#[derive(Debug)]
#[non_exhaustive]
pub enum FireOutcome {
    /// Job fired successfully; optional receipt id was returned by
    /// the caller's [`Fire`] callback and stored on the job.
    Fired {
        /// Which job fired.
        id: JobId,
        /// Receipt id the callback returned, if any.
        receipt_id: Option<u64>,
    },
    /// Job's payload caps weren't admitted by the live kernel grant.
    Refused {
        /// Which job was refused.
        id: JobId,
        /// Why (encoded in `SchedulerError::AdmitRefused`'s fields).
        reason: SchedulerError,
    },
}

/// Aggregate report from one tick.
#[derive(Debug, Default)]
pub struct TickReport {
    /// Per-job outcomes, in fire order.
    pub outcomes: Vec<FireOutcome>,
}

impl TickReport {
    /// Number of jobs that fired successfully.
    #[must_use]
    pub fn fired_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, FireOutcome::Fired { .. }))
            .count()
    }

    /// Number of jobs that were refused.
    #[must_use]
    pub fn refused_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, FireOutcome::Refused { .. }))
            .count()
    }
}

/// Fire callback type. The scheduler invokes this for each due job;
/// the caller dispatches the payload through the agent loop, then
/// returns the resulting receipt id (or `None` if the payload didn't
/// produce one).
pub type Fire = dyn Fn(&Job) -> Option<u64> + Send + Sync;

/// The scheduler. Cheap to clone — internal state is `Arc<dyn JobStore>`.
pub struct Scheduler<C: Clock> {
    store: Arc<dyn JobStore>,
    clock: C,
}

impl<C: Clock> Scheduler<C> {
    /// Build a scheduler over a store + clock.
    pub fn new(store: Arc<dyn JobStore>, clock: C) -> Self {
        Self { store, clock }
    }

    /// Borrow the underlying clock.
    pub const fn clock(&self) -> &C {
        &self.clock
    }

    /// Add a job. Reserves the next monotonic id, computes the first
    /// `next_fire_at` from the schedule grammar + current clock,
    /// and persists.
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend failure.
    pub async fn add(&self, mut job: Job) -> Result<Job, SchedulerError> {
        let now = self.clock.now().unix_timestamp();
        let id = self.store.next_id().await?;
        job.id = id;
        job.created_at = now;
        job.next_fire_at = Some(first_fire(&job.schedule, now));
        self.store.insert(job.clone()).await?;
        Ok(job)
    }

    /// Tick the scheduler. For each armed job whose `next_fire_at ≤
    /// now()`, invoke `fire(job)` and (a) advance the job's
    /// `next_fire_at` (Cron) or (b) mark it `Completed`
    /// (Duration/At). Returns a [`TickReport`].
    ///
    /// `grant` is the live kernel grant — the scheduler refuses to
    /// fire a job whose `payload_caps` aren't contained in it.
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] if the store rejects an
    /// update. Per-job admit refusals don't surface here; they land
    /// on the [`TickReport`] as `Refused` entries.
    pub async fn tick<F>(
        &self,
        grant: gauss_core::CapToken,
        fire: F,
    ) -> Result<TickReport, SchedulerError>
    where
        F: Fn(&Job) -> Option<u64>,
    {
        let now = self.clock.now().unix_timestamp();
        let mut report = TickReport::default();
        let mut jobs = self.store.list().await?;
        // Sort by next_fire_at so fire ordering is deterministic
        // across iteration orders.
        jobs.sort_by_key(|j| j.next_fire_at.unwrap_or(i64::MAX));
        for mut job in jobs {
            if job.status != JobStatus::Armed {
                continue;
            }
            let Some(next) = job.next_fire_at else {
                continue;
            };
            if next > now {
                continue;
            }
            // Cap-gate at fire time.
            if !grant.contains(job.payload_caps) {
                let reason = SchedulerError::AdmitRefused {
                    required: job.payload_caps.bits(),
                    grant: grant.bits(),
                };
                job.status = JobStatus::Failed;
                self.store.update(job.clone()).await?;
                report
                    .outcomes
                    .push(FireOutcome::Refused { id: job.id, reason });
                continue;
            }
            let receipt = fire(&job);
            job.fire_count = job.fire_count.saturating_add(1);
            job.last_fired_at = Some(now);
            job.last_receipt_id = receipt;
            match &job.schedule {
                Schedule::Cron { .. } => {
                    job.next_fire_at = Some(next_cron_fire(&job.schedule, now));
                }
                Schedule::Duration { .. } | Schedule::At { .. } => {
                    job.next_fire_at = None;
                    job.status = JobStatus::Completed;
                }
            }
            self.store.update(job.clone()).await?;
            report.outcomes.push(FireOutcome::Fired {
                id: job.id,
                receipt_id: receipt,
            });
        }
        Ok(report)
    }

    /// Pause a job.
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend / unknown-id failure.
    pub async fn pause(&self, id: JobId) -> Result<(), SchedulerError> {
        let mut job = self
            .store
            .get(id)
            .await?
            .ok_or(SchedulerError::Store(StoreError::Unknown(id)))?;
        if job.status == JobStatus::Armed {
            job.status = JobStatus::Paused;
            self.store.update(job).await?;
        }
        Ok(())
    }

    /// Resume a paused job. `next_fire_at` is preserved — an overdue
    /// paused job fires on the next tick.
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend / unknown-id failure.
    pub async fn resume(&self, id: JobId) -> Result<(), SchedulerError> {
        let mut job = self
            .store
            .get(id)
            .await?
            .ok_or(SchedulerError::Store(StoreError::Unknown(id)))?;
        if job.status == JobStatus::Paused {
            job.status = JobStatus::Armed;
            self.store.update(job).await?;
        }
        Ok(())
    }

    /// Cancel a job — removes it from the store.
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend / unknown-id failure.
    pub async fn cancel(&self, id: JobId) -> Result<(), SchedulerError> {
        self.store.remove(id).await?;
        Ok(())
    }

    /// Fetch one job by id. Mirrors [`JobStore::get`] for callers that
    /// only hold a [`Scheduler`].
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend failure.
    pub async fn get(&self, id: JobId) -> Result<Option<Job>, SchedulerError> {
        Ok(self.store.get(id).await?)
    }

    /// Edit a job in place. Pass `None` for any field to leave it
    /// unchanged. Mutating `schedule` recomputes `next_fire_at` from the
    /// new grammar against the current clock — so `cron edit ID --schedule "1h"`
    /// behaves like a fresh `add` for cadence purposes.
    ///
    /// `Completed` and `Failed` jobs are not editable; the caller should
    /// `cancel` and re-`add` instead.
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend / unknown-id failure.
    pub async fn edit(
        &self,
        id: JobId,
        label: Option<String>,
        schedule: Option<Schedule>,
    ) -> Result<Job, SchedulerError> {
        let mut job = self
            .store
            .get(id)
            .await?
            .ok_or(SchedulerError::Store(StoreError::Unknown(id)))?;
        if let Some(l) = label {
            job.label = l;
        }
        if let Some(s) = schedule {
            let now = self.clock.now().unix_timestamp();
            job.next_fire_at = Some(first_fire(&s, now));
            job.schedule = s;
        }
        self.store.update(job.clone()).await?;
        Ok(job)
    }

    /// Fire one job immediately, bypassing its scheduled time but
    /// still honouring the cap-gate. The job's `fire_count` and
    /// `last_fired_at` advance as if the tick fired naturally;
    /// `next_fire_at` is recomputed (cron) or the job is marked
    /// `Completed` (duration / at).
    ///
    /// Returns [`FireOutcome::Refused`] when the live `grant` doesn't
    /// contain the job's `payload_caps`; the job's status becomes
    /// `Failed` in that case (same semantics as a refused tick fire).
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend / unknown-id failure.
    pub async fn run_now<F>(
        &self,
        id: JobId,
        grant: gauss_core::CapToken,
        fire: F,
    ) -> Result<FireOutcome, SchedulerError>
    where
        F: Fn(&Job) -> Option<u64>,
    {
        let mut job = self
            .store
            .get(id)
            .await?
            .ok_or(SchedulerError::Store(StoreError::Unknown(id)))?;
        if !grant.contains(job.payload_caps) {
            let reason = SchedulerError::AdmitRefused {
                required: job.payload_caps.bits(),
                grant: grant.bits(),
            };
            job.status = JobStatus::Failed;
            self.store.update(job.clone()).await?;
            return Ok(FireOutcome::Refused { id: job.id, reason });
        }
        let now = self.clock.now().unix_timestamp();
        let receipt = fire(&job);
        job.fire_count = job.fire_count.saturating_add(1);
        job.last_fired_at = Some(now);
        job.last_receipt_id = receipt;
        match &job.schedule {
            Schedule::Cron { .. } => {
                job.next_fire_at = Some(next_cron_fire(&job.schedule, now));
                // Force back to Armed if it was Paused/Failed — explicit
                // `run` rearms a sleeping job.
                if matches!(job.status, JobStatus::Failed | JobStatus::Paused) {
                    job.status = JobStatus::Armed;
                }
            }
            Schedule::Duration { .. } | Schedule::At { .. } => {
                job.next_fire_at = None;
                job.status = JobStatus::Completed;
            }
        }
        self.store.update(job.clone()).await?;
        Ok(FireOutcome::Fired {
            id: job.id,
            receipt_id: receipt,
        })
    }

    /// List every job in the store.
    ///
    /// # Errors
    /// Returns [`SchedulerError::Store`] on backend failure.
    pub async fn list(&self) -> Result<Vec<Job>, SchedulerError> {
        Ok(self.store.list().await?)
    }
}

/// Compute the first fire time for a freshly-added job.
fn first_fire(schedule: &Schedule, now: i64) -> i64 {
    match schedule {
        Schedule::Duration { seconds } => now.saturating_add(*seconds),
        Schedule::At { unix_seconds } => *unix_seconds,
        Schedule::Cron { .. } => next_cron_fire(schedule, now),
    }
}

/// Compute the next cron-grammar fire time strictly after `after`.
/// Walks one minute at a time up to 366 days; jobs whose grammar
/// doesn't match anything in that window fire never (returns
/// `i64::MAX`), which the scheduler treats as "armed but unfireable".
fn next_cron_fire(schedule: &Schedule, after: i64) -> i64 {
    let Schedule::Cron {
        minute,
        hour,
        dom,
        month,
        dow,
        ..
    } = schedule
    else {
        return after;
    };
    // Walk minute by minute up to 366 days. Compute UTC y/m/d/h/m for
    // each candidate; check field matches.
    let max_minutes: i64 = 366 * 24 * 60;
    for delta in 1..=max_minutes {
        let candidate = after.saturating_add(delta * 60);
        // Drop sub-minute precision so candidates align on minute
        // boundaries (cron semantic).
        let candidate = (candidate / 60) * 60;
        let Ok(t) = time::OffsetDateTime::from_unix_timestamp(candidate) else {
            continue;
        };
        let cm = u8::try_from(t.minute()).unwrap_or(0);
        let ch = u8::try_from(t.hour()).unwrap_or(0);
        let cdom = u8::try_from(t.day()).unwrap_or(1);
        let cmonth = u8::from(t.month());
        let cdow = u8::from(t.weekday().number_days_from_sunday());
        if minute.matches(cm)
            && hour.matches(ch)
            && dom.matches(cdom)
            && month.matches(cmonth)
            && dow.matches(cdow)
        {
            return candidate;
        }
    }
    i64::MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FixedClock;
    use crate::grammar::Schedule;
    use crate::store::InMemoryJobStore;
    use gauss_core::CapToken;

    fn sample_duration_job(secs: i64) -> Job {
        Job::new(
            JobId::new(0), // overwritten by scheduler
            "sample",
            Schedule::Duration { seconds: secs },
            CapToken::BOTTOM,
            serde_json::Value::Null,
            0, // overwritten
        )
    }

    #[tokio::test]
    async fn add_assigns_id_and_first_fire() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        s.clock().advance(100);
        let j = s.add(sample_duration_job(60)).await.unwrap();
        assert_eq!(j.id.0, 1);
        assert_eq!(j.created_at, 100);
        assert_eq!(j.next_fire_at, Some(160));
    }

    #[tokio::test]
    async fn tick_fires_due_duration_job_once() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let _ = s.add(sample_duration_job(60)).await.unwrap();
        // Not yet due.
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 0);
        s.clock().advance(60);
        let r = s.tick(CapToken::TOP, |_| Some(42)).await.unwrap();
        assert_eq!(r.fired_count(), 1);
        // Second tick after fire: nothing left.
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 0);
        // Job is now Completed.
        let all = s.list().await.unwrap();
        assert_eq!(all[0].status, JobStatus::Completed);
        assert_eq!(all[0].fire_count, 1);
        assert_eq!(all[0].last_receipt_id, Some(42));
    }

    #[tokio::test]
    async fn tick_refuses_when_grant_misses_payload_cap() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let mut j = sample_duration_job(60);
        j.payload_caps = CapToken::CRON_SCHEDULE | CapToken::NETWORK_GET;
        let _ = s.add(j).await.unwrap();
        s.clock().advance(60);
        // Grant only CRON_SCHEDULE, NOT NETWORK_GET.
        let r = s.tick(CapToken::CRON_SCHEDULE, |_| Some(1)).await.unwrap();
        assert_eq!(r.fired_count(), 0);
        assert_eq!(r.refused_count(), 1);
        let all = s.list().await.unwrap();
        assert_eq!(all[0].status, JobStatus::Failed);
    }

    #[tokio::test]
    async fn pause_then_resume_preserves_next_fire_at() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let j = s.add(sample_duration_job(60)).await.unwrap();
        s.pause(j.id).await.unwrap();
        s.clock().advance(120);
        // Paused job doesn't fire even when overdue.
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 0);
        s.resume(j.id).await.unwrap();
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 1);
    }

    #[tokio::test]
    async fn cancel_removes_job() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let j = s.add(sample_duration_job(60)).await.unwrap();
        s.cancel(j.id).await.unwrap();
        assert!(s.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cron_job_reschedules_on_fire() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let mut j = sample_duration_job(0);
        j.schedule = crate::parse_schedule("*/15 * * * *").unwrap();
        let added = s.add(j).await.unwrap();
        let first_fire = added.next_fire_at.expect("computed");
        s.clock()
            .set(time::OffsetDateTime::from_unix_timestamp(first_fire).unwrap());
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 1);
        let all = s.list().await.unwrap();
        // Still armed, with a later next_fire_at.
        assert_eq!(all[0].status, JobStatus::Armed);
        assert!(all[0].next_fire_at.unwrap() > first_fire);
        // Second fire 15 minutes later.
        assert_eq!(all[0].next_fire_at.unwrap(), first_fire + 15 * 60);
    }

    #[tokio::test]
    async fn at_job_fires_once_at_absolute_time() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let mut j = sample_duration_job(0);
        j.schedule = Schedule::At { unix_seconds: 100 };
        let _ = s.add(j).await.unwrap();
        s.clock().advance(99);
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 0);
        s.clock().advance(1);
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 1);
        // Completed.
        let all = s.list().await.unwrap();
        assert_eq!(all[0].status, JobStatus::Completed);
    }

    #[tokio::test]
    async fn run_now_fires_a_duration_job_before_its_time() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let j = s.add(sample_duration_job(3600)).await.unwrap(); // 1 h from now
                                                                 // Not yet due — a normal tick would do nothing.
        let r = s.tick(CapToken::TOP, |_| None).await.unwrap();
        assert_eq!(r.fired_count(), 0);
        // run_now fires anyway.
        let out = s.run_now(j.id, CapToken::TOP, |_| Some(7)).await.unwrap();
        assert!(matches!(
            out,
            FireOutcome::Fired {
                receipt_id: Some(7),
                ..
            }
        ));
        let back = s.list().await.unwrap();
        assert_eq!(back[0].status, JobStatus::Completed);
        assert_eq!(back[0].fire_count, 1);
        assert_eq!(back[0].last_receipt_id, Some(7));
    }

    #[tokio::test]
    async fn run_now_refuses_when_grant_misses_payload_cap() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let mut j = sample_duration_job(60);
        j.payload_caps = CapToken::NETWORK_GET;
        let added = s.add(j).await.unwrap();
        // Empty grant.
        let out = s
            .run_now(added.id, CapToken::BOTTOM, |_| Some(1))
            .await
            .unwrap();
        assert!(matches!(out, FireOutcome::Refused { .. }));
        let back = s.list().await.unwrap();
        assert_eq!(back[0].status, JobStatus::Failed);
        assert_eq!(back[0].fire_count, 0);
    }

    #[tokio::test]
    async fn run_now_on_unknown_id_returns_store_error() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let err = s
            .run_now(JobId::new(99), CapToken::TOP, |_| None)
            .await
            .unwrap_err();
        assert!(matches!(err, SchedulerError::Store(StoreError::Unknown(_))));
    }

    #[tokio::test]
    async fn edit_label_only_preserves_schedule_and_next_fire_at() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let added = s.add(sample_duration_job(60)).await.unwrap();
        let before_next = added.next_fire_at;
        let after = s
            .edit(added.id, Some("renamed".into()), None)
            .await
            .unwrap();
        assert_eq!(after.label, "renamed");
        assert_eq!(after.next_fire_at, before_next);
    }

    #[tokio::test]
    async fn edit_schedule_recomputes_next_fire_at() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let added = s.add(sample_duration_job(60)).await.unwrap();
        let before_next = added.next_fire_at.unwrap();
        s.clock().advance(100);
        let new_schedule = Schedule::Duration { seconds: 300 };
        let after = s.edit(added.id, None, Some(new_schedule)).await.unwrap();
        // Now-time is 100, new duration is 300 → next_fire_at == 400.
        assert_eq!(after.next_fire_at, Some(400));
        assert_ne!(after.next_fire_at.unwrap(), before_next);
    }

    #[tokio::test]
    async fn deterministic_fire_order_by_next_fire_at() {
        let clock = FixedClock::epoch();
        let s = Scheduler::new(Arc::new(InMemoryJobStore::new()), clock);
        let _ = s.add(sample_duration_job(120)).await.unwrap();
        let _ = s.add(sample_duration_job(60)).await.unwrap();
        let _ = s.add(sample_duration_job(180)).await.unwrap();
        s.clock().advance(180);
        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let order_clone = order.clone();
        let _ = s
            .tick(CapToken::TOP, move |job| {
                order_clone.lock().unwrap().push(job.id.0);
                None
            })
            .await
            .unwrap();
        // Expected order: job 2 (60s) → job 1 (120s) → job 3 (180s).
        assert_eq!(*order.lock().unwrap(), vec![2, 1, 3]);
    }

    // ─── Sprint 13: driver pattern test ─────────────────────────────────

    /// Mirrors the `spawn_cron_tick_driver` pattern in `gaussclaw-bin`:
    /// the fire closure spawns an async task and returns `None`. We
    /// drive several ticks and assert that:
    ///   1. The scheduler marks each due job as fired (fire_count
    ///      advances).
    ///   2. The spawned tasks all run to completion before the test
    ///      ends.
    /// This locks in the contract the bin relies on so a future
    /// scheduler change that breaks the spawn-and-don't-wait pattern
    /// is caught here.
    #[tokio::test]
    async fn spawn_from_fire_closure_pattern_is_safe() {
        let clock = FixedClock::epoch();
        let s = Arc::new(Scheduler::new(Arc::new(InMemoryJobStore::new()), clock));
        let _ = s.add(sample_duration_job(60)).await.unwrap();
        let _ = s.add(sample_duration_job(60)).await.unwrap();
        let _ = s.add(sample_duration_job(60)).await.unwrap();
        s.clock().advance(60);

        let work_done = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = work_done.clone();
        let r = s
            .tick(CapToken::TOP, move |_job| {
                let c = counter.clone();
                tokio::spawn(async move {
                    // Simulated agent dispatch: small async delay.
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                });
                None
            })
            .await
            .unwrap();
        assert_eq!(r.fired_count(), 3);

        // Wait for the spawned tasks to settle. In production the driver
        // doesn't wait — that's fine, the agent loop owns its own
        // backpressure. Here we wait so we can assert.
        for _ in 0..20 {
            if work_done.load(std::sync::atomic::Ordering::SeqCst) == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            work_done.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "all spawned dispatches must complete"
        );
    }
}
