# Gauss-Aether · GaussClaw

**An axiomatic operating system for trustworthy autonomous LLM agents,
and the agent that runs on top of it — one Rust workspace, two
projects.**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/Rust-1.83+-orange.svg)](rust-toolchain.toml)
[![Tests](https://img.shields.io/badge/tests-299%20passing-brightgreen.svg)]()
[![Runtime](https://img.shields.io/badge/Gauss--Aether-1.0%20ready-blueviolet.svg)]()
[![Agent](https://img.shields.io/badge/GaussClaw-planning%20%2B%20scaffolding-orange.svg)](GAUSSCLAW_ROADMAP.md)

This repository houses two projects that ship together:

| Project | What it is | Status |
|---|---|---|
| **Gauss-Aether** | The runtime — an axiomatic OS-style kernel for LLM agents whose safety invariants are mechanically checked, not policy documents. 22 `gauss-*` crates. | 1.0 ready · 299 tests green |
| **GaussClaw** | The agent on top — a Rust port of the [Hermes agent](https://github.com/nousresearch/hermes-agent) that preserves every Hermes ergonomic primitive while running every turn under Gauss-Aether's kernel discipline. 19 `gaussclaw-*` crates. | Plan + scaffolding ([roadmap](GAUSSCLAW_ROADMAP.md)) |

Gauss-Aether is a clean-room reimplementation of an LLM agent runtime
whose **safety invariants are mechanically checked**, not policy
documents. Every privileged operation traces back to a numbered axiom
or theorem from the source paper; every theorem has a property test in
`gauss-conformance`; every plugin slots into a typed trait surface
that the polyhedral verifier proves swap-compatible.

GaussClaw is the working agent that demonstrates the runtime end-to-end:
it ports every Hermes Python module (CLI, TUI, REST, WebSocket,
OpenAI-compatible relay, ~20 messaging channels, 20+ provider drivers,
~30 first-party tools, SFT/DPO trajectory export) into Rust, lifts each
into the matching Gauss-Aether subsystem, and adds a Tauri 2 desktop
shell that strictly dominates Hermes Desktop on installer size, RAM,
cold start, and code-signing posture.

If you've built or operated an LLM agent and felt the gap between
"please don't do bad things" and "the type system makes bad things
unreachable," Gauss-Aether is what that gap looks like when closed —
and GaussClaw is what an agent looks like when it lives on the other
side.

---

## At a glance

| Question                                  | Answer                                                                                 |
|-------------------------------------------|----------------------------------------------------------------------------------------|
| Will my agent ever do something before logging it?    | No — the WAL append is the *only* path to the side-effect commit (Axiom A1).                     |
| Can a tool escalate its capabilities?     | No — the kernel's grant can only shrink (`contract`); growth is a compile-time refusal (A2).       |
| Can an instruction-injection prompt cross the worker boundary? | No (≤ 2.19 % theoretical; 0/20 empirical on the Phase-4 corpus) — schema gate (A7/T9).             |
| Can the audit log be tampered with?       | Not without leaving a Merkle-divergent trail (T3) and an Ed25519 signature failure (T11).        |
| Can I swap providers without breaking the deployment? | Yes — the polyhedral verifier (T7) certifies the swap on a probe set before you ship it.        |
| Can I prove this in Lean / Coq?           | Yes — the v2 horizon ships stub theorems against the same type signatures (see `proofs/lean/`).  |

---

## What's in the box

The workspace contains **41 crates** across two layers plus a Lean-4
proof skeleton and a Docusaurus website tree:

- **22 `gauss-*` crates** — the Gauss-Aether runtime (this section).
- **19 `gaussclaw-*` crates** — the GaussClaw agent (see
  [§ GaussClaw](#gaussclaw--the-agent-on-top) below and the full
  [`GAUSSCLAW_ROADMAP.md`](GAUSSCLAW_ROADMAP.md)).

The `gauss-*` crates partition into seven layers:

```text
┌──────────────────────────────────────────────────────────────────┐
│ surface layer (Phase 9)                                              │
│   gauss-canvas · gauss-health · gauss-gateway                        │
├──────────────────────────────────────────────────────────────────┤
│ verifier + scorecard (Phase 8/11)                                    │
│   gauss-poly · gauss-bench                                           │
├──────────────────────────────────────────────────────────────────┤
│ autonomy + audit (Phases 5/7)                                        │
│   gauss-sag · gauss-audit                                            │
├──────────────────────────────────────────────────────────────────┤
│ memory + workers + sandbox (Phases 3/4/6)                            │
│   gauss-memory · gauss-hwca · gauss-sandbox                          │
├──────────────────────────────────────────────────────────────────┤
│ turn engine (Phase 2)                                                │
│   gauss-turn · gauss-provider                                        │
├──────────────────────────────────────────────────────────────────┤
│ kernel + traits + core (Phase 1)                                     │
│   gauss-kernel · gauss-traits · gauss-core                           │
├──────────────────────────────────────────────────────────────────┤
│ hardening + research (Phase 10 + v2 horizon)                         │
│   gauss-attest · gauss-chaos · gauss-zk · gauss-dp · gauss-learnt ·  │
│   gauss-robust                                                       │
└──────────────────────────────────────────────────────────────────┘
```

The full crate purposes:

| Crate                | Purpose                                                                              |
|----------------------|--------------------------------------------------------------------------------------|
| `gauss-core`         | Identifiers, actions, observations, taint, `CapToken` lattice, unified error.        |
| `gauss-traits`       | Plugin trait surface (`Kernel`, `MemoryBackend`, `Provider`, `SandboxTrait`, `ToolTrait`). |
| `gauss-kernel`       | Privileged authority — joint K×L admission, lock-free 3-plane scheduler, consistent-hash cluster ring. |
| `gauss-turn`         | Differential Turn Engine — Algorithm 1 with optional sandbox + signed receipts + SAG. |
| `gauss-memory`       | Trinity Memory: SurrealDB-backed append log + BM25 + HNSW hybrid recall + K-LRU + Myers diff. |
| `gauss-audit`        | SHA-256 chain + Ed25519 signed receipts + RFC 3161 / OpenTimestamps anchors + verifier API. |
| `gauss-provider`     | Provider adapters — `ToyProvider` ships now; vendor adapters land as plugin crates.   |
| `gauss-sandbox`      | Composite sandbox: WASM (wasmi) + Landlock + seccomp + bwrap + Seatbelt; TEE-attest feature. |
| `gauss-hwca`         | HWCA worker contexts + four-stage schema gate + IPI corpus (A7, T9).                  |
| `gauss-sag`          | Supervised Autonomy Gradient — `DecisionTable` + monotonicity verifier + approval surfaces. |
| `gauss-poly`         | Polyhedral trait-equivalence verifier (T7).                                           |
| `gauss-canvas`       | A2UI Live Canvas Protocol — typed widget tree + update stream (T8).                   |
| `gauss-health`       | Self-Diagnosable Health Engine — seven minimum invariants + custom registration.      |
| `gauss-gateway`      | REST/WS/SSE wire types + OpenAI-compatible proxy schema.                              |
| `gauss-attest`       | TEE attestation trait + Ed25519 software simulator (T10 §L4).                         |
| `gauss-chaos`        | Chaos-engineering harness — deterministic kill / partition / clock-skew injectors.    |
| `gauss-bench`        | Pareto-dominance scorecard + 15-axis comparison.                                      |
| `gauss-zk`           | Zero-knowledge proofs over the receipt chain (v2 horizon).                            |
| `gauss-dp`           | Differentially-private trajectory exporter — Laplace + Gaussian mechanisms.           |
| `gauss-learnt`       | Learnt risk classifier Φ̂ — logistic scorer wrapping the SAG rule table.              |
| `gauss-robust`       | Robust declassifiers — adversarially-adaptive taint downgrades.                       |
| `gauss-conformance`  | Axiom-by-axiom test harness (A1–A9, T1–T12).                                          |

---

## GaussClaw — the agent on top

GaussClaw ports the [Hermes](https://github.com/nousresearch/hermes-agent)
agent into Rust and binds it to the Gauss-Aether kernel without losing a
single Hermes ergonomic primitive. The plan and exit criteria live in
[`GAUSSCLAW_ROADMAP.md`](GAUSSCLAW_ROADMAP.md); the crate skeletons live
in `crates/gaussclaw-*`.

### Crate map

| Crate | Replaces (Hermes) | Phase |
|---|---|---|
| `gaussclaw-agent` | `agent.AIAgent.run_conversation` | P1 |
| `gaussclaw-cli` | `hermes` CLI (clap v4 subcommand parity) | P1 |
| `gaussclaw-tui` | `ui-tui/` React + Ink stack (Ratatui + crossterm) | P1 |
| `gaussclaw-web` | `web/` FastAPI + PTY (Axum + retained React frontend) | P1 |
| `gaussclaw-desktop` | Hermes Desktop Electron 39 app (Tauri 2 + Rust) | P1 / P5 |
| `gaussclaw-surfaces` | REST · WebSocket · OpenAI-compat relay | P1 |
| `gaussclaw-channels` | `channels/*` (~20 messaging adapters) | P1 |
| `gaussclaw-store` | `store.session` SQLite/FTS5 + `store.lineage` | P2 |
| `gaussclaw-skill` | `@tool` decorator + Skill Manifest | P3 |
| `gaussclaw-tools` | `tools/*` (~30 first-party tools, HWCA-lifted) | P3 |
| `gaussclaw-providers` | `backends/*` (20 leaf vendor drivers) | P4 |
| `gaussclaw-providers-meta` | OpenRouter aggregator + NotDiamond router | P4 |
| `gaussclaw-api-modes` | `api_modes/*` (chat-completion · responses · oai-compat) | P4 |
| `gaussclaw-export` | `export.sft` + `export.dpo` + Cryptographic Envelope | P5 |
| `gaussclaw-fed` | Federated Trajectory Pool (new) | P5 |
| `gaussclaw-config` | `config/*` TOML loader (Hermes-compatible) | P1 |
| `gaussclaw-migrate` | `gaussclaw import hermes <path>` | P1 |
| `gaussclaw-conformance` | Hermes-parity replay + OAI SDK + CLI/TUI/web/desktop e2e | All |
| `gaussclaw-bin` | The shipping `gaussclaw` binary | All |
| `website/` | Docusaurus (en + zh-Hans) + mdBook API reference | P1 / GA |

### Why GaussClaw on Gauss-Aether

Every Hermes architectural deficit closes against a Gauss-Aether
subsystem with a theorem behind it. See the GaussClaw paper §III for
the full mapping; the headline rows:

| Hermes deficit | Gauss-Aether subsystem that closes it | Theorem |
|---|---|---|
| Tool fn runs in host interpreter with all credentials | `gauss-kernel` capability lattice 𝒦 + `gauss-sandbox` Composite Sandbox | T9, T10 |
| SQLite store mutable; no signed record | `gauss-memory` Trinity over SurrealDB + `gauss-audit` Receipt Chain (Ed25519) | T3, T11 |
| Tool text → next prompt verbatim | `gauss-hwca` worker context + schema-validated value | T9 |
| No taint on web/email observations | `gauss-kernel::flow` info-flow lattice ℒ + declass map | A6 |
| Background + user turns share one event loop | `gauss-kernel::sched` three-plane scheduler (conv / daemon / approval) | T4 |

### GaussClaw target numbers (at GA)

| Metric | Hermes baseline | GaussClaw target | Mechanism |
|---|---|---|---|
| IPI attack success rate | not measured (no defence) | ≤ 2.19 % | T9 + HWCA + ℒ |
| Cold start (warm cache) | 80–150 ms (Python import) | ≤ 10 ms | T12 delta-encoded + K-LRU |
| Composite sandbox compromise | ~ 1 (no sandbox) | ≤ 1.1 × 10⁻⁷ | T10 + TEE |
| Hybrid recall miss rate | 0.08 (FTS5 only) | ≤ 0.015 | T5: ε_fts · ε_vec |
| Receipt forgery probability | no receipts | negl(λ) | T11 EUF-CMA + collision |
| Provider switching | manual retest | build-time verified | T7 + polyhedral equiv. |
| Desktop installer size | ~150 MB (Electron 39) | ≤ 20 MB (Tauri 2) | OS WebView, no Chromium |
| Desktop RAM idle | ~250 MB | ≤ 80 MB | OS WebView |
| Desktop code-signing | unsigned on all OSes | signed + notarized on all 3 OSes | CI signing + `gauss-attest` |

---

## Quick start

Requires **Rust 1.83+** (workspace MSRV). All major OSes; Linux gets
the full sandbox stack, macOS uses Seatbelt for L2.

```bash
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether

# Build the whole workspace + run the conformance suite.
cargo test --workspace

# Tighten quality gates the way CI does.
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

The 299-test conformance suite runs in ~3 seconds on a modern laptop.

A walkthrough of the typical client embed:

```rust
use std::sync::Arc;
use gauss_core::{CapToken, TurnId};
use gauss_kernel::PrivilegedKernel;
use gauss_memory::SurrealMemory;
use gauss_provider::ToyProvider;
use gauss_sag::{ApprovalGate, default_decision_table, AutoApprove};
use gauss_turn::{TurnEngine, TurnInput};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let memory   = Arc::new(SurrealMemory::open_in_memory().await?);
    let kernel   = Arc::new(PrivilegedKernel::new(CapToken::TOP));
    let provider = Arc::new(ToyProvider::always_text("hi from gauss-aether!"));
    let sag      = Arc::new(ApprovalGate::new(
        default_decision_table(),
        AutoApprove::new("operator"),
    ));

    let engine = TurnEngine::new(kernel, memory.clone(), provider)
        .with_sag(sag);

    let summary = engine.run_turn(TurnInput {
        id:  TurnId::new(1),
        obs: /* your observation */ todo!(),
    }).await?;

    println!("chain head = {}", hex::encode(summary.chain_head.digest));
    Ok(())
}
```

See [`docs/QUICKSTART.md`](docs/QUICKSTART.md) for a full deployment
walkthrough including SAG approval round-trip, signed receipts, and
the gateway wire types.

---

## How safety is enforced

Gauss-Aether's safety story is structural, not behavioural. The Rust
type system is the proof carrier; the conformance suite is the
property-test witness; the polyhedral verifier is the plugin-swap
witness; the Lean 4 stubs are the formal-proof carrier (v2 horizon).

The eleven-tuple `G = (S, A, O, K, M, F, π, L, Φ, R, V)` from the
source paper maps onto the crate graph as:

```text
S  ← gauss-core (state types)
A  ← gauss-core (action enum)
O  ← gauss-core (observation)
K  ← gauss-kernel (capability lattice)
M  ← gauss-memory (trinity substrate)
F  ← gauss-audit (receipt chain)
π  ← gauss-provider + gauss-poly (policy + adjunction)
L  ← gauss-core::TaintLabel + gauss-kernel::flow
Φ  ← gauss-sag (rule table) + gauss-learnt (learnt scorer)
R  ← gauss-canvas + gauss-gateway (rendering)
V  ← gauss-conformance + gauss-poly (verification)
```

The axioms / theorems:

| ID         | What it says                                                                 | Where it lives           | Conformance test                                                |
|------------|------------------------------------------------------------------------------|--------------------------|-----------------------------------------------------------------|
| **A1**     | External effects fire only after the WAL append durably succeeds.            | `gauss-turn`             | `axiom_a1_wal_before_effect`                                    |
| **A2 / T2**| Capabilities monotonically shrink under `contract`; CAS-protected.           | `gauss-kernel::admit`    | `axiom_a2_capability_monotonicity`                              |
| **A3 / T3**| Modifying any payload diverges the chain head (Merkle).                      | `gauss-audit::chain`     | `theorem_t3_merkle_tamper_evidence`                             |
| **A4 / T4**| Three-plane scheduler has a `B/ρ` starvation bound.                          | `gauss-kernel::sched`    | `theorem_t4_starvation_bound` + `axiom_a4_fairness_separation`  |
| **A5 / T5 / T12** | Memory monoid laws + hybrid recall + delta warm-switch.                | `gauss-memory`           | `axiom_a5_memory_monoid`, `theorem_t5_*`, `theorem_t12_*`       |
| **A6**     | Information-flow lattice with antitone declass.                              | `gauss-kernel::flow`     | `axiom_a6_taint_lattice`                                        |
| **A7 / T9**| Worker-context isolation + IPI containment ≤ 2.19 %.                         | `gauss-hwca`             | `axiom_a7_and_theorem_t9_hwca`                                  |
| **A8**     | Supervised-autonomy gradient — monotone risk classifier.                     | `gauss-sag`              | `axiom_a8_sag_approval`                                         |
| **A9 / T11**| Ed25519 EUF-CMA signatures + receipt non-repudiation.                       | `gauss-audit::sign`      | (signing crate tests cover EUF-CMA)                              |
| **T6**     | Stateless-turn scaling via consistent-hash routing.                          | `gauss-kernel::cluster`  | `theorem_t6_stateless_scaling_and_attest`                       |
| **T7**     | Provider adjunction — polyhedral equivalence on probe set.                   | `gauss-poly`             | `theorem_t7_provider_adjunction`                                |
| **T8**     | Pareto-dominance scorecard (15-axis comparison against predecessors).        | `gauss-bench` + Phase 9  | `phase11_release` + `theorem_t8_pareto_dominance`               |
| **T10**    | Composite sandbox bound — `Pr[compromise] ≤ Π pᵢ + p_T`.                     | `gauss-sandbox`          | `theorem_t10_composite_sandbox`                                 |
| **T10 §L4**| TEE attestation simulator + verifier.                                        | `gauss-attest`           | `theorem_t6_stateless_scaling_and_attest` (T10-L4 row)          |

---

## v2 horizon

Five research-track crates ship behind a stable contract; production
plugins (real SNARK provers, hardware DP sources, vendor LLMs, hardware
attestation backends) implement these trait surfaces as additive
crates.

| Crate              | What it's the contract for                                                 |
|--------------------|----------------------------------------------------------------------------|
| `gauss-zk`         | Zero-knowledge proofs over the receipt chain (Merkle commitments + statements). |
| `gauss-dp`         | Differentially-private trajectory exporter (Laplace + Gaussian).            |
| `gauss-learnt`     | Learnt risk classifier `Φ̂` (logistic scorer) — *floors* the SAG rule table. |
| `gauss-robust`     | Robust declassifiers — adversarial-rejection counters tighten the declass map. |
| `gauss-bench`      | 15-axis Pareto-dominance scorecard (used by the Phase-11 release gate).    |
| `proofs/lean/`     | Lean 4 stubs of all nine axioms + twelve theorems; proofs land incrementally. |

---

## Documentation

**Gauss-Aether (runtime).**

- **[`SPECS.md`](SPECS.md)** — normative engineering specification (the source paper's recipe).
- **[`ROADMAP.md`](ROADMAP.md)** — phased development plan, axiom / theorem locks per phase.
- **[`docs/QUICKSTART.md`](docs/QUICKSTART.md)** — end-to-end embed walkthrough.
- **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** — crate-by-crate architecture tour.
- **[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md)** — contributor guide + the `specT` style guide.
- **[`docs/SECURITY.md`](docs/SECURITY.md)** — threat model + responsible-disclosure policy.
- **[`docs/adr/`](docs/adr/)** — sixteen architecture decision records (ADRs 0001–0016).
- **[`proofs/lean/README.md`](proofs/lean/README.md)** — mechanised-proof skeleton + roadmap.

**GaussClaw (agent).**

- **[`GaussClaw.pdf`](GaussClaw.pdf)** — the source paper: full architectural plan + theorems.
- **[`GAUSSCLAW_ROADMAP.md`](GAUSSCLAW_ROADMAP.md)** — 5-phase, 24-week, milestoned port plan with exit criteria, rollback paths, and dependency edges.
- **[`website/README.md`](website/README.md)** — Docusaurus content tree + mdBook API reference.

---

## Quality gates

CI enforces, on every PR:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings` (with `clippy::pedantic` + `clippy::nursery`)
- `cargo test --workspace` — currently **299 tests**, ~3 s on a modern laptop.
- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings`.
- `cargo deny check` — supply-chain + license gate.
- MSRV check on 1.83.

The release gate adds a 15-axis Pareto-dominance assertion (the
`phase11_release::one_point_zero_pareto_dominates_every_predecessor`
test).

---

## Design tenets

1. **Axioms before features.** Every subsystem traces back to an axiom or theorem in the paper.
2. **No `unsafe` in privileged crates.** Workspace lint: `unsafe_code = "forbid"`.
3. **Lock-free where the CAS pattern is clean.** Three-plane scheduler packs `(tokens, epoch_ms)` into one `AtomicU64`.
4. **Property tests + type-state.** Lattice laws + chain integrity proptested; the DTE encodes phase ordering in the type system.
5. **`#[non_exhaustive]` everywhere.** Field/variant additions stay semver-minor.
6. **WAL barrier is structural.** Tool execution is *unreachable* until `memory.append(...)` returns — A1 by construction.
7. **Worker boundary is structural.** Only the `ValidatedValue` crosses back to the parent — A7 by construction.
8. **Receipts cover the SAG verdict.** Approval decisions are part of the canonical signed payload — A8 ∧ T11 by construction.
9. **Recall is monoidal + hybrid.** Memory composition is associative; recall fuses BM25 ∪ HNSW with weighted union — A5 ∧ T5.
10. **Autonomy is gated by an auditable table.** A monotone `DecisionTable` is the policy floor; the learnt scorer can only tighten it.

---

## Licence

MIT — see [`LICENSE-MIT`](LICENSE-MIT).

The project was originally dual-licensed (Apache-2.0 OR MIT) through
Phase 10; the Phase-11 1.0 release pin simplified to MIT-only for
plugin-ecosystem clarity (ADR-0017).

---

## Contributing

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md). The short version:

- Every PR cites the axiom / theorem / SPECS §it advances.
- Every new plugin trait follows the four-rule `specT` style guide
  (serializable outputs, `#[non_exhaustive]`, unified `GaussError`,
  probe-set-checkable invariants — ADR-0014).
- Tier-0 changes (`gauss-kernel`, `gauss-audit`, `gauss-attest`) need
  dual review.

---

## Citing

If you use Gauss-Aether in research or production, please cite:

```bibtex
@software{gauss_aether_2026,
  author       = {Gauss-Aether Contributors},
  title        = {Gauss-Aether: An Axiomatic OS for Trustworthy LLM Agents},
  year         = 2026,
  url          = {https://github.com/rismanmattotorang/gauss-aether},
  license      = {MIT}
}
```

---

## Acknowledgements

Gauss-Aether builds on lessons from OpenClaw, ZeroClaw, OpenFang, and
Hermes — its 15-axis scorecard exists precisely to make
"successor-of" a falsifiable claim instead of a marketing one.
GaussClaw, in turn, is the working demonstration that a Hermes-style
agent and a Gauss-Aether-style kernel can occupy the same process
without either side losing what makes it valuable. The
source paper's axiom + theorem numbering is preserved verbatim across
the SPECS, the conformance suite, the ADRs, and the Lean stubs.
