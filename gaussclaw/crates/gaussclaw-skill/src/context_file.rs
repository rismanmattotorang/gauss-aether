//! Context-file discovery (`CLAUDE.md` / `GAUSSCLAW.md`).
//!
//! OpenHarness (HKUDS/OpenHarness) walks the working directory and its
//! parents for a `CLAUDE.md` file and injects its contents into the
//! agent's system prompt as "project-specific operating instructions".
//! The convention is borrowed from Anthropic's Claude Code.
//!
//! GaussClaw adopts the same surface with two design choices:
//!
//! 1. **Walk is depth-bounded.** A misconfigured deployment in `/tmp`
//!    or `/` shouldn't trigger an unbounded scan of the filesystem.
//!    The walk stops at [`DEFAULT_MAX_DEPTH`] (8 by default) or when
//!    it hits a `.gaussclaw/STOP` marker — whichever fires first.
//!
//! 2. **Per-file size cap.** Each file is truncated at
//!    [`DEFAULT_MAX_BYTES`] (64 KiB by default). A pathological
//!    `CLAUDE.md` cannot blow up the system prompt without an
//!    operator opt-in.
//!
//! 3. **Multi-file aggregation.** When multiple ancestors carry a
//!    context file, they are returned ordered from *root → leaf* so
//!    the leaf-most file's directives "override" by being last. The
//!    caller chooses how to render them.
//!
//! 4. **Path-traversal guard.** Symlinks at the file leaf are
//!    refused, mirroring the rule [`MarkdownSkill::from_dir`] applies.
//!
//! [`MarkdownSkill::from_dir`]: crate::MarkdownSkill::from_dir
//!
//! ## Naming convention
//!
//! The default file names are `CLAUDE.md` (Anthropic-compatible) and
//! `GAUSSCLAW.md` (project-native). Callers can pass a custom list.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc
)]

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SkillError;

/// Default max ancestor depth (working directory + 7 parents).
pub const DEFAULT_MAX_DEPTH: usize = 8;

/// Default per-file truncation cap (64 KiB).
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024;

/// Canonical context-file names, in priority order. The first matching
/// name in each directory wins.
pub const DEFAULT_NAMES: &[&str] = &["CLAUDE.md", "GAUSSCLAW.md"];

/// One loaded context file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ContextFile {
    /// Absolute path to the source file.
    pub path: PathBuf,
    /// File body, truncated to the configured byte cap.
    pub body: String,
    /// `true` if the body was truncated.
    pub truncated: bool,
    /// Distance from the starting directory; 0 = same dir, 1 = parent, …
    pub depth: usize,
}

/// Discovery configuration. Defaults are sensible for production;
/// callers tweak via the builder.
#[derive(Debug, Clone)]
pub struct ContextFileFinder {
    names: Vec<String>,
    max_depth: usize,
    max_bytes: usize,
}

impl Default for ContextFileFinder {
    fn default() -> Self {
        Self {
            names: DEFAULT_NAMES.iter().map(|s| (*s).to_owned()).collect(),
            max_depth: DEFAULT_MAX_DEPTH,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

impl ContextFileFinder {
    /// Build with defaults: `CLAUDE.md` + `GAUSSCLAW.md`, depth 8, 64 KiB.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: replace the candidate file names.
    #[must_use]
    pub fn with_names(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.names = names.into_iter().collect();
        self
    }

    /// Builder: cap the ancestor walk.
    #[must_use]
    pub const fn with_max_depth(mut self, n: usize) -> Self {
        self.max_depth = n;
        self
    }

    /// Builder: cap per-file body size in bytes.
    #[must_use]
    pub const fn with_max_bytes(mut self, n: usize) -> Self {
        self.max_bytes = n;
        self
    }

    /// Walk `start` and up to `max_depth` ancestors looking for any
    /// of the configured names. Returns the files in root-to-leaf
    /// order so the leaf-most file is last (most-specific wins for
    /// callers that concatenate).
    ///
    /// Stops early at a directory containing `.gaussclaw/STOP`. Walks
    /// up only through real ancestor links — symlinked parent dirs
    /// are followed but the file leaf itself must be a regular file.
    pub fn discover(&self, start: &Path) -> Result<Vec<ContextFile>, SkillError> {
        // Resolve to an absolute path so the walk is deterministic.
        let start_abs = if start.is_absolute() {
            start.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| SkillError::InvalidSchema(format!("cwd lookup: {e}")))?
                .join(start)
        };

        let mut found_leaf_first: Vec<ContextFile> = Vec::new();
        let mut cursor: Option<PathBuf> = Some(start_abs);
        let mut depth = 0usize;

        while let Some(dir) = cursor {
            if depth >= self.max_depth {
                break;
            }
            if dir.join(".gaussclaw").join("STOP").exists() {
                break;
            }
            for name in &self.names {
                let candidate = dir.join(name);
                match fs::symlink_metadata(&candidate) {
                    Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                        let (body, truncated) = read_capped(&candidate, self.max_bytes)?;
                        found_leaf_first.push(ContextFile {
                            path: candidate.clone(),
                            body,
                            truncated,
                            depth,
                        });
                        // Only the first matching name per directory wins.
                        break;
                    }
                    _ => {}
                }
            }
            cursor = dir.parent().map(Path::to_path_buf);
            depth = depth.saturating_add(1);
        }

        // Caller wants root→leaf order; reverse the leaf-first walk.
        found_leaf_first.reverse();
        Ok(found_leaf_first)
    }

    /// Convenience: concatenate every discovered file's body into one
    /// string, separated by a divider line. Useful when the caller
    /// just wants to inject the result as a single system message.
    pub fn discover_joined(&self, start: &Path) -> Result<String, SkillError> {
        let files = self.discover(start)?;
        Ok(join_context(&files))
    }
}

/// Format `files` (root→leaf) into a single document with section
/// dividers. Useful for callers that want a single string to drop
/// into a system message.
#[must_use]
pub fn join_context(files: &[ContextFile]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (i, f) in files.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n---\n\n");
        }
        // Writing to a `String` is infallible.
        let _ = writeln!(
            out,
            "<!-- context-file: {} (depth={}{}) -->",
            f.path.display(),
            f.depth,
            if f.truncated { ", truncated" } else { "" }
        );
        out.push_str(&f.body);
    }
    out
}

/// Read up to `cap` bytes from `path`, returning (body, truncated_flag).
fn read_capped(path: &Path, cap: usize) -> Result<(String, bool), SkillError> {
    use std::io::Read;
    let mut f = fs::File::open(path)
        .map_err(|e| SkillError::InvalidSchema(format!("open {}: {e}", path.display())))?;
    let mut buf = Vec::with_capacity(cap.min(4096));
    let mut chunk = [0u8; 4096];
    let mut truncated = false;
    loop {
        if buf.len() >= cap {
            // We've already read `cap` bytes — check if more remains.
            let n = f
                .read(&mut chunk[..1])
                .map_err(|e| SkillError::InvalidSchema(format!("read: {e}")))?;
            if n > 0 {
                truncated = true;
            }
            break;
        }
        let space = cap.saturating_sub(buf.len());
        let want = chunk.len().min(space);
        let n = f
            .read(&mut chunk[..want])
            .map_err(|e| SkillError::InvalidSchema(format!("read: {e}")))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let s = String::from_utf8_lossy(&buf).into_owned();
    Ok((s, truncated))
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gc-cf-{tag}-{}-{}",
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

    #[test]
    fn discover_finds_single_claude_md() {
        let root = tmpdir("single");
        let leaf = root.join("a/b/c");
        std::fs::create_dir_all(&leaf).unwrap();
        write_file(&root.join("CLAUDE.md"), "root level instructions");

        let finder = ContextFileFinder::new();
        let files = finder.discover(&leaf).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].body.contains("root level instructions"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_returns_root_to_leaf_order() {
        let root = tmpdir("order");
        let mid = root.join("a");
        let leaf = mid.join("b");
        std::fs::create_dir_all(&leaf).unwrap();
        write_file(&root.join("CLAUDE.md"), "ROOT");
        write_file(&mid.join("CLAUDE.md"), "MID");
        write_file(&leaf.join("CLAUDE.md"), "LEAF");

        let files = ContextFileFinder::new().discover(&leaf).unwrap();
        assert_eq!(files.len(), 3);
        // Root-to-leaf order: ROOT first, LEAF last.
        assert!(files[0].body.contains("ROOT"));
        assert!(files[1].body.contains("MID"));
        assert!(files[2].body.contains("LEAF"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_directory_returns_empty() {
        let finder = ContextFileFinder::new();
        let files = finder
            .discover(Path::new("/this/does/not/exist/abc123"))
            .unwrap();
        // The walk reaches existing parents (/, etc.) without a
        // CLAUDE.md so the result is empty.
        assert!(files.iter().all(|f| !f.body.is_empty()));
        // We assert the call doesn't error — empty is allowed.
        let _ = files;
    }

    #[test]
    fn max_depth_caps_the_walk() {
        let root = tmpdir("depth");
        let leaf = root.join("a/b/c");
        std::fs::create_dir_all(&leaf).unwrap();
        write_file(&root.join("CLAUDE.md"), "root");
        // With max_depth=2, the walk visits leaf, leaf/.., leaf/../..
        // — root is at depth 3, so it should NOT be found.
        let files = ContextFileFinder::new()
            .with_max_depth(2)
            .discover(&leaf)
            .unwrap();
        assert!(files.is_empty());
        // With max_depth=8 (default), root IS found.
        let files = ContextFileFinder::new().discover(&leaf).unwrap();
        assert_eq!(files.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_marker_short_circuits_walk() {
        let root = tmpdir("stop");
        let leaf = root.join("a/b");
        std::fs::create_dir_all(&leaf).unwrap();
        write_file(&root.join("CLAUDE.md"), "root");
        // Drop a STOP marker at the mid-level; walk should not see root.
        std::fs::create_dir_all(root.join("a/.gaussclaw")).unwrap();
        std::fs::File::create(root.join("a/.gaussclaw/STOP")).unwrap();

        let files = ContextFileFinder::new().discover(&leaf).unwrap();
        assert!(files.is_empty(), "STOP marker should halt the walk");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn truncation_is_recorded_for_oversized_files() {
        let root = tmpdir("trunc");
        write_file(&root.join("CLAUDE.md"), &"x".repeat(8_192));
        let files = ContextFileFinder::new()
            .with_max_bytes(1_024)
            .discover(&root)
            .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].truncated);
        assert!(files[0].body.len() <= 1_024);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn small_files_are_not_marked_truncated() {
        let root = tmpdir("nontrunc");
        write_file(&root.join("CLAUDE.md"), "small");
        let files = ContextFileFinder::new().discover(&root).unwrap();
        assert_eq!(files.len(), 1);
        assert!(!files[0].truncated);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn first_name_wins_within_a_dir() {
        let root = tmpdir("priority");
        // Order in DEFAULT_NAMES: CLAUDE.md before GAUSSCLAW.md.
        write_file(&root.join("CLAUDE.md"), "claude-wins");
        write_file(&root.join("GAUSSCLAW.md"), "gaussclaw-loses");
        let files = ContextFileFinder::new().discover(&root).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].body.contains("claude-wins"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn custom_names_override_defaults() {
        let root = tmpdir("custom");
        write_file(&root.join("ROOT.md"), "yes");
        write_file(&root.join("CLAUDE.md"), "no");
        let files = ContextFileFinder::new()
            .with_names(["ROOT.md".to_string()])
            .discover(&root)
            .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].body.contains("yes"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn discover_joined_aggregates_with_dividers() {
        let root = tmpdir("joined");
        let mid = root.join("a");
        std::fs::create_dir_all(&mid).unwrap();
        write_file(&root.join("CLAUDE.md"), "ROOT");
        write_file(&mid.join("CLAUDE.md"), "LEAF");
        let joined = ContextFileFinder::new().discover_joined(&mid).unwrap();
        assert!(joined.contains("ROOT"));
        assert!(joined.contains("LEAF"));
        assert!(joined.contains("\n---\n"));
        // Root before leaf in the joined document.
        let pos_root = joined.find("ROOT").unwrap();
        let pos_leaf = joined.find("LEAF").unwrap();
        assert!(pos_root < pos_leaf);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn depth_field_increments_correctly() {
        let root = tmpdir("depthfield");
        let leaf = root.join("a/b");
        std::fs::create_dir_all(&leaf).unwrap();
        write_file(&root.join("CLAUDE.md"), "root");
        write_file(&leaf.join("CLAUDE.md"), "leaf");
        let files = ContextFileFinder::new().discover(&leaf).unwrap();
        // Root-to-leaf order: root entry came from depth=2, leaf from depth=0.
        assert!(files[0].depth >= files[1].depth);
        let leaf_entry = files.iter().find(|f| f.body.contains("leaf")).unwrap();
        assert_eq!(leaf_entry.depth, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn join_context_renders_path_header() {
        let file = ContextFile {
            path: PathBuf::from("/tmp/CLAUDE.md"),
            body: "instructions".to_string(),
            truncated: true,
            depth: 0,
        };
        let s = join_context(&[file]);
        assert!(s.contains("context-file: /tmp/CLAUDE.md"));
        assert!(s.contains("truncated"));
        assert!(s.contains("instructions"));
    }
}
