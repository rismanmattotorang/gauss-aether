# Gauss-Aether — Rust Technical Specification

**Version:** 0.2.0-draft
**Status:** Phase 2 complete (workspace + kernel + DTE + memory log + chain). Phases 3–11 pending.
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

### 0.1 Implementation status (after Phase 2)

| Phase | Status | Highlights |
|-------|--------|------------|
| 0     | ✅ Done | Workspace, 24 crates planned, 6 stubs, ADRs 0001–0005, CI, 35 tests. |
| 1     | ✅ Done | `gauss-traits` crate; lock-free 3-plane sched; joint K×L admission; `PrivilegedKernel` with CAS-protected grant; antitone declass; SurrealDB embedded backend; 51 tests. |
| 2     | ✅ Done | Real Differential Turn Engine (Algorithm 1, WAL-before-effect); SHA-256 chain replay + inclusion witness; Myers diff snapshot; `ToyProvider`; ADRs 0006–0008; 73 tests. |
| 3–11  | Planned | Composite sandbox, HWCA, signed receipts, hybrid recall, SAG, trait verifier, Canvas, SDHE, scale-out, 1.0 release. |

See [`ROADMAP.md`](./ROADMAP.md) for phase-by-phase deliverables.

---

## 1. Conceptual Foundation

### 1.1 The Eleven-Tuple

Gauss-Aether realises the formal AI Operating System

```text
G = (S, A, O, K, M, F, π, L, Φ, R, V)
```

where each component is implemented by exactly one Rust type or trait (§14):

| Symbol | Meaning                                  | Rust realisation                                      |
|--------|------------------------------------------|--------------------------------------------------------|
| `S`    | Turn state space                         | `gauss_turn::Turn<_>` + `gauss_memory::surreal` rows   |
| `A`    | Actions (`Atxt ⊔ Atool`)                 | `gauss_core::Action` enum                              |
| `O`    | Observations                             | `gauss_core::Observation`                              |
| `K`    | Capability lattice                       | `gauss_core::cap::CapToken` (re-exported by kernel; ADR-0008) |
| `M`    | Memory monoid                            | `gauss_traits::MemoryBackend` (impl: `gauss_memory::SurrealMemory`) |
| `F`    | Fairness allocation                      | `gauss_kernel::sched::Planes`                          |
| `π`    | Policy (LLM)                             | `gauss_traits::Provider` (Phase 2: `ToyProvider`)      |
| `L`    | Info-flow lattice (taint)                | `gauss_core::TaintLattice` + `gauss_kernel::DeclassMap`|
| `Φ`    | Supervised-autonomy gradient             | `gauss_sag::AutonomyClassifier` (Phase 7)              |
| `R`    | Receipt monoid                           | `gauss_audit::ReceiptChain` (Phase 5 adds Ed25519)     |
| `V`    | TEE attestation predicate                | `gauss_attest::Attestation` (Phase 10)                 |

### 1.2 The Nine Axioms (Normative)

Each axiom is encoded as (a) a Rust type-level invariant where feasible, (b) a runtime predicate enforced by the kernel, and (c) a test suite in `crates/gauss-conformance/`.

| ID | Axiom                          | Primary enforcement site (post Phase 2)                          |
|----|--------------------------------|------------------------------------------------------------------|
| A1 | Turn Idempotency               | `gauss_turn::engine::run_turn` (WAL append BEFORE effect)        |
| A2 | Capability Monotonicity        | `gauss_kernel::PrivilegedKernel::contract` (CAS, contract-only)  |
| A3 | Audit Completeness             | `gauss_audit::ReceiptChain::append` + `verify_replay`            |
| A4 | Fairness Separation            | `gauss_kernel::sched::Planes` (3 independent atomic token buckets)|
| A5 | Recall Soundness               | `gauss_memory::hybrid::ρ_hyb` (Phase 6)                          |
| A6 | Information-Flow Non-Decreasing| `gauss_kernel::admit` + `gauss_kernel::DeclassMap` (antitone)    |
| A7 | Context Isolation              | `gauss_hwca::Worker::spawn` + schema gate (Phase 4)              |
| A8 | Supervised Autonomy            | `gauss_sag::classify` (Phase 7)                                  |
| A9 | Receipt Non-Repudiation        | `gauss_audit::sign::Ed25519Signer` (Phase 5)                     |

### 1.3 The Twelve Theorems (Normative bounds)

T1 (crash atomicity), T2 (cap non-interference), T3 (Merkle tamper-evidence), T4 (plane starvation-freedom), T5 (hybrid recall miss product), T6 (Θ(N) stateless scale), T7 (provider adjunction), T8 (Pareto-dominance), T9 (IPI bound `|Σa|/|Σ| · 1[δ]`), T10 (composite-sandbox bound `Πpᵢ + p_T`), T11 (EUF-CMA receipts), T12 (`O(|Δ|)` warm switch).

Each theorem maps to one *performance/security target* in §14.3.

---

## 2. Workspace & Crate Layout

The system is a **Cargo workspace** with strict crate boundaries; the kernel/runtime split mirrors OpenFang but extended to ten subsystems. **`gauss-traits`** owns the public trait surface so plugin authors depend on a stable abstract API rather than the implementation crates.

```
gauss-aether/
├── Cargo.toml                # [workspace]
├── rust-toolchain.toml       # pinned channel: stable, MSRV 1.83
├── deny.toml                 # cargo-deny config (licences, advisories)
├── crates/
│   ├── gauss-core/           # shared types, error, IDs, CapToken lattice
│   ├── gauss-traits/         # PUBLIC trait surface — Kernel, MemoryBackend, Provider, …
│   ├── gauss-kernel/         # PRIVILEGED — joint K×L admit, lock-free 3-plane sched
│   ├── gauss-turn/           # Differential Turn Engine (DTE)
│   ├── gauss-hwca/           # Hierarchical Worker-Context Architecture (Phase 4)
│   ├── gauss-sandbox/        # composite: WASM ∧ Landlock ∧ ns/seccomp ∧ TEE (Phase 3+10)
│   ├── gauss-memory/         # Trinity: SurrealDB-backed log + FTS + HNSW + graph
│   │   ├── src/schema.rs     # SurrealQL bootstrap DDL
│   │   ├── src/surreal.rs    # SurrealMemory: MemoryBackend impl
│   │   └── src/snapshot.rs   # Myers line diff (Phase 6 ADT diff)
│   ├── gauss-audit/          # SHA-256 chain (Phase 5: Ed25519 + TSA)
│   ├── gauss-sag/            # Supervised Autonomy Gradient classifier (Phase 7)
│   ├── gauss-provider/       # Provider trait impls — ToyProvider now, vendors in Phase 8
│   ├── gauss-channel/        # Channel adapters (Phase 7+)
│   ├── gauss-tool/           # Tool trait + MCP bridge (Phase 4)
│   ├── gauss-canvas/         # A2UI Live Canvas Protocol (Phase 9)
│   ├── gauss-health/         # Self-Diagnostic Health Engine (Phase 9)
│   ├── gauss-gateway/        # three-plane router, REST/WS/SSE, OAI-compat (Phase 9)
│   ├── gauss-poly/           # polyhedral equivalence verifier (Phase 8)
│   ├── gauss-cli/            # `gauss` binary, `gauss doctor`, `gauss import …`
│   ├── gauss-tui/            # ratatui TUI
│   ├── gauss-desktop/        # Tauri shell (thin)
│   ├── gauss-conformance/    # axiom test suite (A1–A9, T1–T12)
│   └── gauss-bench/          # criterion + scorecard runner
└── docs/
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

**Forbidden cross-crate flows.** Implementation crates (`gauss-provider`, `gauss-channel`, …) MUST depend on `gauss-core` + `gauss-traits` only — never on `gauss-kernel` directly. The kernel consumes implementations *through* `gauss-traits`.

---

## 3. `gauss-core` — Shared Types

Pure-data crate. No I/O.

### 3.1 Identifier types

```rust
pub struct TurnId(pub u128);          // ULID (Phase 6); opaque now
pub struct SessionId(pub u128);
pub struct AgentId(pub String);       // UUID once key-mgmt lands
pub struct ToolId(pub String);
pub struct WorkerId(pub u64);
```

All numeric IDs `Copy`, `Eq`, `Hash`, `serde::Serialize`.

### 3.2 Capability lattice `K` (ADR-0008)

```rust
pub struct CapToken(u64);            // bitmask over a fixed namespace

impl CapToken {
    pub const BOTTOM: Self;            // ⊥
    pub const TOP: Self;               // ⊤
    pub const FILESYSTEM_READ: Self;   // …
    pub const NETWORK_GET: Self;
    pub const NETWORK_POST: Self;
    pub const SUBPROCESS_SPAWN: Self;
    pub const CRYPTO_SIGN: Self;
    pub const CANVAS_RENDER: Self;
    pub const CANVAS_EMBED: Self;
    pub const CANVAS_FILE_WRITE: Self;

    pub const fn meet(self, rhs: Self) -> Self;   // ⊓
    pub const fn join(self, rhs: Self) -> Self;   // ⊔
    pub const fn leq(self, rhs: Self) -> bool;    // ⪯
}
```

`CapToken` lives in `gauss-core` so `Action::Tool::cap_required` can carry it without inducing a cycle on `gauss-kernel` (ADR-0008).

### 3.3 Action / Observation enums

```rust
#[non_exhaustive]
pub enum Action {
    Text(TextAction),
    Tool(ToolAction),
}

#[non_exhaustive]
pub struct ToolAction {
    pub tool: ToolId,
    pub args: serde_json::Value,
    pub cap_required: CapToken,      // joint-admit input (paper §VI)
    pub reversible: bool,            // manifest-declared
}

#[non_exhaustive]
pub struct Observation {
    pub source: ObservationSource,
    pub taint: TaintLabel,
    pub body: serde_json::Value,
}
```

### 3.4 Error model

```rust
#[non_exhaustive]
pub enum GaussError {
    Denied { reason: RefusalReason },     // (cap_bit, taint_bit)
    AutonomyDenied,
    AutonomyApprovalTimeout,
    SchemaValidation(String),
    AuditChainBroken,
    ReceiptVerify,
    WorkerDepthExceeded { limit: u32 },
    Io(String),
    Internal(String),
}
```

`Denied` carries the two-bit refusal reason (paper §VI.C) so the operator can distinguish capability denials from upstream-taint denials.

---

## 4. `gauss-kernel` — Capability + Flow + Scheduler

The privileged authority. Single-process by default; clustering layered via `gauss-gateway` consistent-hash routing (T6).

### 4.1 Capability Lattice `K` (A2, T2)

Defined in `gauss-core` (ADR-0008). The kernel re-exports it (`gauss_kernel::CapToken`) and ships the lattice-law proptest suite. **Invariants** (enforced by `PrivilegedKernel::contract`):

- I-A2-1: `K_{t+1} ⪯ K_t` on every admissible turn. `contract()` uses a CAS loop to ensure no implicit escalation.
- I-A2-2: Growing the grant requires an out-of-band signed admin operation (Phase 5).
- I-T2-1: Two disjoint capability sets (`K_1 ⊓ K_2 = ⊥`) share no kernel-mediated channel.

### 4.2 Information-Flow Lattice `L` (A6)

```rust
pub enum TaintLabel { Trusted, User, Web, Adversarial }   // ⊥ → ⊤

pub trait DeclassMap: Send + Sync {
    fn declass(&self, taint: TaintLabel) -> CapToken;
}
```

Stock policies: `DefaultDeclass` (paper §VI.B-style permissive) and `StrictDeclass` (anything above User → BOTTOM). Custom maps are runtime-validated by `verify_antitone`, which inspects all `ℓ_1 ≤ ℓ_2` pairs and reports the first antitone violation.

### 4.3 Joint admission (paper §VI)

```rust
impl Kernel for PrivilegedKernel {
    fn admit(&self, required: CapToken, taint: TaintLabel) -> GaussResult<()> {
        let current      = self.current_grant();          // K_t
        let declass_bound = self.declass.declass(taint);  // declass(ℓ)
        let cap_ok   = required.leq(current);
        let taint_ok = required.leq(declass_bound);
        if cap_ok && taint_ok { return Ok(()); }
        Err(GaussError::Denied {
            reason: RefusalReason { cap_bit: !cap_ok, taint_bit: !taint_ok },
        })
    }
}
```

### 4.4 Three-Plane Scheduler (A4, T4)

Three independent **lock-free** token buckets. Each bucket packs `(tokens_fp16.16, epoch_ms)` into one `AtomicU64`; updates are CAS loops with no shared cross-plane state (so starvation freedom holds by construction).

| Plane            | Default capacity `B` | Default refill `ρ` | Workload                              |
|------------------|----------------------|--------------------|----------------------------------------|
| `Conversation`   | 32 tokens            | 8 tok/s            | sync user turns, target <2 s          |
| `Daemon`         | 16 tokens            | 2 tok/s            | scheduled autonomous Hands             |
| `Approval`       | 64 tokens            | 4 tok/s            | HITL approval round-trips              |

Worst-case wait `≤ B / ρ` (T4) MUST hold under arbitrary cross-plane demand.

### 4.5 Attestation Predicate `V` (Phase 10)

```rust
pub trait Attestation {
    fn verify(&self) -> Result<AttestationReport, AttestError>;
    fn platform(&self) -> Platform;   // SevSnp | Tdx | Cca | Software
}
```

If `V(s) = 0`, kernel rejects every `A_high` action (paper Definition 2(3)).

---

## 5. `gauss-turn` — Differential Turn Engine (T1, T6, T12)

Phase 2 ships [`TurnEngine<K, M, P>`] implementing Algorithm 1 minus HWCA (Phase 4) and signed receipts (Phase 5):

```text
Ingest → Generate → Admit → WAL → (effect)
  │         │         │      │         │
  │         │         │      │         └─ apply_actions_locally (Phase 3: sandbox executor)
  │         │         │      └─ memory.append(record) — A1 barrier
  │         │         └─ kernel.admit(cap, taint) for each tool action
  │         └─ provider.generate(obs) → Vec<Action>
  └─ join taint of observation sources
```

### 5.1 Lifecycle API

```rust
pub struct TurnEngine<K: Kernel, M: MemoryBackend, P: Provider> { /* … */ }

impl<K, M, P> TurnEngine<K, M, P>
where K: Kernel, M: MemoryBackend, P: Provider {
    pub fn new(kernel: Arc<K>, memory: Arc<M>, provider: Arc<P>) -> Self;
    pub async fn run_turn(&self, input: TurnInput) -> GaussResult<TurnSummary>;
}
```

`TurnSummary` carries `(id, action_count, chain_head)`. Phase 5 will attach the signed receipt; Phase 4 will attach worker-context isolation provenance.

### 5.2 Crash atomicity (T1) — see ADR-0007

The WAL barrier is **structural**: `apply_actions_locally(...)` is unreachable until `memory.append(...).await` resolves with `Ok(_)`. No configuration can disable this.

### 5.3 Stateless routing (T6) — Phase 10

Per-turn state is fully derivable from `(s, o, kernel-state, memory)`. Phase 10 wires consistent hashing on `SessionId.0`.

---

## 6. `gauss-hwca` — Hierarchical Worker-Context Architecture (A7, T9)

(Phase 4 deliverable; unchanged from earlier drafts.)

---

## 7. `gauss-sandbox` — Composite Sandbox (T10)

(Phase 3 / Phase 10 deliverable; unchanged from earlier drafts. Numerical target ≤ 1.1 × 10⁻⁷ with TEE.)

---

## 8. `gauss-memory` — Trinity Memory Substrate (T3, T5, T12)

**Single SurrealDB instance** stores the append-only event log AND every derived index, replacing the Phase-0 SQLite/tantivy/hnsw_rs stack. See ADR-0006 for the rationale.

### 8.1 Append log

- Storage: **SurrealDB**.
  - `kv-mem` for unit tests (Phase 1+).
  - `kv-surrealkv` (single-node persistent) — Phase 6.
  - `kv-rocksdb` (durable, fsync-on-commit) — Phase 6.
  - `kv-tikv` (clustered Raft) — Phase 10.
- Each `turn_record` row: `(turn_id, payload bytes, payload_text option<string>, embedding option<array<float>>, taint string, seq int, prev_head bytes, this_head bytes, recorded_at datetime)`.
- Per-row constraints: UNIQUE on `turn_id` and `seq`; `taint INSIDE ["trusted","user","web","adversarial"]`.
- Delta blob will be computed via Myers diff over the session ADT in Phase 6; Phase 2 ships the line-level Myers diff in `gauss_memory::snapshot`.

### 8.2 FTS index (keyword)

```sql
DEFINE ANALYZER lower_alphanum TOKENIZERS class FILTERS lowercase, ascii;
DEFINE INDEX turn_record_fts ON turn_record FIELDS payload_text
    SEARCH ANALYZER lower_alphanum BM25;
```

Defined up-front at Phase 1; populated by Phase 6.

### 8.3 HNSW vector index

```sql
DEFINE INDEX turn_record_hnsw ON turn_record FIELDS embedding
    HNSW DIMENSION 384 TYPE F32 DISTANCE COSINE M 16 EFC 200;
```

Dimension 384 (MiniLM default) is overridable per tenant.

### 8.4 Graph lineage (paper §VII)

```sql
DEFINE TABLE derived_from TYPE RELATION FROM turn_record TO turn_record;
```

Each turn that consumes another turn's record emits `RELATE turn:A -> derived_from -> turn:B SET reason = ...`. Enables `SELECT ->derived_from->turn_record FROM turn:X` graph traversal.

### 8.5 Capability grants

```sql
DEFINE TABLE agent SCHEMAFULL;
DEFINE TABLE capability_grant TYPE RELATION FROM agent TO capability_grant;
```

Phase 5 attaches signed-by anchors.

### 8.6 Hybrid recall (T5)

`ρ_hyb(q) = ρ_fts(q) ∪ ρ_vec(q)` — Phase 6 deliverable.

### 8.7 K-LRU prefix tree

Phase 6 deliverable.

### 8.8 Audit chain integration

Merkle chain head `c_i = H(c_{i-1} ‖ ρ_i)` materialised in `gauss-audit` (§9). `gauss-memory` caches the current head in-process (synchronised under the same transaction that writes the row) and exposes it through `MemoryBackend::chain_head()`.

---

## 9. `gauss-audit` — Cryptographic Receipt Chain (A3, A9, T3, T11)

### 9.1 Phase 2 surface

```rust
pub struct ChainHead([u8; 32]);

pub fn link(prev: ChainHead, payload: &[u8]) -> ChainHead;

pub struct ReceiptChain { /* head + len */ }
impl ReceiptChain {
    pub fn append(&mut self, payload: &[u8]) -> ChainHead;
    pub fn verify_replay(payloads: &[&[u8]], expected_head: ChainHead) -> Result<(), VerifyError>;
}

pub struct InclusionWitness { pub prev: ChainHead, pub post: ChainHead }
impl InclusionWitness {
    pub fn verify(&self, payload: &[u8]) -> bool;
}
```

### 9.2 Phase 5 receipts

```rust
pub struct Receipt {
    pub record: RecordHash,                 // BLAKE3
    pub pubkey: ed25519_dalek::VerifyingKey,
    pub sig:    ed25519_dalek::Signature,   // Sign(sk, H(record ‖ prev_head))
    pub ts:     u64,
}
```

- Ed25519 (`ed25519-dalek` v2), BLAKE3 record digest, SHA-256 chain link (RFC 3161 interop).
- Anchor cadence default 1000 receipts to RFC 3161 + OpenTimestamps.

### 9.3 Public verifier API (Phase 5)

| Endpoint                    | Returns                                                  |
|-----------------------------|----------------------------------------------------------|
| `GET  /audit/head`          | `{cn, n, tn}`                                            |
| `GET  /audit/proof/{i}`     | Merkle inclusion proof for `ρi` to current head          |
| `POST /audit/verify`        | `{ρi}` → `{ok, chain_index, anchor_ref?}`                |
| `GET  /audit/anchor/{k}`    | TSA receipt for `c_{k·n_anchor}`                         |

### 9.4 Forgery bounds (T11) — Phase 5

- Unforgeability: `≤ Adv_Σsig(λ)` under EUF-CMA.
- Chain tampering: `≤ n · 2^{-λ+1}`.

---

## 10. `gauss-sag` — Supervised Autonomy Gradient (A8)

(Phase 7 deliverable; unchanged from earlier drafts.)

---

## 11. Trait Polyhedral Plugin Surface (paper §VIII)

The eight required traits — placement after Phase 2:

| Trait              | Crate              | Status      | Implementor examples                              |
|--------------------|--------------------|-------------|---------------------------------------------------|
| `Kernel`           | `gauss-traits`     | ✅ Phase 1  | `gauss_kernel::PrivilegedKernel`                  |
| `MemoryBackend`    | `gauss-traits`     | ✅ Phase 1  | `gauss_memory::SurrealMemory`                     |
| `Provider`         | `gauss-traits`     | ✅ Phase 2  | `gauss_provider::ToyProvider`; Anthropic / `OpenAI` / Google in Phase 8 |
| `ChannelTrait`     | `gauss-channel`    | Phase 7     | Telegram, Discord, Slack, Matrix, IMAP, Signal    |
| `ToolTrait`        | `gauss-tool`       | Phase 4     | local fn, MCP server bridge, gRPC tool            |
| `SandboxTrait`     | `gauss-sandbox`    | Phase 3     | WASM/Landlock/Bubblewrap/Seatbelt composite       |
| `VoiceTrait`       | `gauss-voice`      | Phase 9+    | whisper-rs + piper-rs                             |
| `ApprovalTrait`    | `gauss-sag`        | Phase 7     | channel-based, web canvas, CLI                    |
| `CanvasTrait`      | `gauss-canvas`     | Phase 9     | A2UI server                                       |

### 11.1 Provider adjunction (T7) — Phase 8

Each `Provider` impl ships `(σ_i, τ_i)` such that `τ_i ∘ σ_i = id_{A⋆}` on the working subset; verified at build by `gauss-poly`.

### 11.2 `gauss-canvas` — A2UI Live Canvas Protocol — Phase 9

(Unchanged from earlier drafts.)

### 11.3 `gauss-health` — Self-Diagnostic Health Engine — Phase 9

(Unchanged. Seven minimum invariants from paper Table X.)

---

## 12. `gauss-poly` — Polyhedral Equivalence Verifier — Phase 8

(Unchanged.)

---

## 13. Cross-cutting Concerns

### 13.1 Async runtime

Tokio (ADR-0002).

### 13.2 Configuration

Layered TOML + env + CLI via `figment` (ADR-0004). Secrets via OS keyring / env; never in TOML.

### 13.3 Observability

`tracing` + OpenTelemetry (Phase 9 onward).

### 13.4 Cryptography (ADR-0003)

| Use                  | Algorithm        | Crate                |
|----------------------|------------------|----------------------|
| Receipt signing      | Ed25519          | `ed25519-dalek` v2   |
| Record hashing       | BLAKE3           | `blake3`             |
| Chain link hashing   | SHA-256          | `sha2`               |
| TLS                  | rustls           | `rustls` + `tokio-rustls` |
| TSA timestamp        | RFC 3161         | `rfc3161-client`     |
| Public timestamp     | OpenTimestamps   | `opentimestamps`     |

### 13.5 Build & release

- MSRV: 1.83.
- Tier-1 targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`.
- musl static binary for `gauss-cli`.
- Reproducible via `--locked` + `cargo-vet`.

### 13.6 Testing strategy

| Layer              | Mechanism                                                                |
|--------------------|--------------------------------------------------------------------------|
| Unit               | `#[cfg(test)]` per crate                                                 |
| Property           | `proptest` — lattice laws, K-LRU invariants, chain integrity             |
| Fuzz               | `cargo-fuzz` — JSON-schema validator, WASM imports, receipt parser       |
| Integration        | `gauss-conformance` — axiom A1–A9 + theorem T1–T12 scenarios             |
| Bench              | `criterion` in `gauss-bench`; fifteen-axis scorecard regression          |
| Crash injection    | Engine-drop harness (Phase 2 kv-mem); cross-process replay (Phase 6 rocks)|
| Security regression| AgentDojo + EchoLeak-style corpus (Phase 4)                              |

---

## 14. Compliance Matrix

### 14.1 Axiom → crate → module

| Axiom | Primary crate     | Module                                  | Conformance test ID | Status |
|-------|-------------------|-----------------------------------------|---------------------|--------|
| A1    | `gauss-turn`      | `engine::run_turn` (WAL barrier)        | CONF-A1-*           | ✅ Phase 2 |
| A2    | `gauss-kernel`    | `admit::PrivilegedKernel::{admit,contract}` | CONF-A2-*       | ✅ Phase 1 |
| A3    | `gauss-audit`     | `ReceiptChain::{append, verify_replay}` | CONF-A3-*           | ✅ Phase 2 |
| A4    | `gauss-kernel`    | `sched::Planes`                         | CONF-A4-*           | ✅ Phase 1 |
| A5    | `gauss-memory`    | `hybrid::recall`                        | CONF-A5-*           | Phase 6 |
| A6    | `gauss-kernel`    | `admit` + `flow::DeclassMap`            | CONF-A6-*           | ✅ Phase 1 |
| A7    | `gauss-hwca`      | `worker::spawn`                         | CONF-A7-*           | Phase 4 |
| A8    | `gauss-sag`       | `classify`                              | CONF-A8-*           | Phase 7 |
| A9    | `gauss-audit`     | `sign::ed25519`                         | CONF-A9-*           | Phase 5 |

### 14.2 Theorem → enforcement

| Theorem | Subsystem                  | Enforcement mode                                 | Status |
|---------|----------------------------|--------------------------------------------------|--------|
| T1      | Differential Turn Engine   | WAL-before-effect; crash-injection test          | ✅ Phase 2 |
| T2      | Capability Calculus        | type-level disjoint-cap proof + runtime check    | ✅ Phase 1 |
| T3      | Receipt Chain              | proptest: any payload mutation diverges the head | ✅ Phase 0/2 |
| T4      | Three-Plane Scheduler      | starvation freedom under saturation              | ✅ Phase 1 |
| T5      | Trinity Memory             | hybrid-recall bench on labelled corpus           | Phase 6 |
| T6      | Stateless-turn Routing     | scale-out bench across N nodes                   | Phase 10 |
| T7      | Provider Adjunction        | property-test `τ∘σ = id`                         | Phase 8 |
| T8      | Whole-system               | fifteen-axis scorecard regression                | Phase 9 |
| T9      | HWCA                       | IPI corpus bound `≤ 2.19%`                       | Phase 4 |
| T10     | Composite Sandbox          | per-layer bypass tests; product bound asserted   | Phase 3+10 |
| T11     | Receipt Chain              | Ed25519 EUF-CMA test vectors + chain tampering   | Phase 5 |
| T12     | Memory + DTE               | warm/cold context-switch bench                   | Phase 6 |

### 14.3 Release gates (quantitative)

A 1.0 release of `gauss-aether` MUST satisfy:

| Metric                            | Target                             | Source theorem |
|-----------------------------------|------------------------------------|----------------|
| IPI attack success                | `≤ 2.19%` on AgentDojo+EchoLeak    | T9             |
| Cold-start (warm cache)           | `≤ 10 ms` p95 single session       | T12            |
| Composite sandbox bound           | `≤ 1.1 × 10⁻⁷` (TEE) / `≤ 10⁻⁹` (sw)| T10           |
| Hybrid recall miss                | `≤ 0.015` on benchmark corpus      | T5             |
| Receipt forgery                   | negl(λ) under λ=128                | T11            |
| Approval starvation               | bounded by `B/ρ_A`                 | T4             |
| Turn-record durability            | zero loss under SIGKILL injection  | T1             |

---

## 15. Non-Goals / Out-of-Scope

Aligned with paper §XVII.E. Unchanged.

---

## Appendix A — Architecture Decision Records

| ADR | Topic                                            | Phase | Status     |
|-----|--------------------------------------------------|-------|------------|
| 0001| Axiom-driven phasing                             | 0     | Accepted   |
| 0002| Tokio multi-thread runtime                       | 0     | Accepted   |
| 0003| Ed25519 + BLAKE3 + SHA-256                       | 0     | Accepted   |
| 0004| Configuration via figment                        | 0     | Accepted   |
| 0005| Privilege tiers + review policy                  | 0     | Accepted   |
| 0006| SurrealDB as the Trinity Memory storage engine   | 1     | Accepted   |
| 0007| WAL barrier semantics for the DTE                | 2     | Accepted   |
| 0008| Canonical `CapToken` lives in `gauss-core`       | 2     | Accepted   |

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
