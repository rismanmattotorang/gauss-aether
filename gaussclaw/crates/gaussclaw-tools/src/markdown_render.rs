//! `markdown_render` tool (Sprint 7 §4).
//!
//! Renders a markdown source string to a plain-text or HTML output.
//! The implementation is intentionally narrow (headings, lists,
//! code blocks, bold/italic, links) so the dependency surface stays
//! zero. Hermes ships a heavier `markdown-it`-based renderer; ours
//! prefers reproducibility over completeness.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "markdown_render"
description = "Render a markdown source to plain-text or HTML. Pure-compute, deterministic."
usage       = "Args: {source: string, format?: 'text'|'html'}. Returns {rendered}."
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

/// `markdown_render` tool.
pub struct MarkdownRenderTool {
    manifest: ToolManifest,
}

impl MarkdownRenderTool {
    /// Build.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("toml");
        let manifest = skill
            .compile(ToolId("markdown_render".into()))
            .expect("compile");
        Self { manifest }
    }
}

impl Default for MarkdownRenderTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for MarkdownRenderTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let source = args
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `source`".into()))?;
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let rendered = match format {
            "text" => render_text(source),
            "html" => render_html(source),
            other => {
                return Err(GaussError::Internal(format!(
                    "unknown format {other:?} (try text/html)"
                )))
            }
        };
        Ok(serde_json::json!({
            "kind":     "markdown_rendered",
            "format":   format,
            "rendered": rendered,
        }))
    }
}

/// Render a narrow markdown subset to plain text. Headings strip the
/// `#`; lists prefix with `- `; code blocks pass through. Inline
/// emphasis markers are removed.
#[must_use]
pub fn render_text(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push_str(rest);
            out.push('\n');
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push_str(rest);
            out.push('\n');
        } else if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push_str(rest);
            out.push('\n');
        } else if let Some(rest) = trimmed.strip_prefix("- ") {
            out.push_str("- ");
            out.push_str(&strip_inline(rest));
            out.push('\n');
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            out.push_str("- ");
            out.push_str(&strip_inline(rest));
            out.push('\n');
        } else {
            out.push_str(&strip_inline(line));
            out.push('\n');
        }
    }
    out
}

/// Render the same narrow subset to HTML. Output is intentionally
/// minimal; the rendered HTML is *not* sanitised for general web
/// use — the tool is meant for downstream agent consumption.
#[must_use]
pub fn render_html(source: &str) -> String {
    let mut out = String::with_capacity(source.len() * 2);
    let mut in_list = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        let bullet = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "));
        if let Some(rest) = bullet {
            if !in_list {
                out.push_str("<ul>");
                in_list = true;
            }
            out.push_str("<li>");
            out.push_str(&escape_html(&strip_inline(rest)));
            out.push_str("</li>");
            continue;
        }
        if in_list {
            out.push_str("</ul>");
            in_list = false;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push_str("<h1>");
            out.push_str(&escape_html(rest));
            out.push_str("</h1>");
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push_str("<h2>");
            out.push_str(&escape_html(rest));
            out.push_str("</h2>");
        } else if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push_str("<h3>");
            out.push_str(&escape_html(rest));
            out.push_str("</h3>");
        } else if !line.is_empty() {
            out.push_str("<p>");
            out.push_str(&escape_html(&strip_inline(line)));
            out.push_str("</p>");
        }
    }
    if in_list {
        out.push_str("</ul>");
    }
    out
}

fn strip_inline(s: &str) -> String {
    // Remove **bold**, *italic*, `code`, [text](url) → text.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                }
            }
            '_' | '`' => {}
            '[' => {
                let text: String = chars.by_ref().take_while(|c| *c != ']').collect();
                // Skip the `(url)` if present.
                if chars.peek() == Some(&'(') {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == ')' {
                            break;
                        }
                    }
                }
                out.push_str(&text);
            }
            other => out.push(other),
        }
    }
    out
}

fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn renders_headings_to_plain_text() {
        let t = MarkdownRenderTool::new();
        let out = t
            .invoke_raw(serde_json::json!({"source": "# Title\nbody"}))
            .await
            .unwrap();
        let rendered = out["rendered"].as_str().unwrap();
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("body"));
    }

    #[tokio::test]
    async fn renders_lists_to_html() {
        let t = MarkdownRenderTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "source": "- one\n- two",
                "format": "html"
            }))
            .await
            .unwrap();
        let rendered = out["rendered"].as_str().unwrap();
        assert!(rendered.contains("<ul>"));
        assert!(rendered.contains("<li>one</li>"));
        assert!(rendered.contains("<li>two</li>"));
        assert!(rendered.contains("</ul>"));
    }

    #[tokio::test]
    async fn escapes_html_in_html_format() {
        let t = MarkdownRenderTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "source": "# <script>",
                "format": "html"
            }))
            .await
            .unwrap();
        let rendered = out["rendered"].as_str().unwrap();
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(!rendered.contains("<script>"));
    }

    #[test]
    fn strip_inline_removes_emphasis_markers() {
        assert_eq!(strip_inline("**bold**"), "bold");
        assert_eq!(strip_inline("_italic_"), "italic");
        assert_eq!(strip_inline("`code`"), "code");
        assert_eq!(strip_inline("[text](http://x)"), "text");
    }

    #[tokio::test]
    async fn unknown_format_rejected() {
        let t = MarkdownRenderTool::new();
        let err = t
            .invoke_raw(serde_json::json!({"source": "x", "format": "rtf"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn missing_source_rejected() {
        let t = MarkdownRenderTool::new();
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn render_text_preserves_paragraph_structure() {
        let r = render_text("# Heading\n\nA paragraph\n- item\n- another");
        assert!(r.contains("Heading"));
        assert!(r.contains("- item"));
        assert!(r.contains("- another"));
    }
}
