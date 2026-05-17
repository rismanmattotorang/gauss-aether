//! [`MathEvalTool`] — safe arithmetic expression evaluator. No caps.
//!
//! Supports `+`, `-`, `*`, `/`, parentheses, and `f64` literals. The
//! parser is a recursive-descent that hand-rolls operator precedence
//! and does not call into a general-purpose `eval` — no code execution
//! path, no shell-out, no `unsafe`. Pure compute.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "math_eval"
description = "Evaluate a safe arithmetic expression (+, -, *, /, parentheses)."
usage       = "Use to compute numeric answers without inventing them."
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

/// Safe arithmetic evaluator.
pub struct MathEvalTool {
    manifest: ToolManifest,
}

impl MathEvalTool {
    /// Build a new `MathEvalTool`.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("math_eval".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for MathEvalTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for MathEvalTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let expr = args
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `expression`".into()))?;
        let mut parser = Parser::new(expr);
        let value = parser.parse_expr().map_err(GaussError::Internal)?;
        parser.expect_eof().map_err(GaussError::Internal)?;
        Ok(serde_json::json!({ "expression": expr, "value": value }))
    }
}

// ─── recursive-descent parser ──────────────────────────────────────────────

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    const fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    const fn peek(&self) -> Option<u8> {
        // Manual indexing avoids the `is_some()`-style API on slices that
        // isn't `const`. `self.src` is `&[u8]` so direct indexing works.
        if self.pos < self.src.len() {
            Some(self.src[self.pos])
        } else {
            None
        }
    }

    const fn bump(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    const fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.bump();
        }
    }

    fn expect_eof(&mut self) -> Result<(), String> {
        self.skip_ws();
        if self.peek().is_some() {
            Err(format!("trailing input at position {}", self.pos))
        } else {
            Ok(())
        }
    }

    // expr := term (('+' | '-') term)*
    fn parse_expr(&mut self) -> Result<f64, String> {
        let mut lhs = self.parse_term()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.bump();
                    let rhs = self.parse_term()?;
                    lhs += rhs;
                }
                Some(b'-') => {
                    self.bump();
                    let rhs = self.parse_term()?;
                    lhs -= rhs;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    // term := factor (('*' | '/') factor)*
    fn parse_term(&mut self) -> Result<f64, String> {
        let mut lhs = self.parse_factor()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.bump();
                    let rhs = self.parse_factor()?;
                    lhs *= rhs;
                }
                Some(b'/') => {
                    self.bump();
                    let rhs = self.parse_factor()?;
                    if rhs.abs() < f64::EPSILON {
                        return Err("division by zero".into());
                    }
                    lhs /= rhs;
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    // factor := unary | number | '(' expr ')'
    fn parse_factor(&mut self) -> Result<f64, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.bump();
                let v = self.parse_expr()?;
                self.skip_ws();
                if self.peek() != Some(b')') {
                    return Err(format!("expected ')' at position {}", self.pos));
                }
                self.bump();
                Ok(v)
            }
            Some(b'-') => {
                self.bump();
                Ok(-self.parse_factor()?)
            }
            Some(b'+') => {
                self.bump();
                self.parse_factor()
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => self.parse_number(),
            Some(c) => Err(format!("unexpected byte {c:?} at position {}", self.pos)),
            None => Err("unexpected end of input".into()),
        }
    }

    fn parse_number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        let mut saw_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.bump();
            } else if c == b'.' && !saw_dot {
                saw_dot = true;
                self.bump();
            } else {
                break;
            }
        }
        let slice = &self.src[start..self.pos];
        let s = std::str::from_utf8(slice).map_err(|_e| "non-utf8 number".to_string())?;
        s.parse::<f64>().map_err(|e| format!("number parse: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn eval(expr: &str) -> Result<f64, String> {
        let t = MathEvalTool::new();
        match t
            .invoke_raw(serde_json::json!({ "expression": expr }))
            .await
        {
            Ok(v) => Ok(v["value"].as_f64().unwrap()),
            Err(GaussError::Internal(msg)) => Err(msg),
            Err(_) => Err("non-internal error".into()),
        }
    }

    #[tokio::test]
    #[allow(clippy::float_cmp)]
    async fn basic_arithmetic() {
        assert_eq!(eval("1 + 2").await.unwrap(), 3.0);
        assert_eq!(eval("10 - 4").await.unwrap(), 6.0);
        assert_eq!(eval("3 * 5").await.unwrap(), 15.0);
        assert_eq!(eval("20 / 4").await.unwrap(), 5.0);
    }

    #[tokio::test]
    #[allow(clippy::float_cmp)]
    async fn precedence_and_parens() {
        assert_eq!(eval("1 + 2 * 3").await.unwrap(), 7.0);
        assert_eq!(eval("(1 + 2) * 3").await.unwrap(), 9.0);
        assert_eq!(eval("2 * (3 + 4)").await.unwrap(), 14.0);
    }

    #[tokio::test]
    #[allow(clippy::float_cmp)]
    async fn unary_minus_and_decimals() {
        assert_eq!(eval("-3").await.unwrap(), -3.0);
        assert_eq!(eval("3.5 * 2").await.unwrap(), 7.0);
        assert_eq!(eval("--5").await.unwrap(), 5.0);
    }

    #[tokio::test]
    async fn division_by_zero_is_rejected() {
        let err = eval("1 / 0").await.unwrap_err();
        assert!(err.contains("division by zero"));
    }

    #[tokio::test]
    async fn trailing_input_is_rejected() {
        let err = eval("1 + 2 garbage").await.unwrap_err();
        assert!(err.contains("trailing") || err.contains("unexpected"));
    }

    #[tokio::test]
    async fn missing_expression_is_rejected() {
        let t = MathEvalTool::new();
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_no_caps() {
        let t = MathEvalTool::new();
        assert_eq!(t.manifest().cap_required.bits(), 0);
    }
}
