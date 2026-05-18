//! [`CronJobTool`] — let the agent schedule its own future runs.
//!
//! When the model calls `cronjob({"action": "add", "schedule":
//! "*/15 * * * *", "label": "…", "payload": {…}})`, the tool reaches
//! through [`gauss_cron::Scheduler`] and persists the job. Subsequent
//! `list`, `pause`, `resume`, `cancel` actions surface the same
//! scheduler surface to the model.
//!
//! Gating:
//!
//! - The **tool itself** declares `cap:cron:schedule`. The kernel
//!   admit gate refuses the call unless the session grant carries
//!   that cap.
//! - The **scheduled payload** can declare its own `payload_caps`.
//!   The cron tick re-checks those at fire time against the live
//!   kernel grant — a sub-agent that lost a cap between scheduling
//!   and firing can't fire the job.
//! - The default declass map refuses `cron:schedule` under
//!   `Adversarial` taint, so a web-fetched message can't quietly
//!   plant a daemon-plane job.

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_cron::{parse_schedule, Job, JobId, Scheduler, SystemClock};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "cronjob"
description = "Schedule, pause, resume, or cancel a future job. Useful for follow-ups, periodic tasks, and deferred work."
usage       = "Args: {action: 'add'|'list'|'pause'|'resume'|'cancel', ...}. Add accepts {schedule, label, payload, payload_caps}. Schedule grammar: '30m', '*/15 * * * *', or '2026-05-20T14:30:00Z'."
caps        = ["cron:schedule"]
taint       = "user"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 4096

[schema]
type = "object"
"#;

/// Cron tool. Wraps a shared [`Scheduler`] so multiple sessions can
/// schedule jobs into the same backing store.
pub struct CronJobTool {
    manifest: ToolManifest,
    scheduler: Arc<Scheduler<SystemClock>>,
}

impl CronJobTool {
    /// Build a tool over a caller-supplied scheduler. Production
    /// deployments wire one scheduler per process (or per cluster
    /// node) so cron state survives across sessions.
    ///
    /// # Panics
    /// Panics if the embedded manifest TOML fails to parse — build-
    /// time only.
    #[must_use]
    pub fn new(scheduler: Arc<Scheduler<SystemClock>>) -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("cronjob".into()))
            .expect("embedded skill compiles");
        Self {
            manifest,
            scheduler,
        }
    }

    /// Convenience: build a tool with a fresh in-memory scheduler.
    /// Used by the standalone binary's first-run path; production
    /// deployments share a single scheduler.
    #[must_use]
    pub fn with_in_memory_store() -> Self {
        let store = Arc::new(gauss_cron::InMemoryJobStore::new());
        let scheduler = Arc::new(Scheduler::new(store, SystemClock));
        Self::new(scheduler)
    }
}

#[async_trait]
impl ToolTrait for CronJobTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `action`".into()))?;
        match action {
            "add" => self.do_add(&args).await,
            "list" => self.do_list().await,
            "pause" => self.do_pause(&args).await,
            "resume" => self.do_resume(&args).await,
            "cancel" => self.do_cancel(&args).await,
            other => Err(GaussError::Internal(format!(
                "unknown cronjob action `{other}` (try add/list/pause/resume/cancel)"
            ))),
        }
    }
}

impl CronJobTool {
    async fn do_add(&self, args: &serde_json::Value) -> GaussResult<serde_json::Value> {
        let schedule_str = args
            .get("schedule")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `schedule`".into()))?;
        let schedule = parse_schedule(schedule_str)
            .map_err(|e| GaussError::Internal(format!("schedule grammar: {e}")))?;
        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("(unlabeled cronjob)");
        let payload = args
            .get("payload")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        // Optional payload caps as a bitmask integer. Default to the
        // empty cap (no kernel access at fire time) — that's the
        // safest default; the operator can widen explicitly.
        let payload_caps_bits = args.get("payload_caps").and_then(serde_json::Value::as_u64);
        let payload_caps = payload_caps_bits.map_or(CapToken::BOTTOM, CapToken::from_bits);
        let job = Job::new(
            JobId::new(0), // overwritten by scheduler::add
            label,
            schedule,
            payload_caps,
            payload,
            0, // overwritten
        );
        let inserted = self
            .scheduler
            .add(job)
            .await
            .map_err(|e| GaussError::Internal(format!("scheduler add: {e}")))?;
        Ok(serde_json::json!({
            "kind":         "cronjob_added",
            "id":           inserted.id.0,
            "label":        inserted.label,
            "next_fire_at": inserted.next_fire_at,
        }))
    }

    async fn do_list(&self) -> GaussResult<serde_json::Value> {
        let jobs = self
            .scheduler
            .list()
            .await
            .map_err(|e| GaussError::Internal(format!("scheduler list: {e}")))?;
        let rows: Vec<serde_json::Value> = jobs
            .into_iter()
            .map(|j| {
                serde_json::json!({
                    "id":            j.id.0,
                    "label":         j.label,
                    "schedule":      j.schedule,
                    "status":        j.status,
                    "next_fire_at":  j.next_fire_at,
                    "last_fired_at": j.last_fired_at,
                    "fire_count":    j.fire_count,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "kind": "cronjob_list",
            "jobs": rows,
        }))
    }

    async fn do_pause(&self, args: &serde_json::Value) -> GaussResult<serde_json::Value> {
        let id = parse_id(args)?;
        self.scheduler
            .pause(id)
            .await
            .map_err(|e| GaussError::Internal(format!("scheduler pause: {e}")))?;
        Ok(serde_json::json!({ "kind": "cronjob_paused", "id": id.0 }))
    }

    async fn do_resume(&self, args: &serde_json::Value) -> GaussResult<serde_json::Value> {
        let id = parse_id(args)?;
        self.scheduler
            .resume(id)
            .await
            .map_err(|e| GaussError::Internal(format!("scheduler resume: {e}")))?;
        Ok(serde_json::json!({ "kind": "cronjob_resumed", "id": id.0 }))
    }

    async fn do_cancel(&self, args: &serde_json::Value) -> GaussResult<serde_json::Value> {
        let id = parse_id(args)?;
        self.scheduler
            .cancel(id)
            .await
            .map_err(|e| GaussError::Internal(format!("scheduler cancel: {e}")))?;
        Ok(serde_json::json!({ "kind": "cronjob_cancelled", "id": id.0 }))
    }
}

fn parse_id(args: &serde_json::Value) -> GaussResult<JobId> {
    let n = args
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| GaussError::Internal("missing uint field `id`".into()))?;
    Ok(JobId::new(n))
}

// Re-export `JobStatus` for callers that need to pattern-match the
// list response.
pub use gauss_cron::JobStatus as Status;
// Re-export `JobStore` so a host can plug in its own backend.
pub use gauss_cron::JobStore as Store;

/// Cap surfaced by [`CronJobTool::manifest`].
#[must_use]
pub const fn cron_schedule_cap() -> CapToken {
    CapToken::CRON_SCHEDULE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_then_list_round_trip() {
        let t = CronJobTool::with_in_memory_store();
        t.invoke_raw(serde_json::json!({
            "action":   "add",
            "schedule": "30m",
            "label":    "cleanup",
            "payload":  {"tool": "shell", "args": {"cmd": "echo hi"}},
        }))
        .await
        .unwrap();
        let list = t
            .invoke_raw(serde_json::json!({"action": "list"}))
            .await
            .unwrap();
        assert_eq!(list["kind"], "cronjob_list");
        let jobs = list["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["label"], "cleanup");
    }

    #[tokio::test]
    async fn pause_resume_cancel_round_trip() {
        let t = CronJobTool::with_in_memory_store();
        let added = t
            .invoke_raw(serde_json::json!({
                "action":   "add",
                "schedule": "30m",
                "label":    "x",
            }))
            .await
            .unwrap();
        let id = added["id"].as_u64().unwrap();
        let pause = t
            .invoke_raw(serde_json::json!({"action": "pause", "id": id}))
            .await
            .unwrap();
        assert_eq!(pause["kind"], "cronjob_paused");
        let resume = t
            .invoke_raw(serde_json::json!({"action": "resume", "id": id}))
            .await
            .unwrap();
        assert_eq!(resume["kind"], "cronjob_resumed");
        let cancel = t
            .invoke_raw(serde_json::json!({"action": "cancel", "id": id}))
            .await
            .unwrap();
        assert_eq!(cancel["kind"], "cronjob_cancelled");
        // Job is gone.
        let list = t
            .invoke_raw(serde_json::json!({"action": "list"}))
            .await
            .unwrap();
        assert_eq!(list["jobs"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn unknown_action_rejected() {
        let t = CronJobTool::with_in_memory_store();
        let err = t
            .invoke_raw(serde_json::json!({"action": "fnord"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn missing_action_rejected() {
        let t = CronJobTool::with_in_memory_store();
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn bad_schedule_rejected() {
        let t = CronJobTool::with_in_memory_store();
        let err = t
            .invoke_raw(serde_json::json!({
                "action":   "add",
                "schedule": "not a schedule",
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_cron_schedule_cap() {
        let t = CronJobTool::with_in_memory_store();
        assert_eq!(t.manifest().cap_required, cron_schedule_cap());
    }
}
