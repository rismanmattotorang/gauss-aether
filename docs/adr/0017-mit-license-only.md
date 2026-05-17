# ADR-0017 — Move to MIT-only licensing

**Status:** Accepted (Phase 11)
**Date:** 2026-05-16

## Context

Phases 0–10 dual-licensed Gauss-Aether as `Apache-2.0 OR MIT`. The
dual-licence was the Rust ecosystem's then-current default and let
plugin authors pick whichever clauses fit their downstream legal
constraints.

The Phase-11 1.0 release pin demanded:

1. **Single canonical license string** in every `Cargo.toml`'s
   `license` field, so SPDX scanners can compute the project's
   surface licence without disambiguation logic.
2. **Compatible-by-default** with the largest set of downstream
   licences — MIT is permissive and short, and has no patent-grant
   asymmetry with downstream consumers.
3. **Plugin clarity** — plugins ship under their own licence; mixing
   Apache-2.0's patent grant with MIT can create downstream
   confusion when the plugin author isn't paying close attention to
   the dual-licence semantics.

## Decision

Switch the workspace licence from `Apache-2.0 OR MIT` to **MIT-only**
for the 1.0 release. The change:

1. Sets `license = "MIT"` in the workspace's `Cargo.toml` (every
   member inherits via `license.workspace = true`).
2. Keeps `LICENSE-MIT` at the repository root.
3. Removes `LICENSE-APACHE` from the active licence surface (the file
   stays in the git history for the dual-licence era of Phases 0–10).
4. Updates `README.md`'s licence section.

## Consequences

- **Pro:** SPDX scanners report a single licence string for every
  crate.
- **Pro:** Plugin authors who copy our `specT` style guide don't
  inherit a confusing dual-licence story.
- **Pro:** Auditors who need to read the licence have one short file
  (the MIT text is ~170 words) instead of two.
- **Con:** Downstream users who relied on Apache-2.0's express patent
  grant now have MIT's implicit-patent-grant story (which is
  case-law-dependent in some jurisdictions). The mitigation is that
  the project as a whole has no patent claims; the explicit patent
  grant in Apache-2.0 was never an asymmetric value-add for our
  contributors.
- **Con:** Re-licensing requires every prior contributor's consent —
  for this project, all contributors so far are the original authors
  who explicitly consented to the relicensing via this ADR.

## Alternatives considered

- **Keep dual-license forever.** Status quo; rejected for SPDX
  clarity.
- **Switch to Apache-2.0-only.** Loses the brevity of MIT; gains an
  explicit patent grant the project doesn't need.
- **BSD-3-Clause.** Equivalent to MIT for our purposes, but less
  widely known in the Rust ecosystem.
- **AGPL-3.0.** Strong copyleft; rejected because Gauss-Aether is a
  library meant to be embedded in proprietary deployments.

## Migration

Pre-1.0 forks of Gauss-Aether retain their `Apache-2.0 OR MIT`
licence — the relicensing applies only to the 1.0 release and
forward. Downstream users who want to stay on the dual-licence era
can pin their dep at the last Phase-10 commit.
