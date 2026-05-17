//! Taint-Aware Filter (Phase 5 §2 of the GaussClaw roadmap).
//!
//! Three modes selectable at export time:
//!
//! - **Permissive.** Emit every record. Taint is left in metadata for
//!   downstream consumers to act on. Useful for "raw" trajectory
//!   archives where the consumer applies its own filter later.
//! - **Strict.** Drop any record whose taint dominates [`TaintLabel::Web`]
//!   (i.e. `≥ Web`). Used when shipping to a public corpus.
//! - **Declassified.** Apply the runtime declass map; admit a record
//!   iff its post-declass taint ⪯ [`TaintLabel::Trusted`]. This is the
//!   default. Models the kernel's `declass()` contract from paper
//!   §VII; the declass map is supplied by the caller via
//!   [`TaintFilter::with_declass_fn`].
//!
//! The filter is a pure function over `(taint, record-bytes)` and is
//! composable: callers can stack a `TaintFilter` and then a
//! consumer-specific filter (e.g. drop tool outputs, strip PII) using
//! plain iterators. Hermes upstream has no explicit declass layer at
//! export.

use std::sync::Arc;

use gauss_core::TaintLabel;
use serde::{Deserialize, Serialize};

/// Selectable filter mode.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    /// Emit every record; taint marked in metadata.
    Permissive,
    /// Drop records with `taint ≥ Web`.
    Strict,
    /// Apply the declass map; admit iff post-declass taint = `Trusted`.
    #[default]
    Declassified,
}

/// Summary of one filter pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterReport {
    /// Records seen.
    pub input: u64,
    /// Records that passed the filter.
    pub kept: u64,
    /// Records that were dropped.
    pub dropped: u64,
}

impl FilterReport {
    /// Drop rate in `[0, 1]`. Returns `0.0` for an empty input set.
    #[must_use]
    pub fn drop_rate(&self) -> f64 {
        if self.input == 0 {
            return 0.0;
        }
        // Counts here are well-bounded by corpus size (≤ 10^7 routinely),
        // saturating to u32::MAX is safety paint.
        let dropped_u32 = u32::try_from(self.dropped).unwrap_or(u32::MAX);
        let input_u32 = u32::try_from(self.input).unwrap_or(u32::MAX);
        f64::from(dropped_u32) / f64::from(input_u32)
    }
}

/// Default declass map: paper §VII default — `Trusted` stays `Trusted`,
/// `User` declassifies to `Trusted`, `Web` and `Adversarial` retain.
///
/// Deployments wire stricter or more permissive maps via
/// [`TaintFilter::with_declass_fn`].
#[must_use]
pub fn default_declass(t: TaintLabel) -> TaintLabel {
    match t {
        TaintLabel::Trusted => TaintLabel::Trusted,
        // The default declassifies the User channel by hypothesis —
        // users own their own content; export under their consent
        // brings it to Trusted.
        TaintLabel::User => TaintLabel::Trusted,
        TaintLabel::Web => TaintLabel::Web,
        TaintLabel::Adversarial => TaintLabel::Adversarial,
    }
}

type DeclassFn = Arc<dyn Fn(TaintLabel) -> TaintLabel + Send + Sync>;

/// The filter itself.
pub struct TaintFilter {
    mode: FilterMode,
    declass: DeclassFn,
}

impl Default for TaintFilter {
    fn default() -> Self {
        Self::new(FilterMode::Declassified)
    }
}

impl std::fmt::Debug for TaintFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaintFilter")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl TaintFilter {
    /// Build a filter in the given mode with the default declass map.
    #[must_use]
    pub fn new(mode: FilterMode) -> Self {
        Self {
            mode,
            declass: Arc::new(default_declass),
        }
    }

    /// Swap the declass map. Only consulted in
    /// [`FilterMode::Declassified`].
    #[must_use]
    pub fn with_declass_fn<F>(mut self, f: F) -> Self
    where
        F: Fn(TaintLabel) -> TaintLabel + Send + Sync + 'static,
    {
        self.declass = Arc::new(f);
        self
    }

    /// Mode read-back.
    #[must_use]
    pub const fn mode(&self) -> FilterMode {
        self.mode
    }

    /// True iff a record with the given `taint` is admitted.
    #[must_use]
    pub fn admits(&self, taint: TaintLabel) -> bool {
        match self.mode {
            FilterMode::Permissive => true,
            FilterMode::Strict => taint.leq(TaintLabel::User),
            FilterMode::Declassified => (self.declass)(taint).leq(TaintLabel::Trusted),
        }
    }

    /// Apply to an iterator of `(taint, record)` pairs, returning the
    /// kept records and a [`FilterReport`].
    pub fn apply<T, I>(&self, items: I) -> (Vec<T>, FilterReport)
    where
        I: IntoIterator<Item = (TaintLabel, T)>,
    {
        let mut kept = Vec::new();
        let mut report = FilterReport::default();
        for (t, item) in items {
            report.input = report.input.saturating_add(1);
            if self.admits(t) {
                kept.push(item);
                report.kept = report.kept.saturating_add(1);
            } else {
                report.dropped = report.dropped.saturating_add(1);
            }
        }
        (kept, report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<(TaintLabel, &'static str)> {
        vec![
            (TaintLabel::Trusted, "trusted"),
            (TaintLabel::User, "user"),
            (TaintLabel::Web, "web"),
            (TaintLabel::Adversarial, "adv"),
        ]
    }

    #[test]
    fn permissive_keeps_every_record() {
        let f = TaintFilter::new(FilterMode::Permissive);
        let (kept, report) = f.apply(fixture());
        assert_eq!(kept.len(), 4);
        assert_eq!(report.input, 4);
        assert_eq!(report.kept, 4);
        assert_eq!(report.dropped, 0);
        assert!((report.drop_rate() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn strict_drops_web_and_above() {
        let f = TaintFilter::new(FilterMode::Strict);
        let (kept, report) = f.apply(fixture());
        let labels: Vec<_> = kept.iter().copied().collect();
        assert_eq!(labels, vec!["trusted", "user"]);
        assert_eq!(report.dropped, 2);
        assert!((report.drop_rate() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn declassified_default_keeps_user_drops_web() {
        let f = TaintFilter::new(FilterMode::Declassified);
        let (kept, report) = f.apply(fixture());
        // default_declass: User → Trusted ⇒ admitted; Web → Web (above
        // Trusted) ⇒ dropped.
        assert_eq!(kept, vec!["trusted", "user"]);
        assert_eq!(report.dropped, 2);
    }

    #[test]
    fn declassified_with_strict_declass_drops_user() {
        // A custom strict declass map: nothing declassifies. Only
        // already-Trusted records pass.
        let f = TaintFilter::new(FilterMode::Declassified)
            .with_declass_fn(|t| t);
        let (kept, _report) = f.apply(fixture());
        assert_eq!(kept, vec!["trusted"]);
    }

    #[test]
    fn empty_input_drop_rate_is_zero() {
        let f = TaintFilter::new(FilterMode::Strict);
        let (kept, report) = f.apply::<&str, _>(std::iter::empty());
        assert!(kept.is_empty());
        assert_eq!(report.input, 0);
        assert!((report.drop_rate() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn admits_is_consistent_with_apply() {
        for mode in [FilterMode::Permissive, FilterMode::Strict, FilterMode::Declassified] {
            let f = TaintFilter::new(mode);
            for (t, _) in fixture() {
                let admits = f.admits(t);
                let (kept, _) = f.apply([(t, "x")]);
                assert_eq!(admits, !kept.is_empty(), "mismatch at mode={mode:?} t={t:?}");
            }
        }
    }
}
