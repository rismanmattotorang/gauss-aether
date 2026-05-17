//! [`JsonGetTool`] — JSON Pointer (RFC 6901) extraction. Pure compute,
//! no caps required. Returns the value at `/path/inside/nested/object`
//! within a supplied JSON document.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "json_get"
description = "Extract a JSON value by RFC 6901 pointer from a supplied document."
usage       = "Use to drill into JSON returned by another tool. Args: {input: object|array, pointer: \"/a/b/0\"}."
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

/// JSON Pointer extraction tool.
pub struct JsonGetTool {
    manifest: ToolManifest,
}

impl JsonGetTool {
    /// Build a new `JsonGetTool`.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("json_get".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for JsonGetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for JsonGetTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let input = args
            .get("input")
            .ok_or_else(|| GaussError::Internal("missing field `input`".into()))?;
        let pointer = args
            .get("pointer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `pointer`".into()))?;
        let found = input
            .pointer(pointer)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok(serde_json::json!({ "value": found }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn extracts_nested_value() {
        let t = JsonGetTool::new();
        let doc = serde_json::json!({ "a": { "b": [10, 20, 30] } });
        let out = t
            .invoke_raw(serde_json::json!({ "input": doc, "pointer": "/a/b/1" }))
            .await
            .unwrap();
        assert_eq!(out["value"], 20);
    }

    #[tokio::test]
    async fn missing_pointer_returns_null() {
        let t = JsonGetTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "input": {}, "pointer": "/nope" }))
            .await
            .unwrap();
        assert!(out["value"].is_null());
    }

    #[tokio::test]
    async fn rejects_missing_input() {
        let t = JsonGetTool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "pointer": "/x" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }
}
