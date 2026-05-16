//! Snapshot / delta encoding (Phase 2).
//!
//! Phase 2 ships a small Myers-style line diff over plain text transcripts.
//! It is deliberately not the production ADT diff — that lands in Phase 6
//! alongside the K-LRU prefix tree. The Phase-2 implementation exists so the
//! Differential Turn Engine has a real delta primitive to exercise, and so
//! Theorem T12's warm-cache regime has a benchmark target.
//!
//! ## Algorithm
//!
//! Computes the longest-common-subsequence over **lines** of two strings,
//! then emits a sequence of [`DiffOp`]s that reconstruct `next` from `prev`.
//! The implementation is dynamic programming (`O(n·m)` time, `O(n·m)`
//! space). Acceptable for transcripts of a few hundred lines; Phase 6 will
//! swap in a true Myers `O((n+m)·d)` implementation.

// The DP indices i, j are decremented under guards `i > 0` / `j > 0`, and
// array accesses use `i - 1` / `j - 1` under the same guards. These are
// safe by construction; we silence the side-effects lint locally.
#![allow(clippy::arithmetic_side_effects, clippy::needless_range_loop)]

use serde::{Deserialize, Serialize};

/// One delta operation in the line-level diff.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DiffOp {
    /// A run of `count` lines is identical between `prev` and `next`.
    Keep {
        /// Number of consecutive lines retained.
        count: usize,
    },
    /// A run of `count` lines from `prev` is dropped.
    Delete {
        /// Number of lines deleted from `prev`.
        count: usize,
    },
    /// One line was inserted at this position in `next`.
    Insert {
        /// The inserted line (without the trailing newline).
        line: String,
    },
}

/// Compute a line-level delta from `prev` to `next`.
///
/// The result, applied to `prev` via [`apply`], yields a byte-identical copy
/// of `next` modulo trailing-newline normalisation.
#[must_use]
pub fn diff(prev: &str, next: &str) -> Vec<DiffOp> {
    let prev_lines: Vec<&str> = prev.lines().collect();
    let next_lines: Vec<&str> = next.lines().collect();
    let lcs = lcs_matrix(&prev_lines, &next_lines);
    let mut ops = Vec::new();
    let mut i = prev_lines.len();
    let mut j = next_lines.len();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && prev_lines[i - 1] == next_lines[j - 1] {
            ops.push(DiffOp::Keep { count: 1 });
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || lcs[i][j - 1] >= lcs[i - 1][j]) {
            ops.push(DiffOp::Insert {
                line: next_lines[j - 1].to_owned(),
            });
            j -= 1;
        } else {
            ops.push(DiffOp::Delete { count: 1 });
            i -= 1;
        }
    }
    ops.reverse();
    coalesce(ops)
}

/// Reconstruct `next` from `prev` and a delta.
///
/// # Errors
/// Returns an error string when the delta references more lines than `prev`
/// contains (a sign that the delta and the base diverged).
pub fn apply(prev: &str, ops: &[DiffOp]) -> Result<String, ApplyError> {
    let prev_lines: Vec<&str> = prev.lines().collect();
    let mut cursor = 0usize;
    let mut out_lines: Vec<String> = Vec::new();
    for op in ops {
        match op {
            DiffOp::Keep { count } => {
                if cursor.saturating_add(*count) > prev_lines.len() {
                    return Err(ApplyError::OutOfRange);
                }
                for k in 0..*count {
                    out_lines.push(prev_lines[cursor.saturating_add(k)].to_owned());
                }
                cursor = cursor.saturating_add(*count);
            }
            DiffOp::Delete { count } => {
                if cursor.saturating_add(*count) > prev_lines.len() {
                    return Err(ApplyError::OutOfRange);
                }
                cursor = cursor.saturating_add(*count);
            }
            DiffOp::Insert { line } => out_lines.push(line.clone()),
        }
    }
    Ok(out_lines.join("\n"))
}

/// Error returned by [`apply`] when the delta references lines outside `prev`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ApplyError {
    /// Delta references more lines than the base contains.
    OutOfRange,
}

impl core::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OutOfRange => f.write_str("delta references lines outside prev"),
        }
    }
}

impl std::error::Error for ApplyError {}

fn lcs_matrix(a: &[&str], b: &[&str]) -> Vec<Vec<u32>> {
    let n = a.len();
    let m = b.len();
    let mut lcs = vec![vec![0u32; m.saturating_add(1)]; n.saturating_add(1)];
    for i in 1..=n {
        for j in 1..=m {
            lcs[i][j] = if a[i - 1] == b[j - 1] {
                lcs[i - 1][j - 1].saturating_add(1)
            } else {
                lcs[i - 1][j].max(lcs[i][j - 1])
            };
        }
    }
    lcs
}

/// Merge adjacent same-kind ops to compress the delta.
fn coalesce(ops: Vec<DiffOp>) -> Vec<DiffOp> {
    let mut out: Vec<DiffOp> = Vec::with_capacity(ops.len());
    for op in ops {
        // Arms have identical bodies but distinct patterns; keep separate
        // so future variants don't accidentally merge.
        #[allow(clippy::match_same_arms)]
        match (out.last_mut(), op) {
            (Some(DiffOp::Keep { count }), DiffOp::Keep { count: inc }) => {
                *count = count.saturating_add(inc);
            }
            (Some(DiffOp::Delete { count }), DiffOp::Delete { count: inc }) => {
                *count = count.saturating_add(inc);
            }
            (_, op) => out.push(op),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_to_empty_yields_no_ops() {
        let d = diff("", "");
        assert!(d.is_empty());
        assert_eq!(apply("", &d).unwrap(), "");
    }

    #[test]
    fn identical_inputs_collapse_to_keep() {
        let s = "hello\nworld";
        let d = diff(s, s);
        assert_eq!(d, vec![DiffOp::Keep { count: 2 }]);
        assert_eq!(apply(s, &d).unwrap(), s);
    }

    #[test]
    fn pure_insertion_is_emitted_as_inserts() {
        let d = diff("", "a\nb");
        // Two inserts, possibly interleaved with no-op keeps.
        let inserts = d
            .iter()
            .filter(|op| matches!(op, DiffOp::Insert { .. }))
            .count();
        assert_eq!(inserts, 2);
        assert_eq!(apply("", &d).unwrap(), "a\nb");
    }

    #[test]
    fn pure_deletion_is_emitted_as_deletes() {
        let d = diff("a\nb\nc", "");
        let delete_total: usize = d
            .iter()
            .filter_map(|op| {
                if let DiffOp::Delete { count } = op {
                    Some(*count)
                } else {
                    None
                }
            })
            .sum();
        assert_eq!(delete_total, 3);
        assert_eq!(apply("a\nb\nc", &d).unwrap(), "");
    }

    #[test]
    fn round_trip_on_a_realistic_transcript() {
        let prev = "user: hi\nagent: hello\nuser: what time is it?";
        let next = "user: hi\nagent: hello\nagent: 3 pm\nuser: thanks";
        let d = diff(prev, next);
        let reconstructed = apply(prev, &d).unwrap();
        assert_eq!(reconstructed, next);
    }

    #[test]
    fn apply_rejects_out_of_range_delete() {
        let err = apply("a", &[DiffOp::Delete { count: 5 }]).unwrap_err();
        assert_eq!(err, ApplyError::OutOfRange);
    }
}
