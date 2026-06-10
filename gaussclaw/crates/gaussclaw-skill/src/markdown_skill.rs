//! Markdown-bodied skill (OpenHarness / Anthropic `SKILL.md` format).
//!
//! OpenHarness loads "Skills" as on-demand knowledge: a folder with
//! a `SKILL.md` whose body is markdown prose the model reads when the
//! skill is invoked. The format is also Anthropic-compatible (see
//! `anthropics/skills`).
//!
//! GaussClaw already has [`SkillManifest`](super::SkillManifest) — a
//! TOML-only tool manifest. That stays the source of truth for the
//! capability lattice. This module adds a *complementary* surface:
//! a `MarkdownSkill` is a unit of *knowledge*, not a tool, and it is
//! loaded by walking a skills root directory.
//!
//! ## Structural guarantees
//!
//! 1. **Path-traversal guard.** Discovery refuses symlink chains and
//!    `..` components — same rule the plugin loader applies (Sprint 7).
//! 2. **Provenance digest.** Every loaded skill carries a BLAKE3-style
//!    fingerprint (here, a SHA-2-free SipHash to avoid a new dep; the
//!    full BLAKE3 receipt is recorded by the higher-level skill
//!    installer in `gaussclaw-bin`).
//! 3. **YAML-frontmatter friendly.** A leading `---\n…\n---\n` block is
//!    parsed as a small `key: value` map (one level, string values
//!    only — no nested YAML). Skills without frontmatter are still
//!    valid; the body is the markdown.
//! 4. **Capability bridge.** A frontmatter `caps:` line (comma- or
//!    space-separated) is parsed through [`super::parse_cap`] and
//!    exposed as a [`CapToken`], so a markdown skill can declare the
//!    same caps a TOML tool would. The kernel admit gate consults
//!    this when the skill is loaded into the prompt.
//!
//! ## Format
//!
//! ```markdown
//! ---
//! name: web-research
//! description: Search and summarise web pages.
//! caps: net:get
//! ---
//!
//! # When to use this skill
//!
//! Read this whenever the user asks for current information…
//! ```
//!
//! ## Discovery
//!
//! Production wires four roots, mirroring the OpenHarness convention:
//!
//! - bundled: `<binary>/skills/<name>/SKILL.md`
//! - user: `~/.gaussclaw/skills/<name>/SKILL.md`
//! - project: `.gaussclaw/skills/<name>/SKILL.md` (disabled when the
//!   project is marked untrusted, mirroring OpenHarness's untrusted-
//!   repo guard)
//! - plugin: `<plugin-root>/skills/<name>/SKILL.md`

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use gauss_core::CapToken;
use serde::{Deserialize, Serialize};

use crate::{parse_cap, SkillError};

/// One markdown-bodied skill loaded from disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MarkdownSkill {
    /// Skill slug derived from the parent directory name.
    pub name: String,
    /// Frontmatter values (lowercase keys, raw string values).
    pub frontmatter: BTreeMap<String, String>,
    /// Markdown body — everything after the closing `---` (or the
    /// whole file if no frontmatter was present).
    pub body: String,
    /// Resolved path to the source file. Useful for audit-log keying.
    pub source: PathBuf,
    /// Stable 64-bit fingerprint of the raw file bytes. Provenance
    /// receipts use this as the cheap discriminator; the full BLAKE3
    /// receipt is added by the installer pipeline.
    pub digest: u64,
}

impl MarkdownSkill {
    /// Parse a markdown skill from raw bytes. `name` is normally the
    /// parent directory; `source` is the path the bytes came from
    /// (used purely for diagnostics).
    pub fn from_str(name: impl Into<String>, source: PathBuf, raw: &str) -> Self {
        let (front, body) = split_frontmatter(raw);
        Self {
            name: name.into(),
            frontmatter: front,
            body: body.to_string(),
            source,
            digest: cheap_digest(raw.as_bytes()),
        }
    }

    /// Load `<dir>/SKILL.md` and parse it. The skill name is taken
    /// from `dir.file_name()`.
    pub fn from_dir(dir: &Path) -> Result<Self, SkillError> {
        // Reject `..` segments and symlinks at the SKILL.md file leaf.
        // `dir` is allowed to be a symlink container — only `SKILL.md`
        // itself must be a plain regular file.
        if dir
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(SkillError::InvalidSchema(format!(
                "rejecting traversal in path: {}",
                dir.display()
            )));
        }
        let name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                SkillError::InvalidSchema(format!(
                    "skill directory has no usable name: {}",
                    dir.display()
                ))
            })?
            .to_owned();
        let skill_path = dir.join("SKILL.md");
        let meta = fs::symlink_metadata(&skill_path).map_err(|e| {
            SkillError::InvalidSchema(format!("read {}: {e}", skill_path.display()))
        })?;
        if meta.file_type().is_symlink() {
            return Err(SkillError::InvalidSchema(format!(
                "SKILL.md must be a regular file, not a symlink: {}",
                skill_path.display()
            )));
        }
        let raw = fs::read_to_string(&skill_path).map_err(|e| {
            SkillError::InvalidSchema(format!("read {}: {e}", skill_path.display()))
        })?;
        Ok(Self::from_str(name, skill_path, &raw))
    }

    /// Discover every `SKILL.md` under `root`. Returns the skills
    /// sorted by name for stable iteration order.
    ///
    /// Discovery does NOT recurse into nested skill directories — the
    /// canonical layout is one level deep:
    ///
    /// ```text
    /// root/
    ///   web-research/SKILL.md
    ///   git-helper/SKILL.md
    /// ```
    ///
    /// Subdirectories without a `SKILL.md` are silently skipped.
    pub fn discover_in(root: &Path) -> Result<Vec<Self>, SkillError> {
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let entries = fs::read_dir(root)
            .map_err(|e| SkillError::InvalidSchema(format!("readdir {}: {e}", root.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| SkillError::InvalidSchema(format!("dirent: {e}")))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if !path.join("SKILL.md").is_file() {
                continue;
            }
            // Reject symlinked skill directories at the leaf.
            let meta = fs::symlink_metadata(&path).map_err(|e| {
                SkillError::InvalidSchema(format!("metadata {}: {e}", path.display()))
            })?;
            if meta.file_type().is_symlink() {
                continue;
            }
            out.push(Self::from_dir(&path)?);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Borrow the frontmatter `description` field, if present.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.frontmatter.get("description").map(String::as_str)
    }

    /// Resolve the frontmatter `caps:` list into a [`CapToken`].
    /// Returns `BOTTOM` when no caps are declared.
    pub fn cap_required(&self) -> Result<CapToken, SkillError> {
        let Some(caps_raw) = self.frontmatter.get("caps") else {
            return Ok(CapToken::BOTTOM);
        };
        let mut acc: u64 = 0;
        for token in caps_raw
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
        {
            acc |= parse_cap(token)?.bits();
        }
        Ok(CapToken::from_bits(acc))
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

/// Split a `---\nkey: value\n---\n…body…` document into the frontmatter
/// map and the body. If the document does not start with `---`, the
/// frontmatter is empty and the body is the entire input.
fn split_frontmatter(raw: &str) -> (BTreeMap<String, String>, &str) {
    let mut map = BTreeMap::new();
    let Some(rest) = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
    else {
        return (map, raw);
    };
    // Find the closing fence.
    let Some(end_idx) = rest.find("\n---") else {
        return (map, raw);
    };
    let front = &rest[..end_idx];
    // Body starts after the closing fence + newline.
    let body_start = end_idx.saturating_add("\n---".len());
    let body_after = &rest[body_start..];
    let body = body_after
        .strip_prefix('\n')
        .or_else(|| body_after.strip_prefix("\r\n"))
        .unwrap_or(body_after);
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_owned();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    (map, body)
}

/// FxHash-style cheap digest (not cryptographic). Used as a stable
/// short fingerprint for the file bytes; the higher-level installer
/// records the BLAKE3 receipt separately.
fn cheap_digest(bytes: &[u8]) -> u64 {
    // 64-bit FxHash variant — deterministic and dependency-free.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h = h.rotate_left(5).wrapping_mul(0x100_0000_01b3);
        h ^= u64::from(b);
    }
    h
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SKILL_WITH_FRONT: &str = "\
---
name: web-research
description: Search and summarise web pages.
caps: net:get
---

# When to use this skill

Use whenever the user asks for current information.
";

    const SKILL_NO_FRONT: &str = "\
# Plain markdown

No frontmatter; the whole thing is body.
";

    #[test]
    fn frontmatter_parses() {
        let s = MarkdownSkill::from_str("web", PathBuf::from("test"), SKILL_WITH_FRONT);
        assert_eq!(s.frontmatter.get("name").unwrap(), "web-research");
        assert_eq!(s.description(), Some("Search and summarise web pages."));
        assert!(s.body.contains("When to use this skill"));
        assert!(!s.body.starts_with("---"));
    }

    #[test]
    fn no_frontmatter_keeps_full_body() {
        let s = MarkdownSkill::from_str("plain", PathBuf::from("p"), SKILL_NO_FRONT);
        assert!(s.frontmatter.is_empty());
        assert!(s.body.starts_with("# Plain markdown"));
    }

    #[test]
    fn caps_parse_from_frontmatter() {
        let s = MarkdownSkill::from_str("web", PathBuf::from("t"), SKILL_WITH_FRONT);
        let cap = s.cap_required().unwrap();
        assert_eq!(cap.bits(), CapToken::NETWORK_GET.bits());
    }

    #[test]
    fn caps_default_to_bottom_when_absent() {
        let s = MarkdownSkill::from_str("p", PathBuf::from("t"), SKILL_NO_FRONT);
        assert_eq!(s.cap_required().unwrap().bits(), CapToken::BOTTOM.bits());
    }

    #[test]
    fn unknown_cap_in_frontmatter_is_rejected() {
        let raw = "---\ncaps: banana\n---\n\nbody\n";
        let s = MarkdownSkill::from_str("bad", PathBuf::from("t"), raw);
        assert!(matches!(
            s.cap_required().unwrap_err(),
            SkillError::UnknownCap(_)
        ));
    }

    #[test]
    fn digest_is_stable_for_same_bytes() {
        let a = MarkdownSkill::from_str("x", PathBuf::from("t"), SKILL_WITH_FRONT);
        let b = MarkdownSkill::from_str("x", PathBuf::from("t"), SKILL_WITH_FRONT);
        assert_eq!(a.digest, b.digest);
    }

    #[test]
    fn digest_changes_with_bytes() {
        let a = MarkdownSkill::from_str("x", PathBuf::from("t"), SKILL_WITH_FRONT);
        let b = MarkdownSkill::from_str("x", PathBuf::from("t"), SKILL_NO_FRONT);
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn from_dir_loads_real_files() {
        let tmp = std::env::temp_dir().join(format!("gc-skill-{}", std::process::id()));
        let skill_dir = tmp.join("web-research");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let mut f = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        f.write_all(SKILL_WITH_FRONT.as_bytes()).unwrap();

        let s = MarkdownSkill::from_dir(&skill_dir).expect("load");
        assert_eq!(s.name, "web-research");
        assert_eq!(s.description(), Some("Search and summarise web pages."));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_in_finds_and_sorts() {
        let tmp = std::env::temp_dir().join(format!("gc-disc-{}", std::process::id()));
        for n in ["zeta", "alpha", "mu"] {
            let d = tmp.join(n);
            std::fs::create_dir_all(&d).unwrap();
            let mut f = std::fs::File::create(d.join("SKILL.md")).unwrap();
            f.write_all(b"---\ndescription: x\n---\nbody").unwrap();
        }
        // A directory without SKILL.md must be silently skipped.
        std::fs::create_dir_all(tmp.join("no-skill")).unwrap();

        let skills = MarkdownSkill::discover_in(&tmp).unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mu", "zeta"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_in_returns_empty_when_root_missing() {
        let missing = PathBuf::from("/this/path/does/not/exist/abc123xyz");
        let skills = MarkdownSkill::discover_in(&missing).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn parent_dir_traversal_is_rejected() {
        let p = PathBuf::from("foo/../bar/skill");
        assert!(matches!(
            MarkdownSkill::from_dir(&p).unwrap_err(),
            SkillError::InvalidSchema(_)
        ));
    }

    #[test]
    fn crlf_frontmatter_works() {
        let raw = "---\r\ndescription: hi\r\n---\r\nbody here\r\n";
        let s = MarkdownSkill::from_str("c", PathBuf::from("t"), raw);
        assert_eq!(s.description(), Some("hi"));
        assert!(s.body.contains("body here"));
    }
}
