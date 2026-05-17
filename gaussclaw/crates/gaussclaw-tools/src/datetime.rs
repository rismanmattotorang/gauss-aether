//! [`DatetimeTool`] — current time, ISO 8601 formatting, deterministic parsing.
//!
//! Pure-compute, no caps. The Hermes upstream has no first-class datetime
//! tool; agents end up either invoking shell `date` (a sandbox escape risk)
//! or asking the LLM to guess the current time (drift + hallucination).
//! Shipping a typed datetime tool removes both failure modes.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use time::OffsetDateTime;

const MANIFEST_TOML: &str = r#"
name        = "datetime"
description = "Current time, ISO 8601 formatting, RFC 3339 parsing. Args: {op: \"now\"|\"parse\", input?: string}."
usage       = "Use to stamp records, compare timestamps, or convert RFC 3339 to Unix seconds."
caps        = []
taint       = "trusted"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 4096

[schema]
type = "object"
"#;

/// Pure-compute datetime tool.
pub struct DatetimeTool {
    manifest: ToolManifest,
}

impl DatetimeTool {
    /// Build a new datetime tool.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("datetime".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for DatetimeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for DatetimeTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let op = args
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("now");

        match op {
            "now" => {
                let now = OffsetDateTime::now_utc();
                Ok(serde_json::json!({
                    "op": "now",
                    "unix_seconds": now.unix_timestamp(),
                    "unix_millis":  now.unix_timestamp_nanos() / 1_000_000,
                    "iso":          now
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                }))
            }
            "parse" => {
                let input = args
                    .get("input")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| GaussError::Internal("missing `input` for op=parse".into()))?;
                let dt = OffsetDateTime::parse(input, &time::format_description::well_known::Rfc3339)
                    .map_err(|e| GaussError::Internal(format!("parse: {e}")))?;
                Ok(serde_json::json!({
                    "op": "parse",
                    "input": input,
                    "unix_seconds": dt.unix_timestamp(),
                    "unix_millis":  dt.unix_timestamp_nanos() / 1_000_000,
                }))
            }
            other => Err(GaussError::Internal(format!(
                "unknown op `{other}`; want one of `now`, `parse`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn now_returns_recent_unix_seconds() {
        let t = DatetimeTool::new();
        let out = t.invoke_raw(serde_json::json!({ "op": "now" })).await.unwrap();
        let ts = out["unix_seconds"].as_i64().expect("integer seconds");
        // Generous lower bound: 1.7e9 ≈ 2023-11-15. If we ever ship the
        // tool with a system clock predating that we have bigger problems.
        assert!(ts > 1_700_000_000, "clock looks broken: {ts}");
        assert!(out["iso"].as_str().unwrap().contains('T'));
    }

    #[tokio::test]
    async fn parses_rfc3339() {
        let t = DatetimeTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "op": "parse",
                "input": "2024-01-15T12:00:00Z",
            }))
            .await
            .unwrap();
        assert_eq!(out["unix_seconds"], 1_705_320_000);
    }

    #[tokio::test]
    async fn rejects_unknown_op() {
        let t = DatetimeTool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "op": "explode" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn op_defaults_to_now() {
        let t = DatetimeTool::new();
        let out = t.invoke_raw(serde_json::json!({})).await.unwrap();
        assert_eq!(out["op"], "now");
    }
}
