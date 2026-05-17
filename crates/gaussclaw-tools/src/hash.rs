//! [`HashTool`] — BLAKE3 + SHA-256 of input text. No caps. Pure compute.

use async_trait::async_trait;
use blake3::Hasher as Blake3;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use sha2::{Digest, Sha256};

const MANIFEST_TOML: &str = r#"
name        = "hash"
description = "Compute BLAKE3 and SHA-256 hex digests of input text."
usage       = "Use to compute deterministic identifiers / content fingerprints."
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

/// Pure-compute hash tool.
pub struct HashTool {
    manifest: ToolManifest,
}

impl HashTool {
    /// Build a new `HashTool`.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("hash".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for HashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for HashTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `text`".into()))?;
        let bytes = text.as_bytes();
        let blake3_hex = Blake3::new().update(bytes).finalize().to_hex().to_string();
        let mut sha = Sha256::new();
        sha.update(bytes);
        let sha256_hex = hex_lower(&sha.finalize());
        Ok(serde_json::json!({
            "input_len": text.len(),
            "blake3":    blake3_hex,
            "sha256":    sha256_hex,
        }))
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        s.push(nibble(byte >> 4));
        s.push(nibble(byte & 0x0F));
    }
    s
}

#[allow(clippy::arithmetic_side_effects)]
const fn nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + n - 10) as char,
        _ => '0',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_match_known_vectors() {
        let t = HashTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "text": "abc" }))
            .await
            .unwrap();
        // Known vectors for "abc".
        assert_eq!(
            out["sha256"],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            out["blake3"],
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
        assert_eq!(out["input_len"], 3);
    }

    #[tokio::test]
    async fn empty_string_is_valid() {
        let t = HashTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "text": "" }))
            .await
            .unwrap();
        // SHA-256("")
        assert_eq!(
            out["sha256"],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(out["input_len"], 0);
    }

    #[tokio::test]
    async fn rejects_missing_text() {
        let t = HashTool::new();
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_no_caps() {
        let t = HashTool::new();
        assert_eq!(t.manifest().cap_required.bits(), 0);
    }
}
