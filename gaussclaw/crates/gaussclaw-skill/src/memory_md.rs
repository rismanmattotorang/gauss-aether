//! Persistent `MEMORY.md` — cross-session agent scratchpad.
//!
//! OpenHarness (HKUDS/OpenHarness) keeps a single `MEMORY.md` file in
//! the agent's state directory and writes durable knowledge to it
//! across sessions. The file is markdown so it's also human-readable
//! and editable.
//!
//! GaussClaw's port keeps the same wire format but tightens the
//! discipline:
//!
//! 1. **Bounded size.** The whole file is capped at
//!    [`DEFAULT_MEMORY_CAP`] (256 KiB by default). On overflow the
//!    oldest sections are dropped, never the newest — agent knowledge
//!    is append-mostly and the most recent learning is the highest
//!    signal.
//!
//! 2. **Section-structured.** A `MEMORY.md` is a list of `##`
//!    sections; each section has a heading line and a body. The
//!    type-aware API ([`MemoryFile::section`], [`MemoryFile::upsert_section`])
//!    enforces this so the file never degenerates into one giant
//!    paragraph.
//!
//! 3. **Atomic writes.** [`MemoryFile::save_to`] writes to a sibling
//!    `MEMORY.md.tmp` and renames into place, so a crash mid-write
//!    can never leave a corrupted file.
//!
//! 4. **No traversal.** Path argument is taken verbatim; the caller
//!    is responsible for confining it to a safe location (typically
//!    the platform state dir).

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc
)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SkillError;

/// Default size cap on the whole file (256 KiB).
pub const DEFAULT_MEMORY_CAP: usize = 256 * 1024;

/// One parsed `MEMORY.md` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct MemoryFile {
    /// Ordered sections — preserved in insertion order so the file's
    /// human-readable form keeps the agent's chronology.
    pub sections: Vec<MemorySection>,
    /// Optional preamble (text before the first `##` heading).
    pub preamble: String,
    /// Size cap applied on save. Defaults to [`DEFAULT_MEMORY_CAP`].
    pub cap_bytes: usize,
}

/// One memory section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MemorySection {
    /// Heading text (the part after `## `).
    pub heading: String,
    /// Body text (everything until the next `##` or EOF, trimmed).
    pub body: String,
}

impl MemoryFile {
    /// Build an empty file with the default cap.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sections: Vec::new(),
            preamble: String::new(),
            cap_bytes: DEFAULT_MEMORY_CAP,
        }
    }

    /// Builder: set a custom byte cap. Useful for tests and for
    /// operators with tight context windows.
    #[must_use]
    pub const fn with_cap_bytes(mut self, n: usize) -> Self {
        self.cap_bytes = n;
        self
    }

    /// Parse from raw markdown text.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let mut sections: Vec<MemorySection> = Vec::new();
        let mut preamble = String::new();
        let mut current: Option<MemorySection> = None;
        for line in raw.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                if let Some(s) = current.take() {
                    sections.push(s);
                }
                current = Some(MemorySection {
                    heading: rest.trim().to_owned(),
                    body: String::new(),
                });
            } else if let Some(sec) = current.as_mut() {
                if !sec.body.is_empty() {
                    sec.body.push('\n');
                }
                sec.body.push_str(line);
            } else {
                if !preamble.is_empty() {
                    preamble.push('\n');
                }
                preamble.push_str(line);
            }
        }
        if let Some(s) = current {
            sections.push(s);
        }
        for s in &mut sections {
            s.body = s.body.trim().to_owned();
        }
        Self {
            sections,
            preamble: preamble.trim().to_owned(),
            cap_bytes: DEFAULT_MEMORY_CAP,
        }
    }

    /// Read `path` if it exists; return [`MemoryFile::new`] otherwise.
    pub fn load_or_default(path: &Path) -> Result<Self, SkillError> {
        match fs::read_to_string(path) {
            Ok(s) => Ok(Self::parse(&s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(SkillError::InvalidSchema(format!(
                "read MEMORY.md {}: {e}",
                path.display()
            ))),
        }
    }

    /// Render the file back to markdown.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.preamble.is_empty() {
            out.push_str(self.preamble.trim_end());
            out.push_str("\n\n");
        }
        for (i, s) in self.sections.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str("## ");
            out.push_str(&s.heading);
            out.push('\n');
            if !s.body.is_empty() {
                out.push_str(s.body.trim());
                out.push('\n');
            }
        }
        out
    }

    /// Section count.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.sections.len()
    }

    /// `true` when the file has no sections and no preamble.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.sections.is_empty() && self.preamble.is_empty()
    }

    /// Borrow the first section whose heading matches exactly (case-
    /// sensitive). Returns `None` if no such section exists.
    #[must_use]
    pub fn section(&self, heading: &str) -> Option<&MemorySection> {
        self.sections.iter().find(|s| s.heading == heading)
    }

    /// Insert or replace the section with the given heading.
    /// On replace, the section keeps its original position; on insert,
    /// it appends to the end so the chronological order is preserved.
    pub fn upsert_section(&mut self, heading: impl Into<String>, body: impl Into<String>) {
        let h = heading.into();
        let b = body.into();
        if let Some(s) = self.sections.iter_mut().find(|s| s.heading == h) {
            s.body = b;
        } else {
            self.sections.push(MemorySection {
                heading: h,
                body: b,
            });
        }
    }

    /// Remove the section with the given heading. Returns `true` if a
    /// section was removed.
    pub fn remove_section(&mut self, heading: &str) -> bool {
        let before = self.sections.len();
        self.sections.retain(|s| s.heading != heading);
        before != self.sections.len()
    }

    /// Enforce the cap. Drops oldest sections first until the
    /// rendered byte length is ≤ `cap_bytes`. Preamble is kept iff
    /// it alone fits inside the cap; otherwise it's also dropped.
    pub fn enforce_cap(&mut self) -> usize {
        let mut dropped = 0usize;
        while self.render().len() > self.cap_bytes {
            if !self.sections.is_empty() {
                self.sections.remove(0);
                dropped = dropped.saturating_add(1);
            } else if !self.preamble.is_empty() {
                self.preamble.clear();
            } else {
                break;
            }
        }
        dropped
    }

    /// Save to `path` atomically (write to `<path>.tmp` then rename).
    /// Enforces the cap before serialising; returns the number of
    /// dropped sections so callers can warn the operator.
    pub fn save_to(&mut self, path: &Path) -> Result<usize, SkillError> {
        let dropped = self.enforce_cap();
        let body = self.render();
        let tmp = tmp_path(path);
        if let Some(parent) = tmp.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                SkillError::InvalidSchema(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        {
            let mut f = fs::File::create(&tmp)
                .map_err(|e| SkillError::InvalidSchema(format!("create {}: {e}", tmp.display())))?;
            f.write_all(body.as_bytes())
                .map_err(|e| SkillError::InvalidSchema(format!("write {}: {e}", tmp.display())))?;
            f.sync_all()
                .map_err(|e| SkillError::InvalidSchema(format!("fsync {}: {e}", tmp.display())))?;
        }
        fs::rename(&tmp, path).map_err(|e| {
            SkillError::InvalidSchema(format!(
                "rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        Ok(dropped)
    }

    /// Convert to a map indexed by heading. Convenient for callers
    /// that want to surface the memory to the model as a JSON object.
    #[must_use]
    pub fn as_map(&self) -> BTreeMap<String, String> {
        self.sections
            .iter()
            .map(|s| (s.heading.clone(), s.body.clone()))
            .collect()
    }
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".tmp");
    PathBuf::from(p)
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gc-memmd-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    const SAMPLE: &str = "\
Preamble line one.
Preamble line two.

## User
The user's name is Alice. She prefers concise replies.

## Project
Repo is a Rust workspace using cargo + insta snapshots.

## Lessons
Avoid the `unstable-foo` flag; it bricks the linker.
";

    #[test]
    fn from_str_parses_sections_and_preamble() {
        let m = MemoryFile::parse(SAMPLE);
        assert_eq!(m.sections.len(), 3);
        assert!(m.preamble.contains("Preamble line one"));
        assert_eq!(m.sections[0].heading, "User");
        assert!(m.sections[0].body.contains("Alice"));
        assert_eq!(m.sections[2].heading, "Lessons");
    }

    #[test]
    fn empty_input_yields_empty_file() {
        let m = MemoryFile::parse("");
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn section_lookup_is_exact() {
        let m = MemoryFile::parse(SAMPLE);
        assert!(m.section("User").is_some());
        assert!(m.section("user").is_none(), "case-sensitive lookup");
        assert!(m.section("Missing").is_none());
    }

    #[test]
    fn upsert_replaces_existing_in_place() {
        let mut m = MemoryFile::parse(SAMPLE);
        m.upsert_section("User", "Alice now prefers verbose replies.");
        let s = m.section("User").unwrap();
        assert!(s.body.contains("verbose"));
        assert_eq!(m.sections[0].heading, "User", "position preserved");
        assert_eq!(m.sections.len(), 3);
    }

    #[test]
    fn upsert_inserts_new_at_end() {
        let mut m = MemoryFile::parse(SAMPLE);
        m.upsert_section("New", "fresh learning");
        assert_eq!(m.sections.last().unwrap().heading, "New");
        assert_eq!(m.sections.len(), 4);
    }

    #[test]
    fn remove_section_returns_true_only_when_removed() {
        let mut m = MemoryFile::parse(SAMPLE);
        assert!(m.remove_section("Project"));
        assert_eq!(m.sections.len(), 2);
        assert!(!m.remove_section("Project"));
    }

    #[test]
    fn render_round_trip_preserves_sections() {
        let m = MemoryFile::parse(SAMPLE);
        let rendered = m.render();
        let m2 = MemoryFile::parse(&rendered);
        assert_eq!(m.sections, m2.sections);
        assert_eq!(m.preamble, m2.preamble);
    }

    #[test]
    fn enforce_cap_drops_oldest_first() {
        let mut m = MemoryFile::parse(SAMPLE);
        m.cap_bytes = 80; // small enough to force drops
        let dropped = m.enforce_cap();
        assert!(dropped >= 1);
        // The OLDEST section ("User") goes first.
        assert!(m.section("User").is_none());
        // The newest section ("Lessons") is kept until the very end.
        assert!(m.section("Lessons").is_some() || m.sections.is_empty());
    }

    #[test]
    fn enforce_cap_no_op_when_already_under() {
        let mut m = MemoryFile::parse(SAMPLE);
        m.cap_bytes = 10_000;
        let dropped = m.enforce_cap();
        assert_eq!(dropped, 0);
    }

    #[test]
    fn load_or_default_returns_empty_when_missing() {
        let p = tmpdir("missing").join("does-not-exist.md");
        let m = MemoryFile::load_or_default(&p).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn save_and_reload_round_trip() {
        let dir = tmpdir("io");
        let path = dir.join("MEMORY.md");
        let mut m = MemoryFile::parse(SAMPLE);
        let dropped = m.save_to(&path).unwrap();
        assert_eq!(dropped, 0);
        let m2 = MemoryFile::load_or_default(&path).unwrap();
        assert_eq!(m.sections, m2.sections);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_uses_atomic_rename() {
        let dir = tmpdir("atomic");
        let path = dir.join("MEMORY.md");
        let mut m = MemoryFile::new();
        m.upsert_section("First", "body");
        m.save_to(&path).unwrap();
        // No leftover .tmp file should remain after a clean save.
        assert!(!path.with_extension("md.tmp").exists());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn as_map_returns_heading_keyed_map() {
        let m = MemoryFile::parse(SAMPLE);
        let map = m.as_map();
        assert!(map.contains_key("User"));
        assert!(map.get("User").unwrap().contains("Alice"));
    }

    #[test]
    fn cap_enforced_on_save() {
        let dir = tmpdir("cap");
        let path = dir.join("MEMORY.md");
        let mut m = MemoryFile::new().with_cap_bytes(40);
        m.upsert_section("A", "first entry");
        m.upsert_section("B", "second entry");
        m.upsert_section("C", "third entry");
        let dropped = m.save_to(&path).unwrap();
        assert!(dropped >= 1);
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.len() <= 40);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
