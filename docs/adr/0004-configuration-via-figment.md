# ADR-0004 — Configuration via `figment` (TOML + env + CLI)

**Status:** Accepted (Phase 0)
**Date:** 2026-05-16

## Context

Gauss-Aether needs layered configuration:

- Operator-supplied TOML on disk (`/etc/gauss/gauss.toml` or
  `~/.gauss/config.toml`).
- Environment-variable overrides (`GAUSS_*`) for container deployments.
- CLI flags for one-off overrides during development and debugging.

Secrets — provider API keys, signing keys — MUST NOT live in the on-disk
TOML; they belong to a keyring or environment variable.

## Decision

Configuration is parsed by the **`figment`** crate, with the layered
provider stack:

```
TOML(file)  →  Env(GAUSS_)  →  CLI flags
   lowest precedence            highest precedence
```

Each crate exposes its config struct annotated with `serde::Deserialize`; the
binary (`gauss-cli`) wires the layers and dispatches.

Secrets are loaded **separately** — never via figment — through:

- The OS keyring (`keyring` crate) in development.
- Mounted file paths or environment variables in production.
- An optional HSM trait from Phase 5 onward.

## Consequences

- One config crate, one mental model, layered overrides for ops.
- Secrets never appear in serialised config dumps; `gauss doctor` is allowed
  to print non-secret fields.
- Adding a new config knob = new field on the typed struct + an entry in the
  reference config. No string parsing in business logic.
- `figment` is a Rocket project; the maintenance pulse is healthy.

## Alternatives considered

- **`config-rs`.** Comparable layering, less ergonomic for typed override
  precedence and provider chaining.
- **`clap` derive + manual env loading.** Works but layering becomes
  hand-rolled; the third operator-supplied layer (`/etc/gauss/`) is the one
  that always slips.
- **Pure environment variables.** Painful for nested config trees and
  encourages stringly-typed code.
