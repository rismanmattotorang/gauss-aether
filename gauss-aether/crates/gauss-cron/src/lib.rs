//! `gauss-cron` — 60-second tick scheduler for in-agent scheduled jobs.
//!
//! Sprint 5 §1 of `/ROADMAP.md`. Hermes's `cron` subsystem persisted
//! jobs in a Python-pickled file with no cap-gate and no
//! tamper-evidence. GaussClaw's scheduler:
//!
//! 1. Parses schedules from three grammars: **duration** (`30m`,
//!    `2h15m`, `1d`), **cron expression** (`*/15 * * * *`), and
//!    **ISO 8601 timestamp** (`2026-05-20T14:30:00Z`).
//! 2. Persists the job set through a [`JobStore`] trait — production
//!    deployments wire it into the Trinity store's `cron_jobs` table
//!    so every job mutation joins the chain-protected receipt log.
//! 3. Drives a deterministic 60-second tick that fires every job
//!    whose next-fire-at is `≤ now()`. The tick is wall-clock-aware
//!    *and* monotone — pause/resume preserves "next fire at" rather
//!    than recomputing from now, so a paused job that's overdue
//!    fires immediately on resume (operator's choice).
//! 4. Cap-gates every job: the kernel admit gate checks
//!    [`gauss_core::CapToken::CRON_SCHEDULE`] on `add`, and re-checks
//!    the *payload's* declared caps at fire time. A sub-agent that
//!    lost a cap between scheduling and firing can't fire the job.
//!
//! ## Hermes-superiority axes (verified by tests in this crate)
//!
//! - **Grammar.** Three input forms accepted out of the box; Hermes
//!   only accepts cron expressions.
//! - **Cap-at-fire.** Each fire re-checks the payload's caps against
//!   the live kernel grant. Hermes fires unconditionally.
//! - **Deterministic tick.** [`Scheduler::tick`] is pure given a
//!   [`Clock`] — the conformance suite drives it with a `FixedClock`
//!   to lock fire ordering. Hermes uses the wall clock directly.
//! - **Pluggable store.** [`InMemoryJobStore`] for tests, a SurrealDB
//!   backend ships in `gaussclaw-store` once Sprint 5 §1.3 lands.
//!   Hermes hard-codes pickle.
//! - **Receipt-aware.** Every job carries a `last_receipt_id` after
//!   fire so an operator can replay the chain entry that produced
//!   the result. Hermes has no audit linkage.

#![allow(
    clippy::doc_markdown,
    clippy::missing_docs_in_private_items,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::similar_names,
    clippy::useless_conversion,
    clippy::arithmetic_side_effects,
    clippy::too_long_first_doc_paragraph,
    clippy::significant_drop_tightening,
    clippy::or_fun_call,
    clippy::needless_continue
)]
#![allow(rustdoc::broken_intra_doc_links)]

pub mod clock;
pub mod grammar;
pub mod job;
pub mod scheduler;
pub mod store;

pub use clock::{Clock, FixedClock, SystemClock};
pub use grammar::{parse_schedule, Schedule, ScheduleParseError};
pub use job::{Job, JobId, JobStatus};
pub use scheduler::{FireOutcome, Scheduler, SchedulerError, TickReport};
pub use store::{InMemoryJobStore, JobStore, StoreError};
