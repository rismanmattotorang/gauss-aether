//! [`JsonSetTool`] — write a value at an RFC 6901 JSON Pointer.
//!
//! Pure-compute, no caps. The mirror image of [`crate::JsonGetTool`].
//! Returns the modified document; the original is untouched (the tool is
//! reversible / pure-functional).

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "json_set"
description = "Write a value at an RFC 6901 JSON Pointer. Returns the modified document."
usage       = "Use to splice values into a structured payload. Args: {input: object|array, pointer: \"/a/b\", value: any}."
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

/// JSON Pointer write tool.
pub struct JsonSetTool {
    manifest: ToolManifest,
}

impl JsonSetTool {
    /// Build a new JSON-set tool.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("json_set".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for JsonSetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for JsonSetTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let mut input = args
            .get("input")
            .cloned()
            .ok_or_else(|| GaussError::Internal("missing field `input`".into()))?;
        let pointer = args
            .get("pointer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `pointer`".into()))?;
        let value = args
            .get("value")
            .cloned()
            .ok_or_else(|| GaussError::Internal("missing field `value`".into()))?;

        // Root replacement.
        if pointer.is_empty() {
            return Ok(serde_json::json!({
                "output": value,
                "modified": pointer,
            }));
        }

        let target = input.pointer_mut(pointer).ok_or_else(|| {
            GaussError::Internal(format!("pointer `{pointer}` does not resolve in input"))
        })?;
        *target = value;

        Ok(serde_json::json!({
            "output": input,
            "modified": pointer,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_into_nested_object() {
        let t = JsonSetTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "input": { "a": { "b": 1 } },
                "pointer": "/a/b",
                "value": 42,
            }))
            .await
            .unwrap();
        assert_eq!(out["output"]["a"]["b"], 42);
        assert_eq!(out["modified"], "/a/b");
    }

    #[tokio::test]
    async fn writes_into_array() {
        let t = JsonSetTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "input": { "xs": [1, 2, 3] },
                "pointer": "/xs/1",
                "value": 99,
            }))
            .await
            .unwrap();
        assert_eq!(out["output"]["xs"], serde_json::json!([1, 99, 3]));
    }

    #[tokio::test]
    async fn empty_pointer_replaces_root() {
        let t = JsonSetTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "input": { "a": 1 },
                "pointer": "",
                "value": { "b": 2 },
            }))
            .await
            .unwrap();
        assert_eq!(out["output"], serde_json::json!({ "b": 2 }));
    }

    #[tokio::test]
    async fn rejects_missing_pointer_target() {
        let t = JsonSetTool::new();
        let err = t
            .invoke_raw(serde_json::json!({
                "input": { "a": 1 },
                "pointer": "/does/not/exist",
                "value": 42,
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }
}
