//! Prompt enrichment — pluggable system-prompt augmentation.
//!
//! OpenHarness (HKUDS/OpenHarness) injects two kinds of "ambient
//! knowledge" into every prompt:
//!
//! * **Context files** — the working-directory `CLAUDE.md` discovered
//!   via [`gaussclaw_skill::ContextFileFinder`]. Treated as
//!   project-specific operating instructions.
//! * **Skill bodies** — markdown skills discovered via
//!   [`gaussclaw_skill::MarkdownSkill::discover_in`]. Loaded
//!   on-demand and prepended to the prompt when the model requests
//!   them.
//!
//! GaussClaw now exposes the same surface via the [`PromptEnricher`]
//! trait. The agent loop consults each enricher before every
//! provider invocation and prepends the result as one extra
//! `system`-role message. Several enrichers can compose; their
//! contributions are concatenated in registration order.
//!
//! ## Why a trait?
//!
//! 1. **Each surface picks its own source.** The TUI may want the
//!    `CLAUDE.md` walk; a channel adapter might want the user's
//!    plugin-defined operating instructions. Both implement the trait;
//!    the agent loop only sees the rendered text.
//!
//! 2. **No coupling to gaussclaw-skill from gaussclaw-agent.** The
//!    enricher trait lives here; the concrete implementations
//!    ([`ContextFileEnricher`], [`MarkdownSkillEnricher`]) live in
//!    gaussclaw-skill so we don't add a back-edge in the dep graph.
//!
//! 3. **Idempotent + cacheable.** An enricher returns `Option<String>`;
//!    the loop dedupes against the prior call so a no-change result
//!    doesn't grow the prompt.
//!
//! ## Composition with Auto-Compaction
//!
//! Enricher output lands as a *leading* system message, so the
//! [`WindowedCompactor`] (which preserves the leading system message
//! verbatim) never collapses it. Enrichers can therefore inject
//! durable instructions without worrying about loss under context
//! pressure.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc
)]

use async_trait::async_trait;

use crate::Message;

/// Pluggable prompt-prefix supplier.
///
/// `enrich` returns the additional system-message body, or `None`
/// to opt out (the loop skips appending). Implementers must keep
/// the result deterministic for the same inputs — the agent loop
/// uses the body as a cache key.
#[async_trait]
pub trait PromptEnricher: Send + Sync {
    /// Stable name used for audit-log keying and the
    /// `<!-- prompt-enricher: <name> -->` marker in the rendered
    /// system message.
    fn name(&self) -> &str;

    /// Produce an enrichment body. The agent loop wraps the body in
    /// an HTML comment marker so the model sees the source; on a
    /// `None` return the enricher is silently skipped.
    async fn enrich(&self) -> Option<String>;
}

/// Render an enrichment body with the marker the agent loop expects.
/// The comment marker stays compatible with markdown renderers and
/// makes audit-log replay deterministic.
#[must_use]
pub fn wrap_enrichment(name: &str, body: &str) -> String {
    format!("<!-- prompt-enricher: {name} -->\n{body}")
}

/// Convenience: build a leading system [`Message`] from a list of
/// enrichers. Skips `None` returns; concatenates the wrapped bodies
/// with a divider so the result reads as one coherent system block.
///
/// Returns `None` if every enricher opted out.
pub async fn collect_enrichments(
    enrichers: &[std::sync::Arc<dyn PromptEnricher>],
) -> Option<Message> {
    let mut bodies: Vec<String> = Vec::new();
    for e in enrichers {
        if let Some(b) = e.enrich().await {
            bodies.push(wrap_enrichment(e.name(), &b));
        }
    }
    if bodies.is_empty() {
        return None;
    }
    Some(Message::new("system", bodies.join("\n\n---\n\n")))
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Static {
        name: &'static str,
        body: Option<&'static str>,
    }
    #[async_trait]
    impl PromptEnricher for Static {
        fn name(&self) -> &str {
            self.name
        }
        async fn enrich(&self) -> Option<String> {
            self.body.map(str::to_owned)
        }
    }

    #[tokio::test]
    async fn wrap_enrichment_includes_marker() {
        let s = wrap_enrichment("my-rule", "always be helpful");
        assert!(s.contains("<!-- prompt-enricher: my-rule -->"));
        assert!(s.contains("always be helpful"));
    }

    #[tokio::test]
    async fn collect_enrichments_returns_none_when_all_opt_out() {
        let enrichers: Vec<Arc<dyn PromptEnricher>> = vec![
            Arc::new(Static {
                name: "a",
                body: None,
            }),
            Arc::new(Static {
                name: "b",
                body: None,
            }),
        ];
        assert!(collect_enrichments(&enrichers).await.is_none());
    }

    #[tokio::test]
    async fn collect_enrichments_concatenates_in_order() {
        let enrichers: Vec<Arc<dyn PromptEnricher>> = vec![
            Arc::new(Static {
                name: "ctx",
                body: Some("FIRST"),
            }),
            Arc::new(Static {
                name: "skill",
                body: Some("SECOND"),
            }),
        ];
        let msg = collect_enrichments(&enrichers).await.expect("some");
        assert_eq!(msg.role, "system");
        let body = &msg.content;
        let pos_first = body.find("FIRST").unwrap();
        let pos_second = body.find("SECOND").unwrap();
        assert!(pos_first < pos_second);
        assert!(body.contains("<!-- prompt-enricher: ctx -->"));
        assert!(body.contains("<!-- prompt-enricher: skill -->"));
        assert!(body.contains("\n---\n"));
    }

    #[tokio::test]
    async fn collect_enrichments_skips_none_in_the_middle() {
        let enrichers: Vec<Arc<dyn PromptEnricher>> = vec![
            Arc::new(Static {
                name: "a",
                body: Some("A"),
            }),
            Arc::new(Static {
                name: "skip",
                body: None,
            }),
            Arc::new(Static {
                name: "c",
                body: Some("C"),
            }),
        ];
        let msg = collect_enrichments(&enrichers).await.expect("some");
        assert!(msg.content.contains("A"));
        assert!(msg.content.contains("C"));
        assert!(!msg.content.contains("<!-- prompt-enricher: skip -->"));
    }
}
