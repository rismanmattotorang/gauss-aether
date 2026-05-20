//! Canonical [`PolyhedralProbeSet`] — the baseline the CI gate checks every PR against.
//!
//! Sprint 9 §8 of `/ROADMAP.md`. The polyhedral CI gate at
//! `.github/workflows/poly-gate.yml` triggers when a PR touches
//! `gaussclaw-providers`, `gaussclaw-providers-meta`, or `gauss-poly`
//! itself. It runs every test whose name contains `polyhedral_probe`
//! or `polyhedral_provider`, and the canonical-snapshot probe in this
//! module is the entry point.
//!
//! The snapshot is `src/snapshots/canonical.json`, included via
//! [`include_str!`]. A code-side builder ([`canonical`]) constructs
//! the same probe set in Rust; a round-trip test enforces byte-equal
//! agreement between the builder and the snapshot file, so:
//!
//! 1. Changes to the canonical probe set must update **both** the
//!    builder code and the snapshot JSON in the same commit.
//! 2. A diff that touches one but not the other fails CI immediately
//!    — operators cannot silently widen or narrow the baseline.
//!
//! Hermes upstream has no equivalent baseline.

use gauss_core::{Action, Observation, ObservationSource, TaintLabel, TextAction};

use crate::probe::{PolyhedralProbeSet, Probe};

/// The checked-in canonical snapshot bytes.
pub const SNAPSHOT_BYTES: &str = include_str!("snapshots/canonical.json");

/// Build the canonical probe set from Rust source.
///
/// The set exercises every [`ObservationSource`] variant against a
/// `ToyProvider::always_text("ok")` baseline. Two compatible
/// providers must agree with the baseline on every probe to pass
/// the gate.
#[must_use]
pub fn canonical() -> PolyhedralProbeSet<Observation, Vec<Action>> {
    fn ok() -> Vec<Action> {
        vec![Action::Text(TextAction::new("ok"))]
    }

    let mut set = PolyhedralProbeSet::default();
    set.push(Probe::new(
        "user-empty-body",
        Observation::new(
            ObservationSource::User {
                channel: "cli".into(),
            },
            TaintLabel::User,
            serde_json::Value::Null,
        ),
        ok(),
    ));
    set.push(Probe::new(
        "user-text-body",
        Observation::new(
            ObservationSource::User {
                channel: "telegram".into(),
            },
            TaintLabel::User,
            serde_json::json!({"text": "hello"}),
        ),
        ok(),
    ));
    set.push(Probe::new(
        "tool-result",
        Observation::new(
            ObservationSource::Tool {
                tool: "echo".into(),
            },
            TaintLabel::Trusted,
            serde_json::json!({"echo": "hi"}),
        ),
        ok(),
    ));
    set.push(Probe::new(
        "schedule-fire",
        Observation::new(
            ObservationSource::Schedule {
                schedule_id: "daily-summary".into(),
            },
            TaintLabel::User,
            serde_json::Value::Null,
        ),
        ok(),
    ));
    set.push(Probe::new(
        "canvas-event",
        Observation::new(
            ObservationSource::Canvas {
                widget_id: "submit-button".into(),
            },
            TaintLabel::User,
            serde_json::json!({"clicked": true}),
        ),
        ok(),
    ));
    set.push(Probe::new(
        "web-tainted-tool",
        Observation::new(
            ObservationSource::Tool {
                tool: "web_fetch".into(),
            },
            TaintLabel::Web,
            serde_json::json!({"url": "https://example.com"}),
        ),
        ok(),
    ));
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CI gate runs this test. It asserts the code-side builder
    /// and the committed JSON snapshot are byte-equal after
    /// pretty-printing — so any drift between the Rust builder and the
    /// file fails closed.
    #[test]
    fn polyhedral_probe_canonical_matches_snapshot() {
        let set = canonical();
        let actual = serde_json::to_string_pretty(&set).expect("serialise");
        let expected = SNAPSHOT_BYTES.trim_end_matches('\n');
        assert_eq!(
            actual.trim_end_matches('\n'),
            expected,
            "canonical probe set diverged from committed snapshot — \
             update src/snapshots/canonical.json in the same commit \
             as src/canonical.rs"
        );
    }

    #[test]
    fn polyhedral_probe_canonical_snapshot_round_trips() {
        let set: PolyhedralProbeSet<Observation, Vec<Action>> =
            serde_json::from_str(SNAPSHOT_BYTES).expect("snapshot must be valid JSON");
        assert_eq!(set.len(), 6);
        for probe in &set.probes {
            assert_eq!(probe.expected.len(), 1);
            match &probe.expected[0] {
                Action::Text(t) => assert_eq!(t.body, "ok"),
                _ => panic!("non-text action in canonical baseline"),
            }
        }
    }

    /// End-to-end: `ToyProvider::always_text("ok")` must pass the
    /// canonical baseline. Two of them must also be polyhedrally
    /// equivalent on it. This is the test the CI gate ultimately
    /// gates the provider plane on — silent `ToyProvider` drift would
    /// fail closed.
    #[tokio::test]
    async fn polyhedral_probe_canonical_passes_toy_provider() {
        use crate::provider::verify_provider_equivalence;
        use gauss_provider::ToyProvider;

        let p = ToyProvider::always_text("ok");
        let q = ToyProvider::always_text("ok");
        let report = verify_provider_equivalence(&p, &q, &canonical())
            .await
            .expect("equivalent ToyProviders must pass the canonical baseline");
        assert!(report.ok());
        assert_eq!(report.total, 6);
        assert_eq!(report.passed, 6);
        assert!((report.divergence() - 0.0).abs() < 1e-9);
    }

    /// Smoke check: the baseline exercises every public
    /// `ObservationSource` variant exactly once (User, Tool, Schedule,
    /// Canvas — plus two extra User/Tool probes for taint coverage).
    /// Catches accidental drops when the builder is edited.
    #[test]
    fn polyhedral_probe_canonical_covers_every_source() {
        let set = canonical();
        let mut seen_user = false;
        let mut seen_tool = false;
        let mut seen_schedule = false;
        let mut seen_canvas = false;
        for probe in &set.probes {
            match probe.input.source {
                ObservationSource::User { .. } => seen_user = true,
                ObservationSource::Tool { .. } => seen_tool = true,
                ObservationSource::Schedule { .. } => seen_schedule = true,
                ObservationSource::Canvas { .. } => seen_canvas = true,
                _ => {}
            }
        }
        assert!(seen_user, "canonical baseline missing User source");
        assert!(seen_tool, "canonical baseline missing Tool source");
        assert!(seen_schedule, "canonical baseline missing Schedule source");
        assert!(seen_canvas, "canonical baseline missing Canvas source");
    }
}
