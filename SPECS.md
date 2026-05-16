# Gauss-Aether — Rust Technical Specification

**Version:** 0.1.0-draft
**Status:** Pre-implementation (axiomatic specification)
**Source:** *Gauss-Aether: An Axiomatic Operating System for Trustworthy Autonomous Agents*, Gaussian Technologies.
**License target:** Apache-2.0 / MIT dual.

---

## 0. Document Scope

This document is the **engineering specification** that translates the Gauss-Aether paper into a buildable Rust system. It is normative for the implementation: every subsystem MUST be traceable back to an axiom A1–A9 or theorem T1–T12 in the source paper.

Reading order:

1. §1 Conceptual model (eleven-tuple `G`, axioms, theorems).
2. §2 Workspace and crate layout.
3. §3–§12 Subsystem-by-subsystem specifications (one section per Gauss-Aether subsystem).
4. §13 Cross-cutting concerns (observability, build, release).
5. §14 Compliance matrix (axiom → crate → module → test).
6. §15 Non-goals / out-of-scope.

Conformance keywords (MUST, SHOULD, MAY) follow RFC 2119.

---

## 1. Conceptual Foundation

### 1.1 The Eleven-Tuple

Gauss-Aether realises the formal AI Operating System

```text
G = (S, A, O, K, M, F, π, L, Φ, R, V)
```

where each component is implemented by exactly one Rust type or trait (§14):

| Symbol | Meaning                                  | Rust realisation                     |
|--------|------------------------------------------|--------------------------------------|
| `S`    | Turn state space                         | `gauss_kernel::TurnState`            |
| `A`    | Actions (`Atxt ⊔ Atool`)                 | `gauss_core::Action` enum            |
| `O`    | Observations                             | `gauss_core::Observation`            |
| `K`    | Capability lattice                       | `gauss_kernel::cap::CapLattice`      |
| `M`    | Memory monoid                            | `gauss_memory::MemoryMonoid`         |
| `F`    | Fairness allocation                      | `gauss_kernel::sched::FairAlloc`     |
| `π`    | Policy (LLM)                             | `gauss_provider::Policy`             |
| `L`    | Info-flow lattice (taint)                | `gauss_kernel::flow::TaintLattice`   |
| `Φ`    | Supervised-autonomy gradient             | `gauss_sag::AutonomyClassifier`      |
| `R`    | Receipt monoid                           | `gauss_audit::ReceiptMonoid`         |
| `V`    | TEE attestation predicate                | `gauss_attest::Attestation`          |

### 1.2 The Nine Axioms (Normative)

Each axiom is encoded as (a) a Rust type-level invariant where feasible, (b) a runtime predicate enforced by the kernel, and (c) a test suite in `crates/gauss-conformance/`.

| ID | Axiom                          | Primary enforcement site                                  |
|----|--------------------------------|-----------------------------------------------------------|
| A1 | Turn Idempotency               | `gauss_turn::engine::commit` (WAL-before-effect)          |
| A2 | Capability Monotonicity        | `gauss_kernel::cap::reserve`                              |
| A3 | Audit Completeness             | `gauss_audit::chain::append`                              |
| A4 | Fairness Separation            | `gauss_kernel::sched::Planes` (3 token buckets)           |
| A5 | Recall Soundness               | `gauss_memory::hybrid::ρ_hyb`                             |
| A6 | Information-Flow Non-Decreasing| `gauss_kernel::flow::join_label`                          |
| A7 | Context Isolation              | `gauss_hwca::Worker::spawn` + schema gate                 |
| A8 | Supervised Autonomy            | `gauss_sag::classify`                                     |
| A9 | Receipt Non-Repudiation        | `gauss_audit::sign::Ed25519Signer`                        |

### 1.3 The Twelve Theorems (Normative bounds)

T1 (crash atomicity), T2 (cap non-interference), T3 (Merkle tamper-evidence), T4 (plane starvation-freedom), T5 (hybrid recall miss product), T6 (Θ(N) stateless scale), T7 (provider adjunction), T8 (Pareto-dominance), T9 (IPI bound `|Σa|/|Σ| · 1[δ]`), T10 (composite-sandbox bound `Πpᵢ + p_T`), T11 (EUF-CMA receipts), T12 (`O(|Δ|)` warm switch).

Each theorem maps to one *performance/security target* in §14.3.

---

## 2. Workspace & Crate Layout

The system is a **Cargo workspace** with strict crate boundaries; the kernel/runtime split mirrors OpenFang but extended to ten subsystems.

```
gauss-aether/
├── Cargo.toml                # [workspace]
├── rust-toolchain.toml       # pinned channel: stable, MSRV 1.83
├── deny.toml                 # cargo-deny config (licences, advisories)
├── crates/
│   ├── gauss-core/           # shared types, error, IDs, traits-of-traits
│   ├── gauss-kernel/         # PRIVILEGED — capability + flow + sched
│   │   ├── src/cap/          # K lattice
│   │   ├── src/flow/         # L lattice + declass
│   │   ├── src/sched/        # 3 planes (Conv, Daemon, Approval)
│   │   └── src/attest/       # V predicate
│   ├── gauss-turn/           # Differential Turn Engine (DTE)
│   ├── gauss-hwca/           # Hierarchical Worker-Context Architecture
│   ├── gauss-sandbox/        # composite: WASM ∧ Landlock ∧ ns/seccomp ∧ TEE
│   │   ├── src/wasm/         # wasmtime, fuel+epoch
│   │   ├── src/landlock/     # Linux 5.13+
│   │   ├── src/bwrap/        # bubblewrap glue
│   │   ├── src/seatbelt/     # macOS
│   │   └── src/seccomp/      # libseccomp-rs
│   ├── gauss-memory/         # Trinity: append-log + FTS + HNSW + Merkle
│   │   ├── src/log/          # delta-encoded WAL
│   │   ├── src/fts/          # tantivy
│   │   ├── src/vec/          # hnsw_rs
│   │   ├── src/klru/         # K-LRU + prefix tree
│   │   └── src/snapshot/     # Myers diff over session ADT
│   ├── gauss-audit/          # Receipt chain, Ed25519, TSA anchor
│   ├── gauss-sag/            # Supervised Autonomy Gradient classifier
│   ├── gauss-provider/       # Provider trait + adapters (Anthropic, OpenAI, Google)
│   ├── gauss-channel/        # Channel trait + adapters (Telegram, Discord, Slack, …)
│   ├── gauss-tool/           # Tool trait + MCP bridge
│   ├── gauss-canvas/         # A2UI Live Canvas Protocol (server side)
│   ├── gauss-health/         # Self-Diagnostic Health Engine (SDHE)
│   ├── gauss-gateway/        # three-plane router, REST/WS/SSE, OAI-compat, ACP
│   ├── gauss-traits/         # public trait surface (re-exports for plugin authors)
│   ├── gauss-poly/           # polyhedral equivalence verifier (build-time SMT/Z3)
│   ├── gauss-cli/            # `gauss` binary, `gauss doctor`, `gauss import …`
│   ├── gauss-tui/            # ratatui TUI
│   ├── gauss-desktop/        # Tauri shell (thin)
│   ├── gauss-conformance/    # axiom test suite (A1–A9, T1–T12)
│   └── gauss-bench/          # criterion + scorecard runner
└── docs/
    ├── SPECS.md              # (this file)
    ├── ROADMAP.md
    ├── adr/                  # Architecture Decision Records
    └── proofs/               # Lean/Coq sketches (future)
```

**Privilege levels.** Crates are categorised:

| Tier | Crates                                                                 | Audit requirement              |
|------|------------------------------------------------------------------------|--------------------------------|
| 0    | `gauss-kernel`, `gauss-audit`, `gauss-attest`                          | dual review, no `unsafe` w/o ADR |
| 1    | `gauss-turn`, `gauss-hwca`, `gauss-sandbox`, `gauss-sag`, `gauss-memory`| single review + property tests |
| 2    | provider/channel/tool/canvas/gateway/health/cli/tui/desktop            | normal review                  |
| 3    | `gauss-conformance`, `gauss-bench`, `gauss-poly`                       | best-effort                    |

**Forbidden cross-crate flows.** `gauss-provider` and `gauss-channel` MUST NOT depend on `gauss-kernel` directly; they consume `gauss-core` types only. The kernel exposes its enforcement surface through `gauss-traits`.

---

## 3. `gauss-core` — Shared Types

Pure-data crate, `#![no_std]` friendly where possible (uses `alloc`). No I/O.

### 3.1 Identifier types

```rust
pub struct TurnId(pub u128);          // ULID; monotone within session
pub struct SessionId(pub u128);
pub struct AgentId(pub Uuid);
pub struct ToolId(pub SmolStr);
pub struct WorkerId(pub u64);
```

All IDs `Copy`, `Eq`, `Hash`, `serde::Serialize`. `TurnId` ordering follows ULID lexicographic.

### 3.2 Action / Observation enums

```rust
pub enum Action {
    Text(TextAction),
    Tool(ToolAction),
}

pub struct ToolAction {
    pub tool: ToolId,
    pub args: serde_json::Value,
    pub cap_required: CapToken,
    pub reversible: bool,        // manifest-declared
}
```

`Observation` carries the *taint label* explicitly:

```rust
pub struct Observation {
    pub source: ObservationSource,
    pub taint: TaintLabel,        // see §4.2
    pub body: serde_json::Value,
    pub received_at: SystemTime,
}
```

### 3.3 Error model

All public APIs return `Result<T, GaussError>` where `GaussError` is non-exhaustive:

```rust
#[non_exhaustive]
pub enum GaussError {
    CapDenied { needed: CapToken, granted: CapToken },
    TaintViolation { required: CapToken, declass: CapToken, taint: TaintLabel },
    AutonomyDenied { class: RiskClass },
    AutonomyApprovalTimeout,
    SandboxFailure(SandboxFailure),
    AuditChainBroken,
    ReceiptVerifyFailed,
    SchemaValidationFailed(String),
    /* … */
}
```

A `CapDenied`/`TaintViolation` MUST tag the **two-bit refusal reason** `(cap_bit, taint_bit)` for forensic completeness (paper §VI.C).

---

## 4. `gauss-kernel` — Capability + Flow + Scheduler

The privileged authority. Single-process by default; clustering layered via `gauss-gateway` consistent-hash routing (T6).

### 4.1 Capability Lattice `K` (A2, T2)

Bounded meet-semilattice with explicit `⊥`, `⊤`, `⊓`, `⊔`.

```rust
pub struct CapLattice { /* poset over CapNode */ }

pub trait Capability: Sealed {
    fn meet(&self, other: &Self) -> Self;
    fn join(&self, other: &Self) -> Result<Self, GrantRequired>;
    fn leq(&self, other: &Self) -> bool;     // ⪯
}
```

**Invariants** (enforced in `cap::reserve`):

- I-A2-1: `Kt+1 ⪯ Kt` on every admissible turn.
- I-A2-2: `⊔` (capability grant) requires an out-of-band signed admin operation; the runtime never elevates implicitly.
- I-T2-1: Two disjoint capability sets (`K1 ⊓ K2 = ⊥`) share no kernel-mediated channel.

**Default cap namespace**: `Filesystem.{read,write,scoped(p)}`, `Network.{get(d),post(d)}`, `Subprocess.{spawn(c)}`, `Crypto.{sign(key)}`, `Canvas.{render,embed(d),file_write}`.

### 4.2 Information-Flow Lattice `L` (A6)

Total chain by default, with extensibility to product lattices:

```rust
pub enum TaintLabel { Trusted, User, Web, Adversarial }  // ⊥ → ⊤
impl TaintLabel { fn join(self, rhs: Self) -> Self; }
```

The `declass: L → K` map is configurable per tenant and stored as a signed manifest. Build-time check: `declass` MUST be **antitone** (paper Axiom 6); see §11 (`gauss-poly`).

Joint admissibility check (paper §VI):

```rust
fn admit(k: CapToken, ell: TaintLabel, kt: CapToken) -> Result<(), GaussError> {
    let bound = declass(ell).meet(kt);
    if k.leq(&bound) { Ok(()) } else { Err(/* tagged (cap_bit, taint_bit) */) }
}
```

### 4.3 Three-Plane Scheduler (A4, T4)

Three independent **token buckets**:

| Plane            | Refill rate `ρ_X`     | Bucket `B_X,max` | Default workload                    |
|------------------|------------------------|------------------|--------------------------------------|
| `Conversation`   | configurable (per tenant) | 32                | sync user turns, target <2 s         |
| `Daemon`         | configurable           | 16               | scheduled autonomous Hands           |
| `Approval`       | configurable           | 64               | HITL approval round-trips            |

Worst-case wait `≤ B_X,max / ρ_X` (T4) MUST hold under arbitrary cross-plane demand. Implementation uses lock-free token-bucket per plane (single atomic refill timestamp + saturating subtractions); no cross-plane shared counter.

### 4.4 Attestation Predicate `V` (composite sandbox layer 4)

```rust
pub trait Attestation {
    fn verify(&self) -> Result<AttestationReport, AttestError>;
    fn platform(&self) -> Platform;   // SevSnp | Tdx | Cca | Software
}
```

If `V(s) = 0`, kernel rejects every `Ahigh` action (paper Definition 2(3)). Software fallback is permitted but flagged in receipts.

---

## 5. `gauss-turn` — Differential Turn Engine (T1, T6, T12)

The DTE implements Algorithm 1 of the paper. Single turn = state machine:

```text
Ingest → Generate → Commit
  │         │          │
  │         │          └─ WAL append (chain.append(ρ)) THEN external effect.
  │         └─ stream provider; for each Atool: HWCA spawn → schema gate → sign.
  └─ classify plane, join taint, reserve budget, load delta, hybrid recall.
```

### 5.1 Lifecycle API

```rust
pub struct TurnEngine<K: Kernel, M: Memory, P: Provider> { /* … */ }

impl TurnEngine<…> {
    pub async fn run_turn(&self, t: TurnInput) -> Result<TurnOutcome, GaussError>;
}
```

`TurnOutcome` carries `(record r, receipt ρ, post-state s')`.

### 5.2 Crash atomicity (T1)

The WAL barrier is implemented via:

- `MemoryMonoid::append_record(r)` returns only after `fsync` (configurable: `Strict` / `Group` / `Background`; default `Strict` for Tier-0 deployments).
- Side-effect functions (`commit_effects`) are **idempotent** and re-executable on recovery using `(r, ρ)` as the deterministic input.

### 5.3 Delta loading (T12)

`load_delta(session_id)` returns the warm K-LRU working set; checkpoint cadence default `K = 128` turns. Reconstruction cost `O(|s0|_warm + |W_σ|·|Δ̄|)`.

### 5.4 Stateless routing (T6)

Per-turn state is *fully derivable* from `(s, o, kernel-state, memory)`. Routing uses consistent hashing on `SessionId.0` (Jump-hash, 64-bit). No turn pins to a node.

---

## 6. `gauss-hwca` — Hierarchical Worker-Context Architecture (A7, T9)

Each tool invocation `a ∈ Atool` runs in a **fresh worker context** `sw`. Only the schema-validated return value `v ∈ Σa` crosses back.

### 6.1 Worker spawn

```rust
pub struct Worker { /* opaque */ }

impl Worker {
    pub async fn spawn(
        parent: &TurnState,
        action: &ToolAction,
        taint_in: TaintLabel,
    ) -> Result<Worker, GaussError>;

    pub async fn invoke(&self) -> Result<ValidatedValue, GaussError>;
    // sw is dropped at boundary; raw output destroyed.
}
```

### 6.2 Schema gate

Each tool publishes a `manifest.toml`:

```toml
[tool]
id = "fetch_url"
reversible = false
cap_required = "Network.get(*)"

[output_schema]
kind = "json_schema"
inline = """ { "type": "object", "properties": { "title": {"type":"string", "maxLength": 280}, "body": {"type":"string", "maxLength": 4096} }, "required": ["title"], "additionalProperties": false } """

[guards]
no_instruction_substrings = true   # statistical filter on free-text fields
```

Validation enforces (a) structural conformance (JSON Schema 2020-12 via `jsonschema-rs`), (b) bounded length per field, (c) optional instruction-substring filter, (d) parse into typed Rust ADT.

### 6.3 Recursion depth

Sub-workers are permitted; depth bound default `8` (paper §X.C). Exceeding the bound is logged and surfaces `GaussError::WorkerDepthExceeded`.

### 6.4 IPI bound (T9)

Conformance test asserts empirically that for the standard MCP-tool corpus `|Σa|/|Σ| ≤ 10⁻³`, matching the paper's 2.19% headline ceiling.

---

## 7. `gauss-sandbox` — Composite Sandbox (T10)

Four orthogonal layers; capability-bound depth (paper §IX.B).

| Layer | Mechanism                                | Crate / dep                              |
|-------|------------------------------------------|------------------------------------------|
| L1    | WASM with fuel + epoch interruption      | `wasmtime` (≥24)                         |
| L2    | Linux Landlock / macOS Seatbelt          | `landlock` crate, `sandbox-exec`         |
| L3    | Namespace + seccomp                      | `nix`, `libseccomp-rs`, bubblewrap exec  |
| L4    | TEE attestation (SEV-SNP / TDX / CCA)    | platform crate (`sev`, `tdx-guest`, …)   |

### 7.1 Capability → minimum sandbox class

```rust
pub fn min_sandbox(cap: &CapToken) -> SandboxClass {
    match cap.depth() {
        Depth::ReadOnly   => SandboxClass::L1,
        Depth::ScopedFs   => SandboxClass::L1 | SandboxClass::L2,
        Depth::Subproc    => SandboxClass::L1 | SandboxClass::L2 | SandboxClass::L3,
        Depth::CryptoSign => SandboxClass::L1 | SandboxClass::L2 | SandboxClass::L3 | SandboxClass::L4,
    }
}
```

### 7.2 Composition bound (T10 numerical target)

With `p_WASM ≤ 10⁻³`, `p_LL ≤ 10⁻²`, `p_NS ≤ 10⁻²`, `p_SC ≤ 10⁻²`, `p_T ≤ 10⁻⁷`:

```text
Pr[compromise]  ≤  Π pᵢ + p_T  ≤  10⁻⁹ + 10⁻⁷  ≈  1.1 × 10⁻⁷
```

§14.3 promotes this to a release gate.

### 7.3 Conditional orthogonality

The composition relies on independence given the TCB. To preserve this empirically:

- L1 (wasmtime) MUST be statically linked, signature-verified at build, no `unsafe` host-imports beyond the gauss-defined ABI.
- L2 and L3 MUST be configured by the kernel, not by tool manifests.
- Each layer's bypass-event MUST be loggable independently (so we can audit `p_i` post-hoc).

---

## 8. `gauss-memory` — Trinity Memory Substrate (T3, T5, T12)

Single append-only event log with three derived indices.

### 8.1 Append log

- Storage: SQLite (single-node) or PostgreSQL (clustered) via `sqlx`.
- Each row = `(turn_id, prev_chain_head, delta_blob, receipt_blob, ts)`.
- Delta blob computed via Myers diff over the session ADT, encoded with `bincode2` + zstd-19.
- Checkpoint every `K = 128` turns (configurable per tenant).

### 8.2 FTS index (keyword)

- `tantivy` 0.22+; per-turn incremental indexing.
- Schema: `body STRING (text)`, `taint U64 (indexed, fast)`, `turn_id U128`.

### 8.3 HNSW vector index

- `hnsw_rs` (M = 16, ef_construction = 200 by default).
- Embedding model is pluggable via `EmbeddingTrait` (default: provider-supplied).

### 8.4 Hybrid recall (T5)

`ρ_hyb(q) = ρ_fts(q) ∪ ρ_vec(q)`; conformance suite asserts miss-rate bound `ε_hyb ≤ ε_fts · ε_vec` on the benchmark corpus (default target `≤ 0.015`).

### 8.5 K-LRU prefix tree

Radix tree keyed by token-prefix; nodes carry deltas. K (default 256) most recently accessed deltas pinned in memory; cold deltas spill to mmap-ed disk.

### 8.6 Audit chain integration

Merkle chain head `c_i = H(c_{i-1} ‖ ρ_i)` materialised in `gauss-audit` (§9); `gauss-memory` only stores raw `ρ_i` and exposes `chain_head() -> Hash`.

---

## 9. `gauss-audit` — Cryptographic Receipt Chain (A3, A9, T3, T11)

### 9.1 Receipt structure

```rust
pub struct Receipt {
    pub record: RecordHash,        // BLAKE3 of canonical-JSON record
    pub pubkey: ed25519_dalek::VerifyingKey,
    pub sig:    ed25519_dalek::Signature,  // Sign(sk, H(record ‖ prev_head))
    pub ts:     u64,               // monotonic ns since UNIX_EPOCH
}
```

- Signature scheme: **Ed25519** (`ed25519-dalek` v2, batch verify enabled).
- Hash function: **BLAKE3** for record canonicalisation; **SHA-256** for chain links (interop with RFC 3161 / OpenTimestamps).
- Chain: `c0 = 0^256`, `c_i = SHA-256(c_{i-1} ‖ ρ_i_bytes)`.

### 9.2 TSA anchoring

- Default cadence: every `n_anchor = 1000` receipts.
- Backends: RFC 3161 (`rfc3161-client`) + OpenTimestamps.
- Anchor proof stored as `AnchorRecord { chain_index, tsa_token, ots_proof }`.

### 9.3 Public verifier API

| Endpoint                    | Returns                                                  |
|-----------------------------|----------------------------------------------------------|
| `GET  /audit/head`          | `{cn, n, tn}` (chain head, length, timestamp)            |
| `GET  /audit/proof/{i}`     | Merkle inclusion proof for `ρi` to current head          |
| `POST /audit/verify`        | `{ρi}` → `{ok, chain_index, anchor_ref?}`                |
| `GET  /audit/anchor/{k}`    | TSA receipt for `c_{k·n_anchor}`                         |

Verifier reduces to `VerifypkA(record, sig)` + `H(prev ‖ ρ) == next`.

### 9.4 Forgery bounds (T11)

- Unforgeability: `≤ AdvΣsig(λ)` under EUF-CMA.
- Chain tampering: `≤ n · 2^{-λ+1}`.
- Conformance suite includes a fuzz target attempting both.

---

## 10. `gauss-sag` — Supervised Autonomy Gradient (A8)

`Φ : A × K × L → {auto, approve, deny}`. Implemented as a **monotone decision table** per tenant (paper Table VII).

```rust
pub trait AutonomyClassifier: Send + Sync {
    fn classify(&self, a: &Action, k: &CapToken, ell: TaintLabel) -> RiskClass;
}

pub enum RiskClass { Auto, Approve, Deny }
```

**Monotonicity check (build-time).** `gauss-poly` (§12) enforces that for the *risk preorder* `⊑risk` (capability depth × taint × reversibility) the table is non-decreasing. Z3-discharged.

**Approval routing.** When `RiskClass::Approve`, the kernel enqueues an `ApprovalRequest` onto the Approval plane (§4.3). The channel adapter (Telegram inline-keyboard, Slack interactive, etc.) renders the prompt; the response is itself a *signed receipt* joined to the chain (A9).

**Default deadline.** 5 minutes; on timeout the request transitions to `Deny`.

---

## 11. `gauss-provider`, `gauss-channel`, `gauss-tool`, `gauss-canvas`, `gauss-health`

These crates carry the public **Trait Polyhedral Plugin Surface** (paper §VIII). The eight required traits:

| Trait              | Crate              | Implementor examples                              |
|--------------------|--------------------|---------------------------------------------------|
| `ProviderTrait`    | `gauss-provider`   | Anthropic Messages, OpenAI Chat, Google Gemini    |
| `ChannelTrait`     | `gauss-channel`    | Telegram, Discord, Slack, Matrix, IMAP, Signal    |
| `ToolTrait`        | `gauss-tool`       | local fn, MCP server bridge, gRPC tool            |
| `SandboxTrait`     | `gauss-sandbox`    | WASM/Landlock/Bubblewrap/Seatbelt composite       |
| `MemoryTrait`      | `gauss-memory`     | SQLite, Postgres, in-mem                          |
| `VoiceTrait`       | `gauss-voice`      | whisper-rs + piper-rs                             |
| `ApprovalTrait`    | `gauss-sag`        | channel-based, web canvas, CLI                    |
| `CanvasTrait`      | `gauss-canvas`     | A2UI server                                       |

### 11.1 Provider adjunction (T7)

Each `ProviderTrait` impl ships `(σ_i, τ_i)` such that `τ_i ∘ σ_i = id_{A⋆}` on the working subset; verified at build by `gauss-poly` via property tests over a corpus of internal messages.

### 11.2 `gauss-canvas` — A2UI Live Canvas Protocol

JSON-RPC over WebSocket/SSE. Notification name: `canvas.update`; event: `canvas.event`.

Core widget registry (paper Table IX): `text | table | chart | form | approval | map | diagram | embed | file | slider`.

Capability gating:

- `Canvas_Render` for any update.
- `Canvas_Embed(domain)` for `embed` widget; SSRF allow-list.
- `Canvas_File_Write` for `file` writes to user downloads.

### 11.3 `gauss-health` — Self-Diagnostic Health Engine

Seven minimum invariants (paper Table X):

```text
ι_sig     every loaded plugin's signature verifies
ι_chan    each configured channel has a recent successful round-trip
ι_anchor  Merkle head was anchored within T_anchor
ι_cap     every tool declares all caps it uses dynamically
ι_klru    K-LRU cache hit rate ≥ threshold
ι_tee     V(s) = 1 on every node serving A_high
ι_poly    no trait swap violates polyhedral equivalence
```

Three operating modes: **continuous** (OpenTelemetry), **on-demand** (`gauss doctor`), **reactive** (kernel self-repair per paper Table XI).

---

## 12. `gauss-poly` — Polyhedral Equivalence Verifier

Build-time tool (`cargo gauss-verify`). Two responsibilities:

1. Discharge `specT` (trait relational specification) via SMT (Z3 4.13+ over the `z3` crate or external `z3` binary).
2. Property-test provider adjunction `τ ∘ σ = id`.

Failure aborts the build. Configuration in `gauss.toml`:

```toml
[verify]
solver       = "z3"
timeout_ms   = 30000
provider_corpus = "fixtures/provider_messages.json"
```

---

## 13. Cross-cutting Concerns

### 13.1 Async runtime

Tokio (multi-thread, current-thread for tests). No mixing with `async-std`. All I/O via `tokio::io`; CPU-bound work uses `rayon` via `spawn_blocking`.

### 13.2 Configuration

Layered: TOML file → environment variables (`GAUSS_*`) → CLI flags. Parsed by `figment`. Sensitive material (private keys, API keys) loaded from `keyring` or env only — never from the TOML.

### 13.3 Observability

- Logs: `tracing` + `tracing-subscriber`; structured JSON in production.
- Metrics: OpenTelemetry OTLP exporter (`opentelemetry-otlp`), Prometheus scrape endpoint.
- Tracing: spans per turn, per worker, per tool call; correlation IDs propagated through the kernel.
- Audit events: emitted to the receipt chain *and* to OTLP for live monitoring.

### 13.4 Cryptography

| Use                  | Algorithm        | Crate                |
|----------------------|------------------|----------------------|
| Receipt signing      | Ed25519          | `ed25519-dalek` v2   |
| Record hashing       | BLAKE3           | `blake3`             |
| Chain link hashing   | SHA-256          | `sha2`               |
| TLS                  | rustls           | `rustls` + `tokio-rustls` |
| TSA timestamp        | RFC 3161         | `rfc3161-client`     |
| Public timestamp     | OpenTimestamps   | `opentimestamps`     |

No OpenSSL dependency. FIPS profile is a future extension.

### 13.5 Build & release

- MSRV: Rust 1.83 stable.
- Targets: `x86_64-unknown-linux-gnu` (Tier 1), `aarch64-unknown-linux-gnu` (Tier 1), `aarch64-apple-darwin` (Tier 2), `x86_64-pc-windows-msvc` (Tier 3).
- Static binary: musl build for `gauss-cli` (~30–40 MB target).
- Reproducible builds via `cargo --locked` + `cargo-vet` for dependency audit.
- SLSA Level 3 provenance (future).

### 13.6 Testing strategy

| Layer              | Mechanism                                                                |
|--------------------|--------------------------------------------------------------------------|
| Unit               | `#[cfg(test)]` per crate                                                 |
| Property           | `proptest` for lattice laws, K-LRU invariants, hash chain integrity      |
| Fuzz               | `cargo-fuzz` targets: JSON-schema validator, WASM imports, receipt parser |
| Integration        | `gauss-conformance` crate runs all axiom A1–A9 + theorem T1–T12 scenarios |
| Bench              | `criterion` in `gauss-bench`; runs the fifteen-axis scorecard            |
| Security regression| Adversarial prompt corpus from AgentDojo + EchoLeak-style scenarios      |

---

## 14. Compliance Matrix

### 14.1 Axiom → crate → module

| Axiom | Primary crate     | Module                          | Conformance test ID |
|-------|-------------------|---------------------------------|---------------------|
| A1    | `gauss-turn`      | `engine::commit`                | CONF-A1-*           |
| A2    | `gauss-kernel`    | `cap::reserve`                  | CONF-A2-*           |
| A3    | `gauss-audit`     | `chain::append`                 | CONF-A3-*           |
| A4    | `gauss-kernel`    | `sched::planes`                 | CONF-A4-*           |
| A5    | `gauss-memory`    | `hybrid::recall`                | CONF-A5-*           |
| A6    | `gauss-kernel`    | `flow::join_label`              | CONF-A6-*           |
| A7    | `gauss-hwca`      | `worker::spawn`                 | CONF-A7-*           |
| A8    | `gauss-sag`       | `classify`                      | CONF-A8-*           |
| A9    | `gauss-audit`     | `sign::ed25519`                 | CONF-A9-*           |

### 14.2 Theorem → enforcement

| Theorem | Subsystem                  | Enforcement mode                                 |
|---------|----------------------------|--------------------------------------------------|
| T1      | Differential Turn Engine   | WAL-before-effect; crash-injection test          |
| T2      | Capability Calculus        | type-level disjoint-cap proof + runtime check    |
| T3      | Receipt Chain              | hash-collision-bound fuzz                        |
| T4      | Three-Plane Gateway        | starvation freedom under saturation bench        |
| T5      | Trinity Memory             | hybrid-recall bench on labelled corpus           |
| T6      | Stateless-turn Routing     | scale-out bench across N nodes                   |
| T7      | Provider Adjunction        | property-test `τ∘σ = id`                         |
| T8      | Whole-system               | fifteen-axis scorecard regression                |
| T9      | HWCA                       | IPI corpus bound `≤ 2.19%`                       |
| T10     | Composite Sandbox          | per-layer bypass tests; product bound asserted   |
| T11     | Receipt Chain              | Ed25519 EUF-CMA test vectors + chain tampering   |
| T12     | Memory + DTE               | warm/cold context-switch bench                   |

### 14.3 Release gates (quantitative)

A release of `gauss-aether` MUST satisfy:

| Metric                            | Target                             | Source theorem |
|-----------------------------------|------------------------------------|----------------|
| IPI attack success                | `≤ 2.19%` on AgentDojo+EchoLeak     | T9             |
| Cold-start (warm cache)           | `≤ 10 ms` p95 single session        | T12            |
| Composite sandbox bound           | `≤ 1.1 × 10⁻⁷` (TEE) / `≤ 10⁻⁹` (sw)| T10            |
| Hybrid recall miss                | `≤ 0.015` on benchmark corpus       | T5             |
| Receipt forgery                   | negl(λ) under λ=128                 | T11            |
| Approval starvation               | bounded by `B/ρ_A`                  | T4             |
| Turn-record durability            | zero loss under SIGKILL injection   | T1             |

---

## 15. Non-Goals / Out-of-Scope

Aligned with paper §XVII.E:

- **Model alignment.** Gauss-Aether is structural, not behavioural; it does not constrain *what* the LLM reasons, only what *actions* it can execute.
- **Direct prompt injection.** A user issuing a malicious prompt to their own agent is upstream of the kernel.
- **Hardware supply chain.** TEE attestation surfaces but does not solve hardware trust.
- **GPU scheduling.** Provider inference happens at the provider; local-inference orchestration is a future extension via a new trait, not the core specification.
- **Multi-region replication.** Single-region clustering is in scope; geo-replication is a v2 effort.

---

## Appendix A — Reference Subsystem Diagram

```text
┌─────────────────────────────────────────────────────────────────────────┐
│ SURFACES   CLI · TUI · Tauri · REST/WS/SSE · OAI-compat · ACP · 40+ ch  │
├─────────────────────────────────────────────────────────────────────────┤
│ GATEWAY    three-plane router (Conversation | Daemon | Approval)        │
├─────────────────────────────────────────────────────────────────────────┤
│ KERNEL     K × L lattices  ·  Φ classifier  ·  fairness  ·  V attest    │
├─────────────────────────────────────────────────────────────────────────┤
│ TURN ENG.  prompt → stream → schema-gated tool → WAL → effect           │
├─────────────────────────────────────────────────────────────────────────┤
│ HWCA       isolated worker per tool call; schema-validated return only  │
├─────────────────────────────────────────────────────────────────────────┤
│ SANDBOX    WASM ∧ Landlock ∧ ns/seccomp ∧ TEE (capability-bound depth)  │
├─────────────────────────────────────────────────────────────────────────┤
│ TRAITS     provider · channel · tool · memory · sandbox · voice · …     │
├─────────────────────────────────────────────────────────────────────────┤
│ MEMORY     append-log + FTS + HNSW + Merkle/Ed25519 + K-LRU prefix tree │
├─────────────────────────────────────────────────────────────────────────┤
│ CANVAS     A2UI Live Canvas Protocol (capability-bound widgets)         │
├─────────────────────────────────────────────────────────────────────────┤
│ HEALTH     continuous invariant verifier + self-repair                  │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Appendix B — Glossary

- **A2UI** — Agent-to-User-Interface protocol.
- **CRC** — Cryptographic Receipt Chain.
- **DTE** — Differential Turn Engine.
- **HWCA** — Hierarchical Worker-Context Architecture.
- **IPI** — Indirect Prompt Injection.
- **SAG** — Supervised Autonomy Gradient.
- **SDHE** — Self-Diagnostic Health Engine.
- **TCB** — Trusted Compute Base.
- **TSA** — Time-Stamping Authority (RFC 3161).
- **WAL** — Write-Ahead Log.
