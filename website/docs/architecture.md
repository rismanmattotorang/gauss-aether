---
id: architecture
title: Architecture
sidebar_position: 8
---

# Architecture

GaussClaw is 19 Rust crates on top of the 22-crate Gauss-Aether 1.0
runtime. The combined workspace lives at
[github.com/rismanmattotorang/gauss-aether](https://github.com/rismanmattotorang/gauss-aether).

## Layer cake

```
GaussClaw — the agent on top                              (gaussclaw-*)
├── gaussclaw-cli · -tui · -web · -desktop · -surfaces · -channels
├── gaussclaw-store
├── gaussclaw-skill · -tools
├── gaussclaw-providers · -providers-meta · -api-modes
├── gaussclaw-export · -fed
└── gaussclaw-agent · -config · -migrate · -conformance · -bin

──────────────────────────────────────────────────────────────────────

Gauss-Aether runtime                                      (gauss-*)
├── gauss-canvas · -health · -gateway                      (surface)
├── gauss-poly · -bench                                    (verifier)
├── gauss-sag · -audit                                     (autonomy)
├── gauss-hwca · -sandbox · -memory                        (workers)
├── gauss-turn · -provider                                 (turn engine)
├── gauss-kernel · -traits · -core                         (kernel)
└── gauss-attest · -chaos · -zk · -dp · -learnt · -robust  (hardening / v2)
```

## How GaussClaw sits on Gauss-Aether

Every GaussClaw crate consumes one or more runtime traits without
re-deriving runtime primitives.

| GaussClaw | Consumes | Subsystem |
|---|---|---|
| `gaussclaw-agent` | `Kernel`, `Provider`, `MemoryBackend` | DTE turn policy |
| `gaussclaw-store` | `MemoryBackend` + `gauss-audit` | Trinity + receipt chain |
| `gaussclaw-tools` | `SandboxTrait` + HWCA worker spawn | Capability-gated execution |
| `gaussclaw-skill` | `gauss-core::CapToken` + `TaintLabel` | Manifest → kernel binding |
| `gaussclaw-providers*` | `ProviderTrait` + `gauss-poly` | Polyhedral equivalence |
| `gaussclaw-surfaces` / `-channels` / `-tui` / `-web` / `-desktop` | `gauss-gateway` three-plane router | Surface convergence |
| `gaussclaw-export` | `gauss-audit::ReceiptChain` + `gauss-attest` | Cryptographic Trajectory Envelope |

## Read more

- **[Kernel gate](architecture/kernel-gate)** — how every surface admit-gates.
- **[Three-plane routing](architecture/three-plane)** — Conversation / Daemon / Approval scheduler.
- **[Audit chain](architecture/audit-chain)** — tamper-evident WAL.
