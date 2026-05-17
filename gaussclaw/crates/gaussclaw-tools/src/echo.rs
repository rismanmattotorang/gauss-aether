//! [`EchoTool`] — trivial pure-compute tool. Returns its input wrapped
//! in an `echo` field. Useful as a sanity-check tool and a template
//! for new pure-compute tools.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "echo"
description = "Return the input text wrapped in {echo: ...}. Useful as a sanity check."
usage       = "Use to verify tool dispatch is wired."
caps        = []
taint       = "trusted"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// Pure-compute echo tool.
pub struct EchoTool {
    manifest: ToolManifest,
}

impl EchoTool {
    /// Build a new echo tool.
    ///
    /// # Panics
    /// Panics only if the embedded manifest TOML fails to parse, which
    /// is a build-time bug.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("echo".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for EchoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for EchoTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `text`".into()))?;
        Ok(serde_json::json!({ "echo": text }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echoes_user_text() {
        let t = EchoTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "text": "hello" }))
            .await
            .unwrap();
        assert_eq!(out["echo"], "hello");
    }

    #[tokio::test]
    async fn rejects_missing_text_field() {
        let t = EchoTool::new();
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_no_caps_and_trusted_taint() {
        let t = EchoTool::new();
        // No caps required → BOTTOM bits.
        assert_eq!(t.manifest().cap_required.bits(), 0);
        assert!(t.manifest().reversible);
    }
}
