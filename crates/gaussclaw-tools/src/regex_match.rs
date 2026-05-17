//! [`RegexMatchTool`] — Rust `regex` crate match. No caps. Pure compute.
//!
//! ## Hermes-superior contract
//!
//! Hermes upstream uses Python `re.match()` — backtracking engine,
//! catastrophic-backtracking exposure on adversarial patterns
//! (`(a+)+$` style ReDoS). The Rust `regex` crate is NFA/DFA-based and
//! **rejects** patterns that would require backtracking (no `\1`
//! backreferences, no lookaround) — ReDoS-resistant by design.
//!
//! Adversarial patterns that the regex engine cannot compile in
//! linear time are refused at `Regex::new` with an `Error::Syntax`,
//! surfaced as `GaussError::Internal`.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use regex::Regex;

const MANIFEST_TOML: &str = r#"
name        = "regex_match"
description = "Test whether a regex (RE2-style; no backtracking) matches text. Returns {matched, captures}."
usage       = "Use to extract structured fragments from text. Args: {pattern, text}."
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

/// Regex-match tool. The Rust `regex` crate's RE2-style engine refuses
/// patterns that would require backtracking — ReDoS-resistant by
/// construction.
pub struct RegexMatchTool {
    manifest: ToolManifest,
}

impl RegexMatchTool {
    /// Build a new `RegexMatchTool`.
    ///
    /// # Panics
    /// Build-time only on embedded manifest parse failure.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("regex_match".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for RegexMatchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for RegexMatchTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `pattern`".into()))?;
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `text`".into()))?;
        let re =
            Regex::new(pattern).map_err(|e| GaussError::Internal(format!("regex compile: {e}")))?;
        let captures: Vec<&str> = re
            .captures(text)
            .map(|c| {
                c.iter()
                    .skip(1)
                    .filter_map(|m| m.map(|mm| mm.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(serde_json::json!({
            "matched":  re.is_match(text),
            "captures": captures,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn simple_match_returns_true() {
        let t = RegexMatchTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "pattern": r"hello\s+world",
                "text": "hello world",
            }))
            .await
            .unwrap();
        assert_eq!(out["matched"], true);
    }

    #[tokio::test]
    async fn no_match_returns_false() {
        let t = RegexMatchTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "pattern": r"^foo$",
                "text": "bar",
            }))
            .await
            .unwrap();
        assert_eq!(out["matched"], false);
    }

    #[tokio::test]
    async fn captures_are_extracted() {
        let t = RegexMatchTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "pattern": r"(\w+)=(\d+)",
                "text": "x=42",
            }))
            .await
            .unwrap();
        let caps = out["captures"].as_array().unwrap();
        assert_eq!(caps.len(), 2);
        assert_eq!(caps[0], "x");
        assert_eq!(caps[1], "42");
    }

    /// ReDoS-resistance: a backreference-style pattern that would
    /// catastrophically backtrack in Python's `re` is REFUSED at
    /// compile by the Rust `regex` crate (it doesn't support
    /// backreferences at all). The refusal surfaces as
    /// `GaussError::Internal`, not a DoS.
    #[tokio::test]
    async fn redos_pattern_refused_at_compile() {
        let t = RegexMatchTool::new();
        let err = t
            .invoke_raw(serde_json::json!({
                "pattern": r"(a+)\1+",   // backreference: unsupported
                "text": "aaaa",
            }))
            .await
            .unwrap_err();
        // Either a compile error (likely) or a successful no-match.
        // What we MUST avoid: an unbounded run on adversarial input.
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn rejects_missing_pattern() {
        let t = RegexMatchTool::new();
        let err = t
            .invoke_raw(serde_json::json!({ "text": "x" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_no_caps() {
        let t = RegexMatchTool::new();
        assert_eq!(t.manifest().cap_required.bits(), 0);
    }
}
