# ADR-0005 — Privilege tiers and review policy

**Status:** Accepted (Phase 0)
**Date:** 2026-05-16

## Context

The workspace contains code at very different blast radii. A bug in
`gauss-kernel::cap::reserve` can elevate capabilities silently; a bug in
`gauss-tui` makes the prompt blink wrong. Treating them identically is both
wasteful (over-reviewing trivial changes) and dangerous (under-reviewing
kernel changes).

The four reference platforms studied do not have explicit privilege tiers,
which the AgentRM literature [6] identifies as a contributing factor in
post-mortems of the documented incidents.

## Decision

Crates are assigned to one of four **tiers** (also documented in `SPECS.md`
§2):

| Tier | Crates                                                    | Rules                                          |
|------|-----------------------------------------------------------|------------------------------------------------|
| 0    | `gauss-kernel`, `gauss-audit`, `gauss-attest` (future)    | Dual review; no `unsafe` without an ADR; deps frozen at the workspace level; arithmetic-side-effects clippy lint promoted to error. |
| 1    | `gauss-turn`, `gauss-hwca`, `gauss-sandbox`, `gauss-sag`, `gauss-memory` | Single review + property tests; deps motivated in PR. |
| 2    | provider / channel / tool / canvas / gateway / health / cli / tui / desktop | Normal review.                                  |
| 3    | `gauss-conformance`, `gauss-bench`, `gauss-poly`          | Best-effort review.                            |

Workspace-level lints enforce `unsafe_code = "forbid"` for all crates;
explicit Tier-0 ADR is required to relax it.

PR labels mirror tiers (`tier-0`, `tier-1`, …); CI denies merging a `tier-0`
PR without two approvals from the Tier-0 reviewer set.

## Consequences

- Kernel reviews are intentionally slow. We accept that.
- The reviewer rotation needs explicit Tier-0 capacity; staffing sketch in
  ROADMAP.md §"Staffing Sketch" allocates 2 engineers to Kernel/privileged
  from Phase 0.
- Adding a new crate requires choosing its tier in the same PR, which forces
  upfront classification.
- Tier promotion (e.g. graduating a Phase-2 plugin to Tier 1) is itself an
  ADR.

## Alternatives considered

- **Flat review policy.** Operationally simple, but historically the cause
  of the documented incidents we want to rule out.
- **Per-file CODEOWNERS only.** Misses the lint-policy implications;
  CODEOWNERS handles routing but not enforcement.
