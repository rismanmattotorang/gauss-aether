//! [`ClarifyTool`] — pause the agent loop and surface an approval
//! overlay.
//!
//! When the model calls `clarify({"question": "…", "options":
//! ["a", "b", …]})`, the tool returns a structured "pending" payload
//! that the host surface (TUI / web dashboard / desktop) intercepts
//! to render the matching overlay (TUI: [`gaussclaw_tui::Overlay::clarify`];
//! dashboard: a modal). The loop's [`LoopSink`] forwards the
//! `tool.complete` event over the WebSocket; the frontend prompts
//! the operator, and the operator's answer comes back through a new
//! user-role message on the next iteration.
//!
//! The Hermes upstream ships an equivalent `clarify_tool`; the
//! GaussClaw version is **cap-gated** by the new `cap:approval:ask`
//! token (added to `gauss-core` in this commit) so a low-privilege
//! sub-agent can't open an approval prompt on the operator's behalf.

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "clarify"
description = "Pause and ask the operator for a one-line clarification (or a quick-pick from up to nine options)."
usage       = "Use when the user's intent is ambiguous. Args: {question: string, options?: [string]}."
caps        = ["approval:ask"]
taint       = "user"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 4096

[schema]
type = "object"
"#;

/// Cap on the number of quick-pick options the model can emit.
const MAX_OPTIONS: usize = 9;

/// Clarify tool. The result payload is structured so the host surface
/// can render the matching overlay without further parsing.
pub struct ClarifyTool {
    manifest: ToolManifest,
}

impl ClarifyTool {
    /// Build.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("clarify".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for ClarifyTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for ClarifyTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `question`".into()))?;
        let options: Vec<String> = args
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .take(MAX_OPTIONS)
                    .collect()
            })
            .unwrap_or_default();
        // The tool itself does not block — it returns a structured
        // "pending" payload; the host surface intercepts it on the
        // sink side. The agent's next iteration receives the
        // operator's answer as a tool-role message.
        Ok(serde_json::json!({
            "kind":     "clarify_pending",
            "question": question,
            "options":  options,
            "max_options": MAX_OPTIONS,
        }))
    }
}

/// Cap surfaced by [`ClarifyTool::manifest`].
#[must_use]
pub const fn approval_ask_cap() -> CapToken {
    CapToken::APPROVAL_ASK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_pending_payload() {
        let t = ClarifyTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "question": "Which file did you mean?",
                "options": ["src/a.rs", "src/b.rs"],
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "clarify_pending");
        assert_eq!(out["question"], "Which file did you mean?");
        assert_eq!(out["options"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn rejects_missing_question() {
        let t = ClarifyTool::new();
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn clamps_options_to_nine() {
        let t = ClarifyTool::new();
        let options: Vec<&str> = (0..20).map(|_| "x").collect();
        let out = t
            .invoke_raw(serde_json::json!({
                "question": "?",
                "options": options,
            }))
            .await
            .unwrap();
        assert_eq!(out["options"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn manifest_declares_approval_ask_cap() {
        let t = ClarifyTool::new();
        assert_eq!(t.manifest().cap_required, approval_ask_cap());
    }
}
