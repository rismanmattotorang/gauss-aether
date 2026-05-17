---
id: architecture
title: Architecture overview
sidebar_position: 8
---

# Architecture

GaussClaw is **19 single-responsibility Rust crates** on top of the
**22-crate Gauss-Aether runtime**. The agent crates handle every
user-facing surface — terminal, web, desktop, gateway, exporters. The
runtime crates handle every privileged primitive — kernel admit, turn
engine, receipt chain, sandbox.

The two layers communicate only through a small set of plugin traits.
You can swap any plugin without recompiling its consumers, and the
**polyhedral verifier** certifies the swap is behaviourally equivalent
on a probe set before it ships.

## The two layers

```text
GaussClaw — the agent                                       (gaussclaw-*)
├── -cli  · -tui  · -web  · -desktop                        ← surfaces
├── -surfaces · -channels                                   ← REST / WS / OAI / messaging
├── -agent · -store · -config · -migrate                    ← core loop
├── -skill · -tools                                         ← tool ecosystem
├── -providers · -providers-meta · -api-modes               ← LLM connectivity
├── -export · -fed                                          ← trajectory pipeline
└── -conformance · -bin                                     ← parity tests + shipping binary

─────────────────────────────────────────────────────────────────────────

Gauss-Aether — the runtime                                  (gauss-*)
├── -canvas · -health · -gateway                            ← surface plumbing
├── -poly · -bench                                          ← verifier + scorecard
├── -sag · -audit                                           ← autonomy + audit
├── -hwca · -sandbox · -memory                              ← workers + memory
├── -turn · -provider                                       ← turn engine
├── -kernel · -traits · -core                               ← kernel + types
└── -attest · -chaos · -zk · -dp · -learnt · -robust        ← hardening + research
```

## How the agent rides the runtime

Every GaussClaw crate consumes one or more runtime traits — it never
re-implements a runtime primitive.

| Agent crate | Consumes | What that buys you |
|---|---|---|
| `gaussclaw-agent` | `Kernel`, `Provider`, `MemoryBackend` | Differential Turn Engine — WAL barrier before any tool call. |
| `gaussclaw-store` | `MemoryBackend` + `gauss-audit` | Trinity store + tamper-evident receipt chain. |
| `gaussclaw-tools` | `SandboxTrait` + HWCA worker | Tools run inside a 4-layer sandbox by default. |
| `gaussclaw-skill` | `CapToken`, `TaintLabel` | Manifest → kernel binding; capability and taint declared up front. |
| `gaussclaw-providers*` | `ProviderTrait`, `gauss-poly` | Build-time polyhedral equivalence between providers. |
| `gaussclaw-surfaces` · `-channels` · `-tui` · `-web` · `-desktop` | `gauss-gateway` | Every surface shares one event loop with a fair three-plane scheduler. |
| `gaussclaw-export` | `gauss-audit::ReceiptChain` + `gauss-attest` | SFT/DPO trajectories signed with a Cryptographic Envelope. |

## The safety story, in one screen

| Property | Crate | Mechanism |
|---|---|---|
| Effects fire only after the WAL commits | `gauss-turn` | Differential Turn Engine encodes phase order in the type system. |
| Capabilities can only shrink | `gauss-kernel` | CAS-protected admit gate; growth is a compile-time refusal. |
| Mutating an audit row diverges the chain | `gauss-audit` | SHA-256 Merkle chain + Ed25519 + RFC 3161 / OpenTimestamps anchor. |
| Three planes have a starvation bound | `gauss-kernel::sched` | Lock-free token-budget scheduler with `B/ρ` guarantee. |
| Memory composition is monoidal, recall is hybrid | `gauss-memory` | Trinity = append log + BM25 + HNSW + K-LRU + Myers diff. |
| Information flow respects taint | `gauss-kernel::flow` | Lattice ℒ with antitone declassification. |
| Tool output can't smuggle instructions | `gauss-hwca` | Four-stage schema gate at the worker boundary. |
| Autonomy is gated by an auditable table | `gauss-sag` | Monotone `DecisionTable` floored by an optional learnt scorer. |
| Receipts are EUF-CMA non-repudiable | `gauss-audit::sign` | Ed25519 signatures cover every canonical payload. |
| Providers are swap-equivalent on a probe set | `gauss-poly` | Polyhedral trait-equivalence verifier. |
| Sandbox compromise is mathematically bounded | `gauss-sandbox` | `Pr[c] ≤ Π pᵢ + p_T ≤ 1.1 × 10⁻⁷`. |

Every line is backed by a property test in
[`gauss-conformance`](https://github.com/rismanmattotorang/gauss-aether/tree/main/gauss-aether/crates/gauss-conformance).
The 299-test suite re-runs on every PR and finishes in about three
seconds.

## Read more

- [**Kernel admit gate**](./architecture/kernel-gate) — how every surface admit-gates a tool call.
- [**Three-plane scheduler**](./architecture/three-plane) — conversation / daemon / approval pools.
- [**Receipt chain**](./architecture/audit-chain) — the cryptographic audit trail.
