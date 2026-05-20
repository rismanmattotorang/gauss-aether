//! Concrete [`PromptEnricher`] implementations.
//!
//! The trait lives in `enrich.rs` precisely so this module can supply
//! batteries-included implementations without coupling consumers to a
//! specific discovery strategy. Today we ship two:
//!
//! * [`ContextFileEnricher`] — walks the working directory + ancestors
//!   for `CLAUDE.md` / `GAUSSCLAW.md` and joins the bodies. Composes
//!   with [`gaussclaw_skill::ContextFileFinder`] so operators tune
//!   the walk depth, byte cap, and candidate names.
//!
//! * [`MarkdownSkillEnricher`] — discovers every `SKILL.md` under a
//!   skills root and exposes a per-skill heading + body. Optionally
//!   filtered by name allowlist when the operator only wants a
//!   subset surfaced.
//!
//! Both implementations are *deterministic*: identical filesystem
//! state yields identical output bytes, so audit replay and
//! conformance snapshots stay stable. Failures during discovery
//! (missing directory, refused symlink) are silently skipped — the
//! enricher returns `None` and the loop runs as if the surface
//! were absent. Operators who want a hard failure should walk the
//! finder themselves before constructing the loop.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc
)]

use std::path::PathBuf;

use async_trait::async_trait;
use gaussclaw_skill::{join_context, ContextFileFinder, MarkdownSkill, MemoryFile};

use crate::enrich::PromptEnricher;

// ─── ContextFileEnricher ──────────────────────────────────────────────────

/// Walks the working directory and its ancestors for `CLAUDE.md` /
/// `GAUSSCLAW.md` and surfaces every match as one consolidated
/// enrichment body.
pub struct ContextFileEnricher {
    /// Root directory to start the walk from.
    pub start: PathBuf,
    /// Configured walker.
    pub finder: ContextFileFinder,
    /// Stable enricher name used in the `<!-- prompt-enricher: -->`
    /// marker the agent loop attaches.
    pub label: String,
}

impl ContextFileEnricher {
    /// Build with the default [`ContextFileFinder`] (8-deep, 64 KiB
    /// per file, `CLAUDE.md` then `GAUSSCLAW.md`).
    pub fn new(start: impl Into<PathBuf>) -> Self {
        Self {
            start: start.into(),
            finder: ContextFileFinder::new(),
            label: "context-files".to_owned(),
        }
    }

    /// Builder: swap the [`ContextFileFinder`] policy.
    #[must_use]
    pub fn with_finder(mut self, finder: ContextFileFinder) -> Self {
        self.finder = finder;
        self
    }

    /// Builder: replace the enricher label (default `"context-files"`).
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

#[async_trait]
impl PromptEnricher for ContextFileEnricher {
    fn name(&self) -> &str {
        &self.label
    }

    async fn enrich(&self) -> Option<String> {
        let files = self.finder.discover(&self.start).ok()?;
        if files.is_empty() {
            return None;
        }
        Some(join_context(&files))
    }
}

// ─── MarkdownSkillEnricher ────────────────────────────────────────────────

/// Discovers `SKILL.md` files under one skills root and renders each
/// as a `## <name>` section.
///
/// Use [`Self::with_allowlist`] to surface only a subset of the
/// discovered skills (the rest stay on disk but stay out of the
/// prompt — useful when only a few skills are relevant to the
/// session's task).
pub struct MarkdownSkillEnricher {
    /// Skills root directory.
    pub root: PathBuf,
    /// Optional name allowlist. `None` = all skills surfaced.
    pub allow: Option<Vec<String>>,
    /// Enricher label (default `"markdown-skills"`).
    pub label: String,
}

impl MarkdownSkillEnricher {
    /// Build with the given skills root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            allow: None,
            label: "markdown-skills".to_owned(),
        }
    }

    /// Builder: restrict the enrichment to the named skills only.
    /// Names are matched case-sensitively against
    /// [`MarkdownSkill::name`].
    #[must_use]
    pub fn with_allowlist(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.allow = Some(names.into_iter().collect());
        self
    }

    /// Builder: replace the label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

#[async_trait]
impl PromptEnricher for MarkdownSkillEnricher {
    fn name(&self) -> &str {
        &self.label
    }

    async fn enrich(&self) -> Option<String> {
        let skills = MarkdownSkill::discover_in(&self.root).ok()?;
        let allow = self.allow.as_ref();
        let filtered: Vec<&MarkdownSkill> = skills
            .iter()
            .filter(|s| allow.is_none_or(|a| a.iter().any(|n| n == &s.name)))
            .collect();
        if filtered.is_empty() {
            return None;
        }
        let mut out = String::new();
        out.push_str("# Loaded skills\n\n");
        for (i, s) in filtered.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            out.push_str(&format!("## {name}\n", name = s.name));
            if let Some(desc) = s.description() {
                if !desc.is_empty() {
                    out.push_str(&format!("_{desc}_\n\n"));
                }
            }
            out.push_str(s.body.trim());
            out.push('\n');
        }
        Some(out)
    }
}

// ─── MemoryFileEnricher ───────────────────────────────────────────────────

/// Loads a [`MemoryFile`] (typically `~/.gaussclaw/MEMORY.md`) on each
/// `enrich()` call and renders the section list as a `## Persistent
/// memory` block.
///
/// The enricher trusts the [`MemoryFile`] discipline — `load_or_default`
/// returns an empty file when the path is missing, so a deployment
/// that hasn't yet curated any memory simply opts out. When the file
/// exists, every section in declaration order is rendered (no
/// allowlist filtering — the agent owns its own memory and is the
/// authority on what's worth keeping).
///
/// Combined with [`ContextFileEnricher`] and [`MarkdownSkillEnricher`],
/// the agent gets three layers of ambient context per turn:
///
/// 1. Project — `CLAUDE.md` walked from the working directory.
/// 2. Skills  — `SKILL.md` bodies discovered under the skills root.
/// 3. Memory  — `MEMORY.md` agent-curated cross-session knowledge.
pub struct MemoryFileEnricher {
    /// Path to `MEMORY.md` (or whatever name the operator chose).
    pub path: PathBuf,
    /// Enricher label (default `"memory-file"`).
    pub label: String,
}

impl MemoryFileEnricher {
    /// Wrap a `MEMORY.md` path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            label: "memory-file".to_owned(),
        }
    }

    /// Builder: replace the label (default `"memory-file"`).
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

#[async_trait]
impl PromptEnricher for MemoryFileEnricher {
    fn name(&self) -> &str {
        &self.label
    }

    async fn enrich(&self) -> Option<String> {
        let mem = MemoryFile::load_or_default(&self.path).ok()?;
        if mem.is_empty() {
            return None;
        }
        // Render the file body inline. We don't double-wrap in
        // headings — `MemoryFile::render` already emits `## …` blocks
        // and a preamble, so the model sees the same structure the
        // operator sees on disk.
        let body = mem.render();
        if body.trim().is_empty() {
            return None;
        }
        Some(format!("# Persistent memory\n\n{body}"))
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gc-enrich-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    // ── ContextFileEnricher ──────────────────────────────────────────────

    #[tokio::test]
    async fn context_enricher_emits_when_claude_md_present() {
        let root = tmpdir("ctx-present");
        write_file(&root.join("CLAUDE.md"), "USE MARKDOWN");
        let e = ContextFileEnricher::new(root.clone());
        let body = e.enrich().await.expect("some");
        assert!(body.contains("USE MARKDOWN"));
        assert!(body.contains("context-file:"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn context_enricher_returns_none_when_empty() {
        let root = tmpdir("ctx-empty");
        // .gaussclaw/STOP halts the walk immediately so no ancestor
        // CLAUDE.md leaks into the body.
        std::fs::create_dir_all(root.join(".gaussclaw")).unwrap();
        std::fs::File::create(root.join(".gaussclaw").join("STOP")).unwrap();
        let e = ContextFileEnricher::new(root.clone());
        assert!(e.enrich().await.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn context_enricher_uses_custom_label() {
        let root = tmpdir("ctx-label");
        write_file(&root.join("CLAUDE.md"), "x");
        let e = ContextFileEnricher::new(root.clone()).with_label("project");
        assert_eq!(e.name(), "project");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn context_enricher_honours_custom_finder() {
        let root = tmpdir("ctx-finder");
        write_file(&root.join("PROJECT.md"), "rules");
        write_file(&root.join("CLAUDE.md"), "should-be-ignored");
        let finder = ContextFileFinder::new().with_names(["PROJECT.md".to_string()]);
        let e = ContextFileEnricher::new(root.clone()).with_finder(finder);
        let body = e.enrich().await.unwrap();
        assert!(body.contains("rules"));
        assert!(!body.contains("should-be-ignored"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn context_enricher_silent_on_missing_root() {
        let e =
            ContextFileEnricher::new(PathBuf::from("/this/path/definitely/does/not/exist/xyz123"));
        // Missing roots return Ok(empty) from the walker, which we
        // surface as `None`.
        assert!(e.enrich().await.is_none());
    }

    // ── MarkdownSkillEnricher ────────────────────────────────────────────

    fn write_skill(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        write_file(&dir.join("SKILL.md"), body);
    }

    #[tokio::test]
    async fn markdown_skill_enricher_renders_each_discovered_skill() {
        let root = tmpdir("md-all");
        write_skill(
            &root,
            "alpha",
            "---\ndescription: alpha skill\n---\n\nBody A\n",
        );
        write_skill(
            &root,
            "beta",
            "---\ndescription: beta skill\n---\n\nBody B\n",
        );
        let e = MarkdownSkillEnricher::new(root.clone());
        let body = e.enrich().await.expect("some");
        assert!(body.contains("# Loaded skills"));
        assert!(body.contains("## alpha"));
        assert!(body.contains("## beta"));
        assert!(body.contains("Body A"));
        assert!(body.contains("Body B"));
        // Descriptions appear under each heading.
        assert!(body.contains("_alpha skill_"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn markdown_skill_allowlist_filters() {
        let root = tmpdir("md-allow");
        write_skill(&root, "keep", "body keep\n");
        write_skill(&root, "drop", "body drop\n");
        let e = MarkdownSkillEnricher::new(root.clone()).with_allowlist(["keep".to_owned()]);
        let body = e.enrich().await.unwrap();
        assert!(body.contains("## keep"));
        assert!(!body.contains("## drop"));
        assert!(!body.contains("body drop"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn markdown_skill_returns_none_when_empty() {
        let root = tmpdir("md-empty");
        let e = MarkdownSkillEnricher::new(root.clone());
        assert!(e.enrich().await.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn markdown_skill_allowlist_with_no_matches_yields_none() {
        let root = tmpdir("md-no-match");
        write_skill(&root, "available", "body\n");
        let e =
            MarkdownSkillEnricher::new(root.clone()).with_allowlist(["not-available".to_owned()]);
        assert!(e.enrich().await.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn markdown_skill_skips_missing_description() {
        let root = tmpdir("md-no-desc");
        // No frontmatter — no description.
        write_skill(&root, "raw", "# Plain body\nno frontmatter\n");
        let e = MarkdownSkillEnricher::new(root.clone());
        let body = e.enrich().await.unwrap();
        assert!(body.contains("## raw"));
        // The italic-wrapped description block should NOT appear.
        assert!(!body.contains("__"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── MemoryFileEnricher ───────────────────────────────────────────────

    #[tokio::test]
    async fn memory_enricher_returns_none_when_file_missing() {
        let dir = tmpdir("mem-missing");
        let path = dir.join("MEMORY.md");
        let e = MemoryFileEnricher::new(path);
        assert!(e.enrich().await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn memory_enricher_renders_sections() {
        use gaussclaw_skill::MemoryFile;
        let dir = tmpdir("mem-render");
        let path = dir.join("MEMORY.md");
        let mut m = MemoryFile::new();
        m.upsert_section("User", "Alice prefers concise replies.");
        m.upsert_section("Project", "Rust workspace; cargo + insta snapshots.");
        m.save_to(&path).unwrap();

        let body = MemoryFileEnricher::new(path).enrich().await.unwrap();
        assert!(body.starts_with("# Persistent memory"));
        assert!(body.contains("## User"));
        assert!(body.contains("Alice prefers"));
        assert!(body.contains("## Project"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn memory_enricher_returns_none_for_empty_file() {
        let dir = tmpdir("mem-empty");
        let path = dir.join("MEMORY.md");
        std::fs::File::create(&path).unwrap(); // zero-byte file
        let e = MemoryFileEnricher::new(path);
        assert!(e.enrich().await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn memory_enricher_label_is_customisable() {
        let dir = tmpdir("mem-label");
        let path = dir.join("MEMORY.md");
        let e = MemoryFileEnricher::new(path).with_label("durable");
        assert_eq!(e.name(), "durable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn memory_enricher_uses_default_label() {
        let e = MemoryFileEnricher::new(PathBuf::from("/tmp/nope.md"));
        assert_eq!(e.name(), "memory-file");
    }

    /// The agent loop expects deterministic output: identical disk
    /// state ⇒ identical enrichment bytes (so audit replay stays
    /// stable).
    #[tokio::test]
    async fn memory_enricher_is_deterministic() {
        use gaussclaw_skill::MemoryFile;
        let dir = tmpdir("mem-det");
        let path = dir.join("MEMORY.md");
        let mut m = MemoryFile::new();
        m.upsert_section("A", "one");
        m.upsert_section("B", "two");
        m.save_to(&path).unwrap();
        let e = MemoryFileEnricher::new(path);
        let a = e.enrich().await.unwrap();
        let b = e.enrich().await.unwrap();
        assert_eq!(a, b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
