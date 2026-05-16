# ADR-0008 — Canonical `CapToken` lives in `gauss-core`

**Status:** Accepted (Phase 2)
**Date:** 2026-05-16
**Supersedes parts of:** SPECS §4.1 (originally placed `CapToken` in `gauss-kernel`)

## Context

Phase 0 / 1 placed `CapToken` inside `gauss-kernel` to colocate it with the
lattice laws and proptest coverage. Phase 2's Differential Turn Engine
needs `Action::Tool::cap_required: CapToken` to bind a tool's required cap
inside the tool manifest — but `Action` lives in `gauss-core`, which cannot
depend on `gauss-kernel` (that would invert the dependency graph: kernel
depends on core, not the other way round).

Several workarounds were considered:

1. Carry the cap as raw `u64` bits in `gauss-core` and reify it in
   `gauss-kernel`. Loses type-safety at the action boundary.
2. Add a `CapShim` newtype in `gauss-core` and convert at every call site.
   Boilerplate, easy to forget.
3. Move the canonical `CapToken` definition into `gauss-core` and have
   `gauss-kernel` re-export it.

## Decision

**`CapToken` is defined in `gauss-core` (option 3).** `gauss-kernel`
re-exports it for source-compatibility and continues to own the lattice-law
proptest suite. `Action::Tool::cap_required` references the canonical type
directly; the engine's admission path uses it without any conversion.

## Consequences

- `gauss-traits::Kernel::admit` takes `gauss_core::CapToken` directly. The
  associated-type indirection (`type Cap = …`) is removed — it bought us
  nothing now that the type is universal across the workspace.
- The bitmask namespace and lattice ops remain identical; only the source
  file moved. No behaviour change.
- Plugin authors who only need to *carry* capability tokens (channels, MCP
  tool stubs, telemetry exporters) can now depend on `gauss-core` alone
  without pulling in the kernel crate.

## Alternatives considered

- See "Context" above.
- A fourth option would be a sealed-trait abstraction
  (`trait CapabilityBits: Sealed { fn bits(&self) -> u64; }`) implemented
  for the kernel's `CapToken` and consumed by `gauss-core`. Strictly more
  abstract; offers no concrete benefit at this scale.
