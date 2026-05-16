# ADR-0014 — Polyhedral trait equivalence + `specT` style guide

**Status:** Accepted (Phase 8)
**Date:** 2026-05-16
**Proves:** Theorem T7 (provider adjunction)

## Context

The plugin surface of Gauss-Aether is a small set of stable traits
(`Kernel`, `MemoryBackend`, `Provider`, `SandboxTrait`, `ToolTrait`,
`SigningBackend`, `ApprovalSurface`, `Classifier`, `Canvas`, `Attestor`).
Phase 8 freezes the trait shapes and ships a build-time check that two
implementations of the same trait are **polyhedrally equivalent** —
they produce structurally-identical outputs on a finite probe set.

The check is the mechanical witness for Theorem T7 (paper §XII.B):
swapping one implementation for another in a running deployment yields
semantically-equivalent transcripts.

## Decision

### 1. `Probe<I, O>` + `PolyhedralProbeSet<I, O>`

Probes are ordered, named (input, expected-output) pairs. The probe set
is deterministic — tests pin probes by name + index so a regression is
diagnosable from the failure message alone.

### 2. Verification is byte-equal canonical-form comparison

Two implementations are equivalent iff `serde_json::to_vec(p_out) ==
serde_json::to_vec(q_out)` for every probe. We do NOT use
semantic-equivalence-up-to-bisimulation: two providers that produce
JSON with different field orderings are NOT equivalent. This makes the
trait contract precise and lets tooling (the future `cargo
gauss-verify` driver) pin equivalence without an SMT solver.

### 3. `verify_provider_equivalence` is the only Phase-8 helper

Phase 8 ships the canonical helper for `Provider` because that's the
trait the swap-test in the SPECS Phase-8 exit gate exercises. The same
shape generalises trivially to other traits — additional helpers
(`verify_approval_surface_equivalence`, `verify_classifier_equivalence`,
etc.) follow as the trait surfaces stabilise in Phase 9+ deployments.

### 4. `specT` style guide for plugin traits

A trait is plugin-ready iff:

1. It returns serializable outputs (`serde::Serialize +
   serde::Deserialize`).
2. Its constructor surface uses `#[non_exhaustive]` so semver-minor
   evolution doesn't break downstream impls.
3. Its asynchronous methods return `GaussResult<T>` so error semantics
   line up with the kernel's unified error.
4. Its core invariants are property-test-able from a finite probe set.

`gauss-traits` enforces (1–3) at compile time; the `verify_*_equivalence`
helpers enforce (4) at test time.

## Consequences

- **Pro:** Plugin authors get a one-liner check (`verify_provider_
  equivalence(&new, &reference, &PROBE_SET).await?`) to certify their
  impl against a reference.
- **Pro:** Equivalence is purely structural; no SMT, no semantic
  approximations.
- **Pro:** The trait surface is auditable line-by-line; ADR-0014
  documents the four `specT` style-guide rules.
- **Con:** Probe sets are operator-supplied. The Phase-8 ship is the
  toy-provider probe set; production deployments need to author probes
  that exercise their workload.
- **Con:** Two providers that are *semantically* equivalent but
  produce different JSON field orderings will diverge. This is the
  cost of the byte-equal contract.

## Migration / replacement

The verifier is intentionally generic. New traits + new helpers:

- New trait surface (Phase 9+ Canvas, Phase 10 Attestor, …): add a
  `verify_<trait>_equivalence` function in `gauss-poly`.
- Production probe set: load from disk via `serde_json::from_str`; the
  `PolyhedralProbeSet<I, O>` type is `serde::Deserialize` for any
  `I: Deserialize, O: Deserialize`.
