# ADR-0001 — Axiom-driven phasing

**Status:** Accepted (Phase 0)
**Date:** 2026-05-16
**Deciders:** Gauss-Aether architects

## Context

The Gauss-Aether paper specifies the system as an eleven-tuple `G = (S, A, O,
K, M, F, π, L, Φ, R, V)` constrained by nine axioms (A1–A9) and twelve
theorems (T1–T12). Each subsystem in the architecture exists because some
theorem requires it (paper Table IV).

The natural failure mode for a project of this scope is to deliver a feature
list rather than the architectural commitments that make those features
trustworthy. We have studied the four reference platforms (Hermes, OpenFang,
OpenClaw, ZeroClaw): each ships an impressive subset of the features but
none ships all nine axioms simultaneously. The paper's central claim is that
Gauss-Aether is the *unique* fixed point that satisfies all nine.

## Decision

The development plan is sequenced **by axiom**, not by feature. Each phase
locks a coherent subset of axioms (e.g. Phase 1 locks A2 + A4, proves T2 +
T4). A phase exits only when its conformance suite is green.

Every PR description MUST reference the axiom / theorem / SPECS section it
advances. Reviewers reject PRs that change kernel behaviour without a clear
trace back to the formal model.

## Consequences

- Phases overlap less than feature-driven plans typically do; cross-cutting
  refactors are rarer.
- The conformance suite (`gauss-conformance`) is the actual exit criterion,
  not an afterthought.
- Roadmap slippage shows up as "Phase N took longer" not "feature X is
  half-done"; status communication is cleaner.
- New contributors need to read SPECS.md before reviewing kernel PRs. The
  cost is a steeper onboarding ramp; the benefit is that reviewers all share
  the same vocabulary.

## Alternatives considered

- **Feature-driven roadmap.** Rejected: documented to fail in the reference
  platforms (see paper §II for the architectural lessons).
- **Big-bang single release.** Rejected: too large; we need usable
  conformance feedback by Phase 2 at the latest.
