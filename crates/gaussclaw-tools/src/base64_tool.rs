//! [`Base64Tool`] — RFC 4648 base64 encode / decode. No caps. Pure compute.

use async_trait::async_trait;
use base64::Engine;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "base64"
description = "Encode or decode base64 (RFC 4648). Args: {op: \"encode\"|\"decode\", input: str}."
usage       = "Use to handle binary blobs that round-trip through JSON."
caps        = []
taint       = "trusted"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 1048576

[schema]
type = "object"
"#;

/// Base64 encode/decode tool.
pub struct Base64Tool {
    manifest: ToolManifest,
}

impl Base64Tool {
    /// Build a new `Base64Tool`.
    ///
    /// # Panics
    /// Build-time only on embedded manifest parse failure.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("base64".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for Base64Tool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for Base64Tool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let op = args
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `op`".into()))?;
        let input = args
            .get("input")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `input`".into()))?;
        let engine = base64::engine::general_purpose::STANDARD;
        let output = match op {
            "encode" => engine.encode(input.as_bytes()),
            "decode" => {
                let bytes = engine
                    .decode(input)
                    .map_err(|e| GaussError::Internal(format!("decode: {e}")))?;
                String::from_utf8(bytes).map_err(|e| GaussError::Internal(format!("utf8: {e}")))?
            }
            other => {
                return Err(GaussError::Internal(format!(
                    "unknown op `{other}`; expected `encode` or `decode`"
                )));
            }
        };
        Ok(serde_json::json!({ "op": op, "output": output }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn encode_round_trip() {
        let t = Base64Tool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "op": "encode", "input": "hello" }))
            .await
            .unwrap();
        assert_eq!(out["output"], "aGVsbG8=");
    }

    #[tokio::test]
    async fn decode_round_trip() {
        let t = Base64Tool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "op": "decode", "input": "aGVsbG8=" }))
            .await
            .unwrap();
        assert_eq!(out["output"], "hello");
    }

    #[tokio::test]
    async fn unknown_op_is_rejected() {
        let t = Base64Tool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "op": "rot13", "input": "x" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn invalid_decode_is_rejected() {
        let t = Base64Tool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "op": "decode", "input": "%%%not-base64%%%" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_no_caps() {
        let t = Base64Tool::new();
        assert_eq!(t.manifest().cap_required.bits(), 0);
    }
}
