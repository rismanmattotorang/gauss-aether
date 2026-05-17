# Contributing to Gauss-Aether

Welcome. This guide covers the workflow, the `specT` style guide for
plugin traits, and the Tier-0 review policy for privileged crates.

## Workflow

1. **Trace your change.** Every PR description must cite the axiom /
   theorem / SPECS § / ADR that the change advances. The conformance
   suite is the authoritative reference — if you can't point at a
   pin, your change probably needs a new pin.
2. **Branch + small commits.** One commit per logical change; the
   commit message names the axiom / theorem advanced.
3. **CI pass.** Run all five gates locally before pushing:
   - `cargo fmt --all -- --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --workspace`
   - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`
   - `cargo deny check`
4. **Open a PR.** Tier-0 changes (see below) need dual review.

## Tier-0 review policy (ADR-0005)

Changes to these crates need **two reviewers** because they're the
privileged authority surface:

- `gauss-kernel`
- `gauss-audit`
- `gauss-attest`

Tier-1 changes (one reviewer) cover everything else; Tier-2 (no
review beyond CI) covers documentation-only PRs.

## The `specT` style guide for plugin traits (ADR-0014)

A trait is plugin-ready iff:

1. **Outputs are serializable.** `serde::Serialize + serde::Deserialize`
   on every return type. This is what makes the polyhedral verifier
   (`gauss-poly`) work — it compares canonical-JSON bytes.
2. **All public structs / enums are `#[non_exhaustive]`.** Field /
   variant additions stay semver-minor. Provide explicit constructors
   (`new`, `with_*`) since external crates can't struct-literal a
   non-exhaustive type.
3. **Async methods return `GaussResult<T>`.** Error semantics line up
   with `gauss_core::GaussError`.
4. **Invariants are probe-set-checkable.** A finite probe set must
   distinguish conforming implementations from non-conforming ones.

If your trait can't satisfy (4) — e.g. it ranges over an infinite
input space — write a property test in `gauss-conformance` instead.

## Adding a new plugin trait

The workflow:

1. Land the trait + types in `gauss-traits`.
2. Land a reference implementation as a workspace crate (`gauss-<feature>`).
3. Land a `verify_<trait>_equivalence` helper in `gauss-poly`.
4. Land a conformance module in `gauss-conformance::theorem_t<N>_*`.
5. Land an ADR (`docs/adr/00XX-*.md`).

The ADR is the authoritative artifact; the code is its mechanical
witness.

## Adding a new axiom / theorem

The workflow:

1. Land the prose in `SPECS.md` (single source of truth).
2. Land the formal statement in `proofs/lean/GaussAether/Axioms.lean`
   with a `sorry` / `trivial` placeholder.
3. Land the conformance test in `gauss-conformance` — this is where
   the property test or property-test-equivalent assertion lives.
4. Update `ROADMAP.md`'s test-count row.
5. Wire the axiom into the README's conformance table.

Mechanising the Lean proof is a separate, follow-up contribution
(v2 horizon — proofs land incrementally against the stable type
signature).

## Adding a new ADR

`docs/adr/00XX-short-title.md`. The template:

```markdown
# ADR-00XX — Short title

**Status:** Accepted / Proposed / Superseded (with date)
**Date:** YYYY-MM-DD
**Locks:** (axioms)
**Proves:** (theorems)

## Context
What's the problem; what alternatives were considered?

## Decision
The actual choice + the rationale.

## Consequences
What's better; what's worse.

## Migration / replacement
What changes if this decision is reversed?
```

ADR numbers are monotone — never reuse a number.

## Code review checklist

- [ ] PR description cites the relevant axiom / theorem / ADR.
- [ ] All five quality gates pass.
- [ ] New traits satisfy the `specT` rules.
- [ ] New types are `#[non_exhaustive]` with explicit constructors.
- [ ] No `unsafe` in `gauss-kernel` / `gauss-audit` / `gauss-attest`
      (workspace lint enforces this — CI would already block).
- [ ] Conformance test pin updated for every behavioural change.
- [ ] ROADMAP / README updated if a phase / scorecard axis changed.

## Tier-0 forbidden changes

These changes always require an ADR + dual review:

- Adding a new `CapToken` constant.
- Changing the canonical pre-image of a `SignedReceipt` (Phase 5
  receipts would no longer verify).
- Changing the SAG `Predicate` algebra (re-runs of
  `verify_monotonicity` MUST still accept the default table).
- Changing the wire format of an attestation report.

## Disclosure

Security issues — see `docs/SECURITY.md` for the responsible
disclosure policy.

## Code of conduct

Treat contributors well. We don't ship a separate CoC document; the
relevant constraint is the four-rule `specT` style guide above,
applied to your interactions as well as your traits.
