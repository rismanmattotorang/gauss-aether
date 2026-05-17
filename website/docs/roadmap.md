---
id: roadmap
title: Roadmap
sidebar_position: 10
---

# Roadmap

The full plan lives in
[`GAUSSCLAW_ROADMAP.md`](https://github.com/rismanmattotorang/gauss-aether/blob/main/GAUSSCLAW_ROADMAP.md)
at the repo root. Five phases, twenty-four weeks, four milestones
plus GA.

| Phase | Weeks | Milestone | Headline |
|---|---|---|---|
| **P1** Surfaces and channels | 1–4 | **M1** | CLI + TUI + web + desktop + channels in shim regime |
| **P2** Memory, receipts, lineage | 4–10 | **M2** | SQLite/FTS5 → Trinity over SurrealDB; Ed25519 receipt chain |
| **P3** Tools and sandbox | 10–16 | **M3** | Skill Manifest + `#[tool]`; HWCA + Composite Sandbox; IPI ≤ 2.19% |
| **P4** Providers + meta-routers | 16–20 | **M4** | 20 leaf + OpenRouter + NotDiamond under polyhedral contracts |
| **P5** Trajectory export + GA | 20–24 | **GA** | Cryptographic Envelope, Federated Pool, signed installers |

## Binding constraints

1. **Surface-Convergence Preservation** — every Hermes surface
   produces a behaviourally identical turn under GaussClaw.
2. **Trajectory schema bit-equality** — SFT/DPO JSONL preserved
   field-for-field; new material appended in an optional envelope.
3. **`@tool` decorator ergonomics** preserved literally.
4. **TOML config compatibility** — Hermes keys continue to work; new
   keys are optional.
5. **No axiom regression** — A1–A9 / T1–T12 conformance stays green
   on every PR.
