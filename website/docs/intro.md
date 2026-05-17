---
id: intro
title: GaussClaw
slug: /intro
sidebar_position: 1
---

# GaussClaw

**A Rust port of the [Hermes](https://github.com/NousResearch/hermes-agent) agent that runs on the Gauss-Aether axiomatic kernel.**

GaussClaw preserves every Hermes ergonomic primitive — the `@tool` decorator,
the TOML config schema, the CLI / TUI / web / channel surfaces, the SFT/DPO
trajectory export — and binds each to a Gauss-Aether subsystem with a
numbered theorem behind it. The result is a working agent that closes the
five Hermes architectural deficits with kernel discipline rather than
operator hope.

## Why this exists

Hermes is a delightful agent and a fragile substrate. Tool calls run in
the host interpreter with the agent's full credential set; the session
store is mutable and unsigned; web-fetched text becomes the next prompt
verbatim; background and user turns share one event loop; secrets are
read from `os.environ` and trusted forever. The trajectory flywheel
spins, but every revolution leaves a single point of failure.

GaussClaw drops the same agent into a kernel that was missing:

- **Tool dispatch is admit-gated and sandboxed.** Every `#[tool]` runs
  inside a Hierarchical Worker Context whose schema-validated value is
  the only thing that crosses back to the parent. IPI attack success
  rate target: **≤ 2.19%**.
- **The session store is a tamper-evident chain.** SQLite + FTS5
  becomes SurrealDB Trinity + Ed25519 receipts; any byte changed in
  any past entry diverges the chain head.
- **Web-fetched text never crosses the worker boundary.** Information
  flow is a lattice with antitone declassification — Web-tainted data
  cannot direct a `NETWORK_POST`.
- **Background, user, and approval turns have separate budget pools.**
  A three-plane scheduler with a `B/ρ` starvation bound.
- **Secrets resolve through an attested store, not raw env vars.**

## Where to start

- **[Installation](getting-started/installation)** — get the binary running.
- **[First run](getting-started/first-run)** — start the TUI, the dashboard, the desktop app.
- **[Migration from Hermes](getting-started/migration-from-hermes)** — `gaussclaw import hermes`.
- **[CLI reference](cli)** — every subcommand with examples.
- **[Architecture](architecture)** — how it all fits together.

## Status

GaussClaw is **Phase 1 / M1 in active development** on top of the
Gauss-Aether 1.0 trunk. See the
[roadmap](https://github.com/rismanmattotorang/gauss-aether/blob/main/GAUSSCLAW_ROADMAP.md)
for the full milestone plan.
