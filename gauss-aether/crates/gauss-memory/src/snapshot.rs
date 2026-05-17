//! Snapshot / delta encoding (Phase 2 line diff, Phase 6 Myers + ADT
//! patches).
//!
//! Phase 2 shipped a small LCS line diff over plain text transcripts as a
//! placeholder Phase-2 primitive. Phase 6 adds two things on top, *without*
//! removing the line diff (`gauss-conformance` still exercises it under
//! Theorem T3):
//!
//! * [`myers`] — a proper Myers greedy diff that walks the edit-distance
//!   diagonals in `O((N + M) · D)` time, where `D` is the number of
//!   non-matching edits. Produces a serialisable [`myers::Patch`] of
//!   `Equal` / `Insert` / `Delete` runs over abstract tokens (so callers can
//!   diff lines, words, AST nodes, or canonicalised JSON ADT fragments).
//! * The [`crate::klru`] cache builds on top of `myers::Patch` for its
//!   delta-from-parent nodes — the cache + diff together implement the
//!   warm-context regime of paper §VIII.D.
//!
//! ## Line-diff algorithm (retained Phase-2 path)
//!
//! Computes the longest-common-subsequence over **lines** of two strings,
//! then emits a sequence of [`DiffOp`]s that reconstruct `next` from `prev`.
//! The implementation is dynamic programming (`O(n·m)` time, `O(n·m)`
//! space). Acceptable for transcripts of a few hundred lines; for larger
//! payloads use [`myers::diff`] which scales linearly in the common case.

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

/// Myers diff over abstract tokens (Phase 6).
///
/// The classic greedy Myers algorithm runs in `O((N + M) · D)` time where
/// `D` is the edit-script length. For long mostly-identical transcripts this
/// is effectively linear in the input size — a key property for the
/// warm-cache regime targeted by Theorem T12.
///
/// `myers::diff` is generic over `T: Eq + Clone`, so callers can:
///
/// * Diff lines: `myers::diff(prev.lines().collect(), next.lines().collect())`.
/// * Diff JSON canonical-form tokens for ADT delta storage.
/// * Diff transcript tokens (regex-split words).
pub mod myers {
    // The greedy Myers algorithm interleaves signed and unsigned offsets in
    // a tight loop. The casts are deliberate and bounded by `prev.len() +
    // next.len()`; the saturating ops are correct by construction. We
    // allow the lints inside this module rather than wrapping every cast
    // in a `#[allow(...)]` attribute.
    #![allow(
        clippy::arithmetic_side_effects,
        clippy::cast_lossless,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        clippy::many_single_char_names,
        clippy::similar_names,
        clippy::redundant_closure_for_method_calls
    )]

    use serde::{Deserialize, Serialize};

    /// One run in a Myers patch.
    ///
    /// `Equal` carries the count of common tokens; `Insert` and `Delete`
    /// carry the tokens themselves so the patch is self-contained.
    #[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    pub enum Op<T> {
        /// A run of `count` matching tokens carried unchanged from `prev`.
        Equal {
            /// Number of consecutive matching tokens.
            count: usize,
        },
        /// One token from `next` inserted at this position.
        Insert {
            /// The inserted token.
            token: T,
        },
        /// One token from `prev` deleted at this position.
        Delete {
            /// The deleted token.
            token: T,
        },
    }

    /// Serialisable patch: a flat sequence of [`Op`]s, in order.
    #[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
    pub struct Patch {
        /// The operations that transform `prev` into `next`.
        pub ops: Vec<Op<String>>,
    }

    impl Patch {
        /// Build a patch from a vector of ops.
        #[must_use]
        pub const fn new(ops: Vec<Op<String>>) -> Self {
            Self { ops }
        }

        /// True iff this patch is empty (`prev == next`).
        #[must_use]
        pub const fn is_empty(&self) -> bool {
            self.ops.is_empty()
        }

        /// Number of insert + delete operations (the edit distance).
        #[must_use]
        pub fn edit_distance(&self) -> usize {
            self.ops
                .iter()
                .filter(|op| !matches!(op, Op::Equal { .. }))
                .count()
        }
    }

    /// Compute the Myers edit script that transforms `prev` into `next`.
    ///
    /// The output is a vector of `Op<T>`s where adjacent `Equal` ops are
    /// coalesced. For typical `String` callers, see [`diff_lines`] /
    /// [`diff_strs`] which materialise the result as a [`Patch`].
    #[must_use]
    pub fn diff<T: Eq + Clone>(prev: &[T], next: &[T]) -> Vec<Op<T>> {
        let n = prev.len();
        let m = next.len();
        let max = n.saturating_add(m);
        if max == 0 {
            return Vec::new();
        }
        // V[k + max] = furthest x reached on diagonal k after d edits.
        let size = max.saturating_mul(2).saturating_add(1);
        let mut v = vec![0i64; size];
        let mut trace: Vec<Vec<i64>> = Vec::with_capacity(max.saturating_add(1));
        let mut found_d: Option<usize> = None;
        let off = i64::try_from(max).unwrap_or(0);
        'outer: for d in 0..=max {
            let d_i = i64::try_from(d).unwrap_or(0);
            let mut k = -d_i;
            while k <= d_i {
                let idx = (k + off) as usize;
                let mut x: i64 = if k == -d_i
                    || (k != d_i && v[(k - 1 + off) as usize] < v[(k + 1 + off) as usize])
                {
                    v[(k + 1 + off) as usize]
                } else {
                    v[(k - 1 + off) as usize].saturating_add(1)
                };
                let mut y = x - k;
                while x < n as i64 && y < m as i64 {
                    let xi = usize::try_from(x).unwrap_or(0);
                    let yi = usize::try_from(y).unwrap_or(0);
                    if prev[xi] == next[yi] {
                        x = x.saturating_add(1);
                        y = y.saturating_add(1);
                    } else {
                        break;
                    }
                }
                v[idx] = x;
                if x >= n as i64 && y >= m as i64 {
                    trace.push(v.clone());
                    found_d = Some(d);
                    break 'outer;
                }
                k = k.saturating_add(2);
            }
            trace.push(v.clone());
        }
        let d = found_d.unwrap_or(max);
        // Reconstruct the path by walking backwards through `trace`.
        let mut x = i64::try_from(n).unwrap_or(0);
        let mut y = i64::try_from(m).unwrap_or(0);
        let mut ops_rev: Vec<Op<T>> = Vec::new();
        let mut dd = i64::try_from(d).unwrap_or(0);
        while dd > 0 {
            let v_prev = &trace[usize::try_from(dd.saturating_sub(1)).unwrap_or(0)];
            let k = x - y;
            let prev_k = if k == -dd
                || (k != dd && v_prev[(k - 1 + off) as usize] < v_prev[(k + 1 + off) as usize])
            {
                k + 1
            } else {
                k - 1
            };
            let prev_x = v_prev[(prev_k + off) as usize];
            let prev_y = prev_x - prev_k;
            // Diagonal moves before the edit.
            while x > prev_x && y > prev_y {
                let xi = usize::try_from(x.saturating_sub(1)).unwrap_or(0);
                ops_rev.push(Op::Equal { count: 1 });
                let _ = xi;
                x = x.saturating_sub(1);
                y = y.saturating_sub(1);
            }
            // The edit itself.
            if x == prev_x {
                // Insert from next at y-1.
                let yi = usize::try_from(prev_y).unwrap_or(0);
                ops_rev.push(Op::Insert {
                    token: next[yi].clone(),
                });
            } else {
                // Delete from prev at x-1.
                let xi = usize::try_from(prev_x).unwrap_or(0);
                ops_rev.push(Op::Delete {
                    token: prev[xi].clone(),
                });
            }
            x = prev_x;
            y = prev_y;
            dd = dd.saturating_sub(1);
        }
        // Any remaining diagonal at the start.
        while x > 0 && y > 0 {
            ops_rev.push(Op::Equal { count: 1 });
            x = x.saturating_sub(1);
            y = y.saturating_sub(1);
        }
        ops_rev.reverse();
        coalesce_equal(ops_rev)
    }

    /// Convenience: line-level Myers diff for `String`-shaped payloads.
    #[must_use]
    pub fn diff_lines(prev: &str, next: &str) -> Patch {
        let p: Vec<String> = prev.lines().map(str::to_owned).collect();
        let n: Vec<String> = next.lines().map(str::to_owned).collect();
        Patch::new(diff(&p, &n))
    }

    /// Convenience: token-level Myers diff over whitespace-split words.
    #[must_use]
    pub fn diff_strs(prev: &str, next: &str) -> Patch {
        let p: Vec<String> = prev.split_whitespace().map(str::to_owned).collect();
        let n: Vec<String> = next.split_whitespace().map(str::to_owned).collect();
        Patch::new(diff(&p, &n))
    }

    /// Reconstruct `next` from `prev` and a [`Patch`].
    ///
    /// # Errors
    /// Returns `ApplyError::OutOfRange` when the patch references more
    /// tokens than `prev` contains, or `Mismatch` when a `Delete` op's
    /// token does not equal the current `prev` token (a sign of base
    /// drift).
    pub fn apply_lines(prev: &str, patch: &Patch) -> Result<String, ApplyError> {
        let tokens: Vec<&str> = prev.lines().collect();
        let mut out: Vec<String> = Vec::with_capacity(tokens.len());
        let mut cursor = 0usize;
        for op in &patch.ops {
            match op {
                Op::Equal { count } => {
                    let end = cursor.saturating_add(*count);
                    if end > tokens.len() {
                        return Err(ApplyError::OutOfRange);
                    }
                    for k in cursor..end {
                        out.push(tokens[k].to_owned());
                    }
                    cursor = end;
                }
                Op::Delete { token } => {
                    if cursor >= tokens.len() {
                        return Err(ApplyError::OutOfRange);
                    }
                    if tokens[cursor] != token {
                        return Err(ApplyError::Mismatch);
                    }
                    cursor = cursor.saturating_add(1);
                }
                Op::Insert { token } => out.push(token.clone()),
            }
        }
        Ok(out.join("\n"))
    }

    /// Errors returned by [`apply_lines`].
    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    pub enum ApplyError {
        /// A patch op references positions outside `prev`.
        OutOfRange,
        /// A `Delete` op disagrees with the actual token in `prev` — the
        /// patch and its base diverged.
        Mismatch,
    }

    impl core::fmt::Display for ApplyError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::OutOfRange => f.write_str("Myers patch references tokens outside prev"),
                Self::Mismatch => f.write_str("Myers patch delete token disagrees with prev"),
            }
        }
    }

    impl std::error::Error for ApplyError {}

    /// Coalesce adjacent `Equal` ops into a single counted run.
    fn coalesce_equal<T>(ops: Vec<Op<T>>) -> Vec<Op<T>> {
        let mut out: Vec<Op<T>> = Vec::with_capacity(ops.len());
        for op in ops {
            match (out.last_mut(), op) {
                (Some(Op::Equal { count }), Op::Equal { count: inc }) => {
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
        fn empty_inputs_yield_empty_patch() {
            let p: Vec<String> = Vec::new();
            let n: Vec<String> = Vec::new();
            assert!(diff(&p, &n).is_empty());
        }

        #[test]
        fn identical_inputs_collapse_to_one_equal() {
            let prev = "alpha\nbeta\ngamma";
            let patch = diff_lines(prev, prev);
            assert_eq!(patch.ops.len(), 1);
            assert!(matches!(patch.ops[0], Op::Equal { count: 3 }));
            assert_eq!(patch.edit_distance(), 0);
        }

        #[test]
        fn pure_insertion_emits_only_inserts() {
            let patch = diff_lines("", "a\nb");
            let inserts = patch
                .ops
                .iter()
                .filter(|op| matches!(op, Op::Insert { .. }))
                .count();
            assert_eq!(inserts, 2);
            assert_eq!(patch.edit_distance(), 2);
        }

        #[test]
        fn pure_deletion_emits_only_deletes() {
            let patch = diff_lines("a\nb\nc", "");
            let deletes = patch
                .ops
                .iter()
                .filter(|op| matches!(op, Op::Delete { .. }))
                .count();
            assert_eq!(deletes, 3);
        }

        #[test]
        fn apply_reconstructs_next() {
            let prev = "user: hi\nagent: hello\nuser: when?";
            let next = "user: hi\nagent: hi there\nagent: 3 pm\nuser: thanks";
            let patch = diff_lines(prev, next);
            let reconstructed = apply_lines(prev, &patch).unwrap();
            assert_eq!(reconstructed, next);
        }

        #[test]
        fn token_diff_round_trips_whitespace_words() {
            let prev = "the quick brown fox";
            let next = "the quick orange fox jumps";
            let patch = diff_strs(prev, next);
            // 1 delete (brown) + 2 inserts (orange, jumps) = edit distance 3.
            assert_eq!(patch.edit_distance(), 3);
        }

        #[test]
        fn apply_rejects_mismatched_delete() {
            let patch = Patch::new(vec![Op::Delete {
                token: "expected".to_owned(),
            }]);
            let err = apply_lines("actual", &patch).unwrap_err();
            assert_eq!(err, ApplyError::Mismatch);
        }

        #[test]
        fn apply_rejects_out_of_range_equal() {
            let patch = Patch::new(vec![Op::Equal { count: 99 }]);
            let err = apply_lines("a", &patch).unwrap_err();
            assert_eq!(err, ApplyError::OutOfRange);
        }
    }
}
