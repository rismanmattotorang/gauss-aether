//! Job records — the persistent shape stored by [`crate::JobStore`].

use gauss_core::CapToken;
use serde::{Deserialize, Serialize};

use crate::grammar::Schedule;

/// Stable monotonic job identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub u64);

impl JobId {
    /// Build a `JobId` from a `u64`.
    #[must_use]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }
}

/// Lifecycle state of a scheduled job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum JobStatus {
    /// Job is armed and will fire when its `next_fire_at` is reached.
    Armed,
    /// Job is suspended; `next_fire_at` is preserved so resume
    /// restores the original cadence.
    Paused,
    /// Job's `Duration` / `At` form has fired; nothing more will run.
    Completed,
    /// Last fire returned an error. Operator can `cron resume` to
    /// rearm or `cron remove` to drop.
    Failed,
}

/// Persistent job record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Job {
    /// Stable id.
    pub id: JobId,
    /// Free-text operator-supplied label.
    pub label: String,
    /// Schedule grammar (cf. [`Schedule`]).
    pub schedule: Schedule,
    /// Capabilities the job's payload requires at fire time. The
    /// kernel admit gate re-checks these on every tick — a cap revoked
    /// after scheduling refuses the fire.
    pub payload_caps: CapToken,
    /// Payload body. Either a Skill Manifest tool call (`{tool, args}`)
    /// or a free-form text prompt the agent loop should run.
    pub payload: serde_json::Value,
    /// Current lifecycle state.
    pub status: JobStatus,
    /// UNIX seconds when this job was created.
    pub created_at: i64,
    /// UNIX seconds when this job should next fire. `None` for
    /// `Completed` jobs.
    pub next_fire_at: Option<i64>,
    /// UNIX seconds of the most recent fire. `None` until the first
    /// fire occurs.
    pub last_fired_at: Option<i64>,
    /// Number of fires that have completed.
    pub fire_count: u64,
    /// Receipt id of the most recent fire (Trinity-store row id).
    pub last_receipt_id: Option<u64>,
}

impl Job {
    /// Build a fresh armed job. `next_fire_at` is left `None`; the
    /// scheduler computes it on `add` via the schedule's grammar.
    #[must_use]
    pub fn new(
        id: JobId,
        label: impl Into<String>,
        schedule: Schedule,
        payload_caps: CapToken,
        payload: serde_json::Value,
        created_at: i64,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            schedule,
            payload_caps,
            payload,
            status: JobStatus::Armed,
            created_at,
            next_fire_at: None,
            last_fired_at: None,
            fire_count: 0,
            last_receipt_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_serde_round_trips() {
        let j = Job::new(
            JobId::new(7),
            "weekly cleanup",
            Schedule::Duration { seconds: 86_400 },
            CapToken::NETWORK_GET,
            serde_json::json!({"tool": "shell", "args": {"cmd": "echo hi"}}),
            1_700_000_000,
        );
        let s = serde_json::to_string(&j).unwrap();
        let back: Job = serde_json::from_str(&s).unwrap();
        assert_eq!(j, back);
    }

    #[test]
    fn job_status_is_armed_on_creation() {
        let j = Job::new(
            JobId::new(1),
            "x",
            Schedule::Duration { seconds: 1 },
            CapToken::BOTTOM,
            serde_json::Value::Null,
            0,
        );
        assert_eq!(j.status, JobStatus::Armed);
        assert!(j.next_fire_at.is_none());
        assert!(j.last_fired_at.is_none());
        assert_eq!(j.fire_count, 0);
    }
}
