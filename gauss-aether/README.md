# Gauss-Aether

**Gauss-Aether** is the Rust runtime framework that anchors this repository — a kernel-mediated execution substrate for safety-critical AI agents.

This directory holds the framework's source, specification, and proofs. The companion directory [`../gaussclaw/`](../gaussclaw/) holds the **GaussClaw** agent built on top of it.

## Reference documents

| File | Purpose |
|---|---|
| [`SPECS.md`](./SPECS.md) | Normative specification: the 9 Axioms (A1–A9), 12 Theorems (T1–T12), and the trait surface every runtime impl must satisfy. |
| [`ROADMAP.md`](./ROADMAP.md) | Phase-by-phase build plan. Phases 0–11 are merged on `main`; phases 12+ track research vehicles (zk, DP, learnt-Φ). |
| [`Gauss-Aether.pdf`](./Gauss-Aether.pdf) | Architecture paper. Definitions, theorems, proof sketches. |
| [`proofs/`](./proofs/) | Mechanised proof scaffolding (Coq/Lean extraction targets). |

## Crates

The framework ships as a Cargo workspace of 22 single-responsibility crates under [`crates/`](./crates/). The most-frequently-touched ones:

| Crate | Role |
|---|---|
| [`gauss-core`](./crates/gauss-core/) | Capability tokens, taint labels, observation types, the lattice algebra `𝒦` and `ℒ`. |
| [`gauss-traits`](./crates/gauss-traits/) | Public trait surface: `Kernel`, `MemoryBackend`, `Provider`, `ToolTrait`, `ChannelTrait`, `SecretStore`. |
| [`gauss-kernel`](./crates/gauss-kernel/) | `PrivilegedKernel` admit gate, three-plane scheduler, declassification map. |
| [`gauss-memory`](./crates/gauss-memory/) | SurrealDB-backed BM25 + HNSW + Merkle-chain "Trinity" store. |
| [`gauss-audit`](./crates/gauss-audit/) | Ed25519 signed receipts, SHA-256 chain, RFC 3161 / OpenTimestamps TSA anchoring. |
| [`gauss-sandbox`](./crates/gauss-sandbox/) | Composite Sandbox abstraction: WASM L1 + Landlock L2 + seccomp L3 + bwrap L4. |
| [`gauss-hwca`](./crates/gauss-hwca/) | Hardware-enforced compute attestation: worker spawner with JSON-schema gate at the boundary. |
| [`gauss-sag`](./crates/gauss-sag/) | Supervised Autonomy Gradient — approval-plane gate for tool calls with `tool_approval_required = true`. |
| [`gauss-poly`](./crates/gauss-poly/) | Polyhedral equivalence verifier — build-time provider-swap-compatibility check (Theorem T7). |
| [`gauss-health`](./crates/gauss-health/) | Self-Diagnosable Health Engine — the 7 SPECS §XIII.C invariants. |
| [`gauss-gateway`](./crates/gauss-gateway/) | Three-plane scheduler glue: Conversation / Daemon / Approval. |
| [`gauss-attest`](./crates/gauss-attest/) | Secret store + Ed25519 key release for the desktop updater and channel adapters. |

Research vehicles (deferred to post-GA, see `ROADMAP.md`):

- `gauss-zk` — zk-SNARK trajectory envelopes.
- `gauss-dp` — differential-privacy noise on export.
- `gauss-learnt` — learnt-Φ adaptive autonomy gradient.
- `gauss-chaos` / `gauss-bench` / `gauss-robust` — chaos injectors, benchmark harnesses, robustness ring.

## Status

Phases 0–11 ship on `main`. The framework is the substrate every `gaussclaw-*` crate binds to via [`gauss-traits`](./crates/gauss-traits/) — see [`../gaussclaw/`](../gaussclaw/) for the user-facing agent.
