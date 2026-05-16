//! Probe sets for trait equivalence verification.
//!
//! A [`Probe`] is one (input, expected-output) pair plus a human-readable
//! name; a [`PolyhedralProbeSet`] is a deterministic ordered collection of
//! probes. The verifier walks the set, calls the implementations in turn,
//! and reports the first divergence.

use serde::{Deserialize, Serialize};

/// One probe — a named input + the canonical expected output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Probe<I, O> {
    /// Operator-readable identifier for diagnostics.
    pub name: String,
    /// Input passed to each implementation.
    pub input: I,
    /// Canonical expected output. Two implementations are equivalent iff
    /// both produce this value for the input.
    pub expected: O,
}

impl<I, O> Probe<I, O> {
    /// Construct a probe.
    #[must_use]
    pub fn new(name: impl Into<String>, input: I, expected: O) -> Self {
        Self {
            name: name.into(),
            input,
            expected,
        }
    }
}

/// Deterministic ordered probe set. The verifier walks them in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PolyhedralProbeSet<I, O> {
    /// The probes.
    pub probes: Vec<Probe<I, O>>,
}

impl<I, O> Default for PolyhedralProbeSet<I, O> {
    fn default() -> Self {
        Self { probes: Vec::new() }
    }
}

impl<I, O> PolyhedralProbeSet<I, O> {
    /// Build a probe set from a vec of probes.
    #[must_use]
    pub const fn new(probes: Vec<Probe<I, O>>) -> Self {
        Self { probes }
    }

    /// Append a probe.
    pub fn push(&mut self, probe: Probe<I, O>) {
        self.probes.push(probe);
    }

    /// Number of probes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.probes.len()
    }

    /// True iff there are no probes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_serde() {
        let p = Probe::new("hello", 1_i32, "one".to_owned());
        let s = serde_json::to_string(&p).unwrap();
        let back: Probe<i32, String> = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "hello");
        assert_eq!(back.input, 1);
        assert_eq!(back.expected, "one");
    }

    #[test]
    fn probe_set_starts_empty_and_grows() {
        let mut set: PolyhedralProbeSet<i32, i32> = PolyhedralProbeSet::default();
        assert!(set.is_empty());
        set.push(Probe::new("one", 1, 1));
        assert_eq!(set.len(), 1);
    }
}
