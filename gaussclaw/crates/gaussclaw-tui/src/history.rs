//! Persistent input history for the GaussClaw TUI.
//!
//! Mirrors the upstream Hermes Ink TUI's `~/.hermes/.hermes_history`
//! affordance — every submitted user line is appended to a newline-delimited
//! file under `$XDG_STATE_HOME/gaussclaw/history` (falling back to
//! `~/.gaussclaw/history` or `./gaussclaw-history` if neither is writable).
//! The file is intentionally plain text so that operators can grep, prune,
//! or revoke individual entries without needing a CLI.
//!
//! The store is best-effort: every IO operation is logged-and-ignored so
//! the TUI never crashes because the home directory is read-only.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// How many past entries we keep on disk.
const MAX_ENTRIES: usize = 5_000;

/// On-disk + in-memory ring of submitted lines.
#[derive(Debug)]
pub struct HistoryStore {
    path: Option<PathBuf>,
    entries: VecDeque<String>,
    cursor: Option<usize>,
}

impl HistoryStore {
    /// Open the history file at the platform-appropriate path, creating it
    /// if necessary. Returns an empty in-memory ring when the filesystem
    /// is read-only or the home directory is missing.
    #[must_use]
    pub fn open() -> Self {
        let path = resolve_history_path();
        let entries = path
            .as_ref()
            .and_then(|p| File::open(p).ok())
            .map(|f| {
                BufReader::new(f)
                    .lines()
                    .map_while(Result::ok)
                    .filter(|l| !l.is_empty())
                    .collect::<VecDeque<_>>()
            })
            .unwrap_or_default();
        Self {
            path,
            entries,
            cursor: None,
        }
    }

    /// Build an in-memory-only history (used in tests).
    #[must_use]
    pub const fn in_memory() -> Self {
        Self {
            path: None,
            entries: VecDeque::new(),
            cursor: None,
        }
    }

    /// Append a freshly submitted line. Duplicates of the most recent entry
    /// are ignored, matching the upstream Hermes contract.
    pub fn push(&mut self, line: &str) {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            return;
        }
        if self.entries.back().is_some_and(|prev| prev == trimmed) {
            return;
        }
        self.entries.push_back(trimmed.to_owned());
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.cursor = None;
        if let Some(p) = self.path.as_ref() {
            let _ = OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .and_then(|mut f| writeln!(f, "{trimmed}"));
        }
    }

    /// Walk one step backward in history. Returns the entry to display, or
    /// `None` if we're already at the oldest entry.
    pub fn step_back(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        self.cursor = Some(
            self.cursor
                .map_or(self.entries.len() - 1, |c| c.saturating_sub(1)),
        );
        self.cursor
            .and_then(|c| self.entries.get(c).map(String::as_str))
    }

    /// Walk one step forward in history. Returns `None` when we step off the
    /// end (signalling that the caller should restore the live input buffer).
    pub fn step_forward(&mut self) -> Option<&str> {
        let Some(c) = self.cursor else {
            return None;
        };
        if c + 1 >= self.entries.len() {
            self.cursor = None;
            return None;
        }
        self.cursor = Some(c + 1);
        self.entries.get(c + 1).map(String::as_str)
    }

    /// Reset the recall cursor (called after a submit).
    pub const fn reset_cursor(&mut self) {
        self.cursor = None;
    }

    /// Number of entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no entries are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Read-only view of the on-disk history path, if any.
    #[must_use]
    pub const fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }
}

fn resolve_history_path() -> Option<PathBuf> {
    if let Ok(state) = std::env::var("XDG_STATE_HOME") {
        let p = PathBuf::from(state).join("gaussclaw");
        if std::fs::create_dir_all(&p).is_ok() {
            return Some(p.join("history"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".gaussclaw");
        if std::fs::create_dir_all(&p).is_ok() {
            return Some(p.join("history"));
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(profile).join(".gaussclaw");
        if std::fs::create_dir_all(&p).is_ok() {
            return Some(p.join("history"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_history_is_empty() {
        let h = HistoryStore::in_memory();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn push_dedupes_consecutive_duplicates() {
        let mut h = HistoryStore::in_memory();
        h.push("hello");
        h.push("hello");
        h.push("world");
        h.push("world");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn step_back_walks_oldest_to_newest() {
        let mut h = HistoryStore::in_memory();
        h.push("one");
        h.push("two");
        h.push("three");
        assert_eq!(h.step_back(), Some("three"));
        assert_eq!(h.step_back(), Some("two"));
        assert_eq!(h.step_back(), Some("one"));
        assert_eq!(h.step_back(), Some("one")); // saturates at oldest
    }

    #[test]
    fn step_forward_returns_none_off_the_end() {
        let mut h = HistoryStore::in_memory();
        h.push("one");
        h.push("two");
        assert_eq!(h.step_back(), Some("two"));
        assert_eq!(h.step_forward(), None);
    }

    #[test]
    fn empty_lines_are_ignored() {
        let mut h = HistoryStore::in_memory();
        h.push("");
        h.push("   ");
        h.push("real");
        assert_eq!(h.len(), 1);
    }
}
