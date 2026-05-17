# Gauss-Aether

**The verifiable runtime for trustworthy AI agents.**

Gauss-Aether is a kernel-mediated execution substrate for safety-critical
LLM agents. Where most agent frameworks treat safety as a prompt-engineering
concern, Gauss-Aether treats it as a *property of the type system*:
every privileged operation traces back to one of nine numbered axioms or
twelve theorems, each backed by a property test, each formalised as a
Lean 4 stub.

The companion directory [`../gaussclaw/`](../gaussclaw/) is the
reference agent built on top — a single-binary, Hermes-compatible agent
that demonstrates the runtime end-to-end. Embed Gauss-Aether directly
if you want to build something else.

[![Axioms](https://img.shields.io/badge/axioms-A1--A9-blueviolet.svg)]()
[![Theorems](https://img.shields.io/badge/theorems-T1--T12-blue.svg)]()
[![Tests](https://img.shields.io/badge/conformance-299%20green-brightgreen.svg)]()
[![Unsafe](https://img.shields.io/badge/unsafe__code-forbid-orange.svg)]()

---

## What it gives you

- **A capability lattice.** Tool calls pass an admit gate before they run. Capabilities can only shrink (`contract`); growth is a compile-time refusal. **Axiom A2.**
- **A WAL-first turn engine.** Side effects are unreachable until the write-ahead log commits. The Differential Turn Engine encodes phase ordering in the type system, so "log before effect" cannot be skipped. **Axiom A1.**
- **A cryptographic audit chain.** SHA-256 Merkle chain + Ed25519 signatures + RFC 3161 / OpenTimestamps anchoring. Tampering with any payload diverges the chain head and fails signature verification. **Theorems T3, T11.**
- **A four-layer composite sandbox.** WASM (wasmi) + Landlock + seccomp + bwrap on Linux; Seatbelt at L2 on macOS. Compromise probability bounded at `Pr[c] ≤ Π pᵢ + p_T ≤ 1.1 × 10⁻⁷`. **Theorem T10.**
- **An information-flow lattice.** Observations carry taint labels; the declassification map `d: ℒ → 𝒦` is verified antitone at startup. Web/email content can't reach high-privilege sinks. **Axiom A6.**
- **A schema-gated worker boundary.** Tool output is parsed and validated before re-entering the conversation, capping instruction-injection success at **≤ 2.19 %** (0 / 20 empirical). **Axiom A7, Theorem T9.**
- **A monotone autonomy gradient.** The Supervised Autonomy Gradient is an auditable decision table; the optional learnt scorer can only tighten it, never widen it. **Axiom A8.**
- **A polyhedral provider verifier.** Swap LLM providers at build time with a probe-set equivalence certificate. **Theorem T7.**
- **A lock-free three-plane scheduler.** Conversation / daemon / approval planes share an `AtomicU64` budget; starvation bounded by `B/ρ`. **Axiom A4, Theorem T4.**
- **Trinity Memory.** SurrealDB-backed append log + BM25 full-text + HNSW vector + K-LRU cache + Myers diff. Hybrid recall miss rate `ε_fts · ε_vec ≤ 0.015`. **Axiom A5, Theorem T5.**

---

## What's in the workspace

Twenty-two single-responsibility crates under [`crates/`](./crates/),
organised in seven layers:

```text
┌────────────────────────────────────────────────────────────────────┐
│ surface           gauss-canvas · gauss-health · gauss-gateway      │
├────────────────────────────────────────────────────────────────────┤
│ verifier          gauss-poly · gauss-bench                         │
├────────────────────────────────────────────────────────────────────┤
│ autonomy + audit  gauss-sag · gauss-audit                          │
├────────────────────────────────────────────────────────────────────┤
│ memory + work     gauss-memory · gauss-hwca · gauss-sandbox        │
├────────────────────────────────────────────────────────────────────┤
│ turn engine       gauss-turn · gauss-provider                      │
├────────────────────────────────────────────────────────────────────┤
│ kernel + traits   gauss-kernel · gauss-traits · gauss-core         │
├────────────────────────────────────────────────────────────────────┤
│ research          gauss-attest · gauss-chaos · gauss-zk · gauss-dp │
│                   gauss-learnt · gauss-robust                      │
└────────────────────────────────────────────────────────────────────┘
```

### Core crates

| Crate | Role |
|---|---|
| [`gauss-core`](./crates/gauss-core/) | Identifiers, actions, observations, taint labels, the `CapToken` lattice, unified `GaussError`. |
| [`gauss-traits`](./crates/gauss-traits/) | Public trait surface — `Kernel`, `MemoryBackend`, `Provider`, `SandboxTrait`, `ToolTrait`, `ChannelTrait`, `SecretStore`. |
| [`gauss-kernel`](./crates/gauss-kernel/) | Privileged admit gate, lock-free three-plane scheduler, consistent-hash cluster ring, declassification map. |
| [`gauss-turn`](./crates/gauss-turn/) | The Differential Turn Engine (Algorithm 1) — WAL barrier, optional sandbox, signed receipts, SAG approval. |
| [`gauss-memory`](./crates/gauss-memory/) | Trinity Memory — SurrealDB + BM25 + HNSW + K-LRU + Myers diff. |
| [`gauss-audit`](./crates/gauss-audit/) | SHA-256 chain + Ed25519 signed receipts + RFC 3161 / OpenTimestamps anchors + verifier API. |
| [`gauss-provider`](./crates/gauss-provider/) | Provider adapter contract; ships `ToyProvider` for tests. Vendor drivers live in `gaussclaw-providers`. |

### Worker and sandbox

| Crate | Role |
|---|---|
| [`gauss-sandbox`](./crates/gauss-sandbox/) | Composite sandbox — WASM (wasmi) + Landlock + seccomp + bwrap + Seatbelt; TEE-attest feature. |
| [`gauss-hwca`](./crates/gauss-hwca/) | Hardware-enforced compute attestation — worker contexts + four-stage schema gate. |

### Autonomy, audit, verification

| Crate | Role |
|---|---|
| [`gauss-sag`](./crates/gauss-sag/) | Supervised Autonomy Gradient — `DecisionTable` + monotonicity verifier + approval surfaces. |
| [`gauss-poly`](./crates/gauss-poly/) | Polyhedral trait-equivalence verifier (T7). |
| [`gauss-conformance`](./crates/gauss-conformance/) | Axiom-by-axiom property-test harness (A1–A9, T1–T12). |
| [`gauss-bench`](./crates/gauss-bench/) | 15-axis Pareto-dominance scorecard. |

### Surface and operations

| Crate | Role |
|---|---|
| [`gauss-canvas`](./crates/gauss-canvas/) | A2UI Live Canvas Protocol — typed widget tree + update stream (T8). |
| [`gauss-health`](./crates/gauss-health/) | Self-Diagnosable Health Engine — seven minimum invariants plus custom registration. |
| [`gauss-gateway`](./crates/gauss-gateway/) | REST · WebSocket · SSE wire types and OpenAI-compatible proxy schema. |
| [`gauss-attest`](./crates/gauss-attest/) | TEE attestation trait + Ed25519 software simulator (T10 §L4). |
| [`gauss-chaos`](./crates/gauss-chaos/) | Deterministic kill / partition / clock-skew injectors. |

### Research vehicles (additive, behind stable trait contracts)

| Crate | Role |
|---|---|
| [`gauss-zk`](./crates/gauss-zk/) | Zero-knowledge proofs over the receipt chain. |
| [`gauss-dp`](./crates/gauss-dp/) | Differentially-private trajectory exporter — Laplace + Gaussian mechanisms. |
| [`gauss-learnt`](./crates/gauss-learnt/) | Learnt risk classifier Φ̂ — logistic scorer that *floors* the SAG rule table. |
| [`gauss-robust`](./crates/gauss-robust/) | Robust declassifiers — adversarially-adaptive taint downgrades. |

---

## The eleven-tuple, mapped

The source paper writes the runtime as
`G = (S, A, O, K, M, F, π, L, Φ, R, V)`. Every component is one crate:

```text
S  state types       ← gauss-core         Φ  risk classifier  ← gauss-sag + gauss-learnt
A  actions           ← gauss-core         R  rendering        ← gauss-canvas + gauss-gateway
O  observations      ← gauss-core         V  verification     ← gauss-conformance + gauss-poly
K  capabilities      ← gauss-kernel       L  taint lattice    ← gauss-core + gauss-kernel::flow
M  memory            ← gauss-memory       F  audit chain      ← gauss-audit
π  policy            ← gauss-provider + gauss-poly
```

---

## Axioms and theorems

Every property below has a one-to-one conformance test. The full
299-test suite runs in about three seconds.

| ID | Statement | Crate | Test |
|---|---|---|---|
| **A1** | External effects fire only after the WAL append durably succeeds. | `gauss-turn` | `axiom_a1_wal_before_effect` |
| **A2 / T2** | Capabilities monotonically shrink under `contract`; CAS-protected. | `gauss-kernel::admit` | `axiom_a2_capability_monotonicity` |
| **A3 / T3** | Modifying any payload diverges the Merkle chain head. | `gauss-audit::chain` | `theorem_t3_merkle_tamper_evidence` |
| **A4 / T4** | Three-plane scheduler has a `B/ρ` starvation bound. | `gauss-kernel::sched` | `theorem_t4_starvation_bound` |
| **A5 / T5 / T12** | Memory monoid laws + hybrid recall + delta warm-switch. | `gauss-memory` | `axiom_a5_memory_monoid` etc. |
| **A6** | Information-flow lattice with antitone declassification. | `gauss-kernel::flow` | `axiom_a6_taint_lattice` |
| **A7 / T9** | Worker-context isolation + IPI containment ≤ 2.19 %. | `gauss-hwca` | `axiom_a7_and_theorem_t9_hwca` |
| **A8** | Supervised-autonomy gradient — monotone risk classifier. | `gauss-sag` | `axiom_a8_sag_approval` |
| **A9 / T11** | Ed25519 EUF-CMA signatures + receipt non-repudiation. | `gauss-audit::sign` | signing tests |
| **T6** | Stateless-turn scaling via consistent-hash routing. | `gauss-kernel::cluster` | `theorem_t6_stateless_scaling_and_attest` |
| **T7** | Provider adjunction — polyhedral equivalence on a probe set. | `gauss-poly` | `theorem_t7_provider_adjunction` |
| **T8** | 15-axis Pareto-dominance scorecard. | `gauss-bench` | `theorem_t8_pareto_dominance` |
| **T10** | Composite sandbox bound — `Pr[compromise] ≤ Π pᵢ + p_T`. | `gauss-sandbox` | `theorem_t10_composite_sandbox` |
| **T10 §L4** | TEE attestation simulator + verifier. | `gauss-attest` | `theorem_t6_stateless_scaling_and_attest` (L4 row) |

Lean 4 stubs of every axiom and theorem live in [`proofs/lean/`](./proofs/lean/).

---

## Embedding the runtime

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
    let provider = Arc::new(ToyProvider::always_text("hello from gauss-aether"));
    let sag      = Arc::new(ApprovalGate::new(
        default_decision_table(),
        AutoApprove::new("operator"),
    ));

    let engine = TurnEngine::new(kernel, memory.clone(), provider).with_sag(sag);

    let summary = engine.run_turn(TurnInput {
        id:  TurnId::new(1),
        obs: /* your observation */ todo!(),
    }).await?;

    println!("chain head = {}", hex::encode(summary.chain_head.digest));
    Ok(())
}
```

End-to-end walkthrough — SAG approval round-trip, signed receipts,
gateway wire types — in [`../docs/QUICKSTART.md`](../docs/QUICKSTART.md).

---

## Quality gates (every PR)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings    # pedantic + nursery
cargo test --workspace                                   # 299 tests, ~3 s
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check                                         # supply-chain + license
```

The release gate additionally asserts a 15-axis Pareto-dominance
result against the prior version
(`phase11_release::one_point_zero_pareto_dominates_every_predecessor`).

---

## Design tenets

1. **Properties before features.** Every subsystem cites the axiom or theorem it advances.
2. **No `unsafe` in privileged crates.** Workspace lint: `unsafe_code = "forbid"`.
3. **Lock-free where the CAS pattern is clean.** The scheduler packs `(tokens, epoch_ms)` into one `AtomicU64`.
4. **Type-state encodes phase order.** The Differential Turn Engine cannot run a tool before the WAL barrier.
5. **`#[non_exhaustive]` everywhere.** Field and variant additions stay semver-minor.
6. **Receipts cover the SAG verdict.** Approval decisions are part of the canonical signed payload.
7. **Recall is monoidal and hybrid.** BM25 ∪ HNSW with weighted union; composition is associative.

---

## Reference documents

| File | Purpose |
|---|---|
| [`SPECS.md`](./SPECS.md) | Normative specification — A1–A9, T1–T12, trait surface every runtime impl must satisfy. |
| [`Gauss-Aether.pdf`](./Gauss-Aether.pdf) | Architecture paper — definitions, theorems, proof sketches. |
| [`ROADMAP.md`](./ROADMAP.md) | Phased build plan and research-track register. |
| [`proofs/lean/`](./proofs/lean/) | Mechanised proof scaffolding (Lean 4 / Coq extraction targets). |
| [`../docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) | Crate-by-crate architecture tour. |
| [`../docs/SECURITY.md`](../docs/SECURITY.md) | Threat model + responsible-disclosure policy. |
| [`../docs/adr/`](../docs/adr/) | Sixteen architecture decision records. |
