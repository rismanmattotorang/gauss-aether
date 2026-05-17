//! [`UpperTool`] — uppercase a string. Pure compute, no caps.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "upper"
description = "Convert a string to uppercase."
usage       = "Use to normalise case for an exact-match lookup."
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

/// Uppercase tool.
pub struct UpperTool {
    manifest: ToolManifest,
}

impl UpperTool {
    /// Build a new `UpperTool`.
    ///
    /// # Panics
    /// Build-time only on embedded manifest parse failure.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("upper".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for UpperTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for UpperTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `text`".into()))?;
        Ok(serde_json::json!({ "upper": text.to_uppercase() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn uppercases_text() {
        let t = UpperTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "text": "hello world" }))
            .await
            .unwrap();
        assert_eq!(out["upper"], "HELLO WORLD");
    }
}
