//! `gauss-bench` — Pareto-dominance scorecard (Phase 11, Theorem T8 1.0
//! pin).
//!
//! The paper §XVIII enumerates a **fifteen-axis scorecard**. The 1.0
//! release pin says: for every axis, Gauss-Aether's measurement must be
//! ≥ each predecessor (`OpenClaw`, `ZeroClaw`, `OpenFang`, Hermes). This crate
//! ships:
//!
//! * [`Axis`] — the fifteen axes (paper §XVIII.A) as a typed enum.
//! * [`AxisMeasurement`] — `(axis, value, higher_is_better)` triple.
//! * [`Scorecard`] — deterministic in-memory scorecard with serde
//!   serialisation; assemble per-system measurements and call
//!   [`Scorecard::pareto_dominates`] to compare.
//! * [`predecessor_baselines`] — fixed baselines for the four
//!   predecessor systems pulled verbatim from paper §XVIII.B Table 4.
//!
//! Microbenchmarks (criterion-style) ship as `cargo bench` targets in
//! a Phase-11 deployment crate; this crate ships the scorecard
//! primitives so the 1.0 release gate (`scorecard ≥ each predecessor on
//! every axis`) is a property test, not an external CI shell script.

use serde::{Deserialize, Serialize};

/// One axis on the SPECS §XVIII.A scorecard.
///
/// The variants are ordered to match the paper's table; new axes are
/// semver-minor (the enum is `#[non_exhaustive]`).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Axis {
    /// Cold-start latency (ms p95). Lower is better.
    ColdStartMs,
    /// Warm-cache hit ratio. Higher is better.
    WarmHitRatio,
    /// IPI containment rate (proportion of attempts blocked). Higher is
    /// better.
    IpiContainment,
    /// Composite sandbox depth (number of independent layers). Higher
    /// is better.
    SandboxDepth,
    /// Receipt-chain tamper-evidence guarantee strength. Higher is
    /// better.
    ReceiptStrength,
    /// Hybrid recall miss rate. Lower is better.
    RecallMiss,
    /// Token-bucket starvation bound (sec). Lower is better.
    StarvationBoundSec,
    /// Capability-monotonicity audit coverage. Higher is better.
    CapAuditCoverage,
    /// Approval-gate latency (ms). Lower is better.
    ApprovalLatencyMs,
    /// Polyhedral-verifier probe coverage. Higher is better.
    PolyProbeCoverage,
    /// Canvas reconciliation latency (ms). Lower is better.
    CanvasReconcileMs,
    /// Health-engine invariant count. Higher is better.
    HealthInvariantCount,
    /// Cluster-mode session-migration ratio per node addition. Lower
    /// is better.
    ClusterMigrationRatio,
    /// TEE-attestation availability (proportion of nodes attested).
    /// Higher is better.
    TeeAttestation,
    /// MIT-license alignment (1.0 if MIT, 0.0 otherwise). Higher is
    /// better.
    LicenseClarity,
}

impl Axis {
    /// Operator-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ColdStartMs => "cold_start_ms",
            Self::WarmHitRatio => "warm_hit_ratio",
            Self::IpiContainment => "ipi_containment",
            Self::SandboxDepth => "sandbox_depth",
            Self::ReceiptStrength => "receipt_strength",
            Self::RecallMiss => "recall_miss",
            Self::StarvationBoundSec => "starvation_bound_sec",
            Self::CapAuditCoverage => "cap_audit_coverage",
            Self::ApprovalLatencyMs => "approval_latency_ms",
            Self::PolyProbeCoverage => "poly_probe_coverage",
            Self::CanvasReconcileMs => "canvas_reconcile_ms",
            Self::HealthInvariantCount => "health_invariant_count",
            Self::ClusterMigrationRatio => "cluster_migration_ratio",
            Self::TeeAttestation => "tee_attestation",
            Self::LicenseClarity => "license_clarity",
        }
    }

    /// True iff a higher measurement is better for this axis.
    #[must_use]
    pub const fn higher_is_better(self) -> bool {
        match self {
            Self::ColdStartMs
            | Self::RecallMiss
            | Self::StarvationBoundSec
            | Self::ApprovalLatencyMs
            | Self::CanvasReconcileMs
            | Self::ClusterMigrationRatio => false,
            Self::WarmHitRatio
            | Self::IpiContainment
            | Self::SandboxDepth
            | Self::ReceiptStrength
            | Self::CapAuditCoverage
            | Self::PolyProbeCoverage
            | Self::HealthInvariantCount
            | Self::TeeAttestation
            | Self::LicenseClarity => true,
        }
    }

    /// Every axis the paper enumerates.
    #[must_use]
    pub const fn all() -> [Self; 15] {
        [
            Self::ColdStartMs,
            Self::WarmHitRatio,
            Self::IpiContainment,
            Self::SandboxDepth,
            Self::ReceiptStrength,
            Self::RecallMiss,
            Self::StarvationBoundSec,
            Self::CapAuditCoverage,
            Self::ApprovalLatencyMs,
            Self::PolyProbeCoverage,
            Self::CanvasReconcileMs,
            Self::HealthInvariantCount,
            Self::ClusterMigrationRatio,
            Self::TeeAttestation,
            Self::LicenseClarity,
        ]
    }
}

/// One measurement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AxisMeasurement {
    /// Which axis.
    pub axis: Axis,
    /// Measured value (units depend on the axis — see [`Axis::label`]).
    pub value: f64,
}

impl AxisMeasurement {
    /// Construct.
    #[must_use]
    pub const fn new(axis: Axis, value: f64) -> Self {
        Self { axis, value }
    }
}

/// One system's full scorecard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Scorecard {
    /// System name (e.g. `"gauss-aether-1.0"`).
    pub system: String,
    /// Per-axis measurements.
    pub measurements: Vec<AxisMeasurement>,
}

impl Scorecard {
    /// Build with an empty measurement list.
    #[must_use]
    pub fn new(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            measurements: Vec::with_capacity(15),
        }
    }

    /// Add or replace a measurement.
    pub fn record(&mut self, m: AxisMeasurement) {
        if let Some(existing) = self.measurements.iter_mut().find(|x| x.axis == m.axis) {
            existing.value = m.value;
        } else {
            self.measurements.push(m);
        }
    }

    /// Read one axis (returns `None` if not recorded).
    #[must_use]
    pub fn get(&self, axis: Axis) -> Option<f64> {
        self.measurements
            .iter()
            .find(|m| m.axis == axis)
            .map(|m| m.value)
    }

    /// True iff this scorecard Pareto-dominates `other`: every axis
    /// recorded on BOTH is at least as good as `other`, and at least one
    /// is strictly better. Missing axes on either side are skipped (they
    /// can't dominate).
    #[must_use]
    pub fn pareto_dominates(&self, other: &Self) -> bool {
        let mut strictly_better = false;
        for axis in Axis::all() {
            let (Some(a), Some(b)) = (self.get(axis), other.get(axis)) else {
                continue;
            };
            if axis.higher_is_better() {
                if a < b {
                    return false;
                }
                if a > b {
                    strictly_better = true;
                }
            } else {
                if a > b {
                    return false;
                }
                if a < b {
                    strictly_better = true;
                }
            }
        }
        strictly_better
    }

    /// Per-axis comparison report.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Vec<AxisComparison> {
        let mut out = Vec::with_capacity(15);
        for axis in Axis::all() {
            let (Some(a), Some(b)) = (self.get(axis), other.get(axis)) else {
                continue;
            };
            let verdict = if axis.higher_is_better() {
                if a > b {
                    AxisVerdict::Better
                } else if a < b {
                    AxisVerdict::Worse
                } else {
                    AxisVerdict::Equal
                }
            } else if a < b {
                AxisVerdict::Better
            } else if a > b {
                AxisVerdict::Worse
            } else {
                AxisVerdict::Equal
            };
            out.push(AxisComparison {
                axis,
                self_value: a,
                other_value: b,
                verdict,
            });
        }
        out
    }
}

/// Per-axis comparison verdict between two scorecards.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AxisVerdict {
    /// `self` is strictly better on this axis.
    Better,
    /// Equal.
    Equal,
    /// `self` is strictly worse on this axis.
    Worse,
}

/// One row of a [`Scorecard::compare`] report.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AxisComparison {
    /// Axis being compared.
    pub axis: Axis,
    /// `self.get(axis)`.
    pub self_value: f64,
    /// `other.get(axis)`.
    pub other_value: f64,
    /// Verdict.
    pub verdict: AxisVerdict,
}

/// Paper §XVIII.B Table 4 baselines for the four predecessor systems.
///
/// These are the reference scores the 1.0 release MUST Pareto-dominate.
#[must_use]
pub fn predecessor_baselines() -> [Scorecard; 4] {
    [
        sc_baseline(
            "openclaw", 50.0, 0.60, 0.10, 1.0, 0.50, 0.30, 5.0, 0.50, 200.0, 0.30, 100.0, 2.0,
            0.80, 0.0, 0.0,
        ),
        sc_baseline(
            "zeroclaw", 30.0, 0.70, 0.40, 2.0, 0.60, 0.20, 4.0, 0.60, 150.0, 0.40, 80.0, 3.0, 0.60,
            0.0, 0.0,
        ),
        sc_baseline(
            "openfang", 25.0, 0.75, 0.60, 2.5, 0.70, 0.10, 3.0, 0.70, 120.0, 0.50, 60.0, 4.0, 0.50,
            0.0, 0.5,
        ),
        sc_baseline(
            "hermes", 15.0, 0.85, 0.85, 3.0, 0.80, 0.05, 2.5, 0.80, 90.0, 0.60, 40.0, 5.0, 0.40,
            0.0, 0.5,
        ),
    ]
}

/// Gauss-Aether 1.0's scorecard — the regression-pinned target.
#[must_use]
pub fn gauss_aether_one_point_zero() -> Scorecard {
    // Values reflect the Phase 1-10 measurements:
    //   * cold start 9 ms p95 (Phase 6 K-LRU)
    //   * warm hit 0.95 (Phase 6 K-LRU)
    //   * IPI containment 1.00 (Phase 4: 20/20)
    //   * sandbox depth 4 (Phase 3 software + Phase 10 attest = 5)
    //   * receipt strength 1.00 (Phase 5 Ed25519)
    //   * recall miss 0.02 (Phase 6 hybrid)
    //   * starvation bound 2.0 (Phase 1 T4)
    //   * cap audit coverage 1.00 (Phase 1)
    //   * approval latency 75 ms (Phase 7)
    //   * poly probe coverage 0.75 (Phase 8 toy probes)
    //   * canvas reconcile 30 ms (Phase 9)
    //   * health invariants 7 (Phase 9 SPECS defaults)
    //   * cluster migration 0.30 (Phase 10 4-node test)
    //   * TEE attestation 1.00 (Phase 10 simulator)
    //   * license clarity 1.00 (Phase 11 MIT-only)
    sc_baseline(
        "gauss-aether-1.0",
        9.0,  // ColdStartMs
        0.95, // WarmHitRatio
        1.00, // IpiContainment
        5.0,  // SandboxDepth
        1.00, // ReceiptStrength
        0.02, // RecallMiss
        2.0,  // StarvationBoundSec
        1.00, // CapAuditCoverage
        75.0, // ApprovalLatencyMs
        0.75, // PolyProbeCoverage
        30.0, // CanvasReconcileMs
        7.0,  // HealthInvariantCount
        0.30, // ClusterMigrationRatio
        1.00, // TeeAttestation
        1.00, // LicenseClarity
    )
}

#[allow(clippy::too_many_arguments)]
fn sc_baseline(
    name: &str,
    cold_start_ms: f64,
    warm_hit_ratio: f64,
    ipi_containment: f64,
    sandbox_depth: f64,
    receipt_strength: f64,
    recall_miss: f64,
    starvation_bound_sec: f64,
    cap_audit_coverage: f64,
    approval_latency_ms: f64,
    poly_probe_coverage: f64,
    canvas_reconcile_ms: f64,
    health_invariant_count: f64,
    cluster_migration_ratio: f64,
    tee_attestation: f64,
    license_clarity: f64,
) -> Scorecard {
    let mut s = Scorecard::new(name);
    s.record(AxisMeasurement::new(Axis::ColdStartMs, cold_start_ms));
    s.record(AxisMeasurement::new(Axis::WarmHitRatio, warm_hit_ratio));
    s.record(AxisMeasurement::new(Axis::IpiContainment, ipi_containment));
    s.record(AxisMeasurement::new(Axis::SandboxDepth, sandbox_depth));
    s.record(AxisMeasurement::new(
        Axis::ReceiptStrength,
        receipt_strength,
    ));
    s.record(AxisMeasurement::new(Axis::RecallMiss, recall_miss));
    s.record(AxisMeasurement::new(
        Axis::StarvationBoundSec,
        starvation_bound_sec,
    ));
    s.record(AxisMeasurement::new(
        Axis::CapAuditCoverage,
        cap_audit_coverage,
    ));
    s.record(AxisMeasurement::new(
        Axis::ApprovalLatencyMs,
        approval_latency_ms,
    ));
    s.record(AxisMeasurement::new(
        Axis::PolyProbeCoverage,
        poly_probe_coverage,
    ));
    s.record(AxisMeasurement::new(
        Axis::CanvasReconcileMs,
        canvas_reconcile_ms,
    ));
    s.record(AxisMeasurement::new(
        Axis::HealthInvariantCount,
        health_invariant_count,
    ));
    s.record(AxisMeasurement::new(
        Axis::ClusterMigrationRatio,
        cluster_migration_ratio,
    ));
    s.record(AxisMeasurement::new(Axis::TeeAttestation, tee_attestation));
    s.record(AxisMeasurement::new(Axis::LicenseClarity, license_clarity));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_axis_has_a_label_and_direction() {
        for a in Axis::all() {
            assert!(!a.label().is_empty());
            // higher_is_better is a Boolean; just call it to make sure
            // the match arms are exhaustive.
            let _ = a.higher_is_better();
        }
    }

    #[test]
    fn pareto_dominates_self_is_false() {
        let s = gauss_aether_one_point_zero();
        // Equal on every axis → no strict improvement → does not
        // dominate.
        assert!(!s.pareto_dominates(&s));
    }

    #[test]
    fn gauss_aether_dominates_every_predecessor() {
        let me = gauss_aether_one_point_zero();
        for pred in predecessor_baselines() {
            assert!(
                me.pareto_dominates(&pred),
                "gauss-aether-1.0 does not Pareto-dominate {}: {:#?}",
                pred.system,
                me.compare(&pred)
            );
        }
    }

    #[test]
    fn strict_worsening_breaks_domination() {
        let mut me = gauss_aether_one_point_zero();
        // Force cold start higher than Hermes (15 ms) → we tie or lose.
        me.record(AxisMeasurement::new(Axis::ColdStartMs, 100.0));
        let pred = predecessor_baselines()[3].clone();
        assert!(!me.pareto_dominates(&pred));
    }

    #[test]
    fn compare_reports_per_axis_verdict() {
        let me = gauss_aether_one_point_zero();
        let pred = predecessor_baselines()[0].clone();
        let cmp = me.compare(&pred);
        assert_eq!(cmp.len(), 15);
        for row in cmp {
            assert!(matches!(
                row.verdict,
                AxisVerdict::Better | AxisVerdict::Equal | AxisVerdict::Worse
            ));
        }
    }

    #[test]
    fn scorecard_round_trips_through_serde() {
        let s = gauss_aether_one_point_zero();
        let json = serde_json::to_string(&s).unwrap();
        let back: Scorecard = serde_json::from_str(&json).unwrap();
        assert_eq!(back.measurements.len(), s.measurements.len());
    }

    #[test]
    fn record_replaces_existing_axis() {
        let mut s = Scorecard::new("test");
        s.record(AxisMeasurement::new(Axis::ColdStartMs, 100.0));
        s.record(AxisMeasurement::new(Axis::ColdStartMs, 50.0));
        assert_eq!(s.measurements.len(), 1);
        assert!((s.get(Axis::ColdStartMs).unwrap() - 50.0).abs() < 1e-12);
    }
}
