# Gauss-Aether

An axiomatic operating system for trustworthy autonomous LLM agents,
implemented in Rust.

> Status: **Phase 6 complete** — workspace, lock-free three-plane kernel
> with joint capability/taint admission, Differential Turn Engine with
> WAL-before-effect barrier, composite sandbox (WASM via wasmi ∧ Linux
> Landlock ∧ seccomp ∧ bubblewrap ∧ macOS Seatbelt) with capability-bound
> depth (T10), HWCA worker contexts + four-stage schema gate (0/20 IPI
> escape, T9), Ed25519 signed receipt chain + TSA-anchor abstractions
> (A9, T11), and **Trinity Memory hybrid recall** — BM25 keyword search,
> HNSW vector kNN, and a `hybrid_recall(α)` weighted-union surface
> backed by SurrealDB's `@@` and `<|k|>` operators; plus a **K-LRU
> prefix-tree cache** (K = 128 default, paper §VIII.C) and a proper
> Myers `O((N+M)·D)` diff for ADT-aware deltas. Optional
> `kv-surrealkv` / `kv-rocksdb` Cargo features for persistent storage.
> **170 tests pass** across 10 crates; Phases 7–11 add SAG, trait
> verifier, A2UI Canvas, SDHE, and 1.0 release (see
> [`ROADMAP.md`](./ROADMAP.md)).

## Documents

- [`SPECS.md`](./SPECS.md) — normative engineering specification.
- [`ROADMAP.md`](./ROADMAP.md) — phased development plan, axiom/theorem locks.
- [`docs/adr/`](./docs/adr/) — Architecture Decision Records.

## Quick start

```bash
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Workspace layout (after Phase 6)

| Crate                | Purpose                                                                              |
|----------------------|--------------------------------------------------------------------------------------|
| `gauss-core`         | Shared types: identifiers, actions, observations, taint, `CapToken` lattice, errors. |
| `gauss-traits`       | Public trait surface — `Kernel`, `MemoryBackend`, `Provider`, `SandboxTrait`, `ToolTrait`, `OutputSchema`, `SchemaGuards`, `ValidatedValue`. |
| `gauss-kernel`       | Privileged kernel: joint K×L admission, lock-free 3-plane token bucket, declass map. |
| `gauss-turn`         | Differential Turn Engine — Algorithm 1 with optional sandbox executor + signed receipts. |
| `gauss-memory`       | Trinity Memory: SurrealDB-backed append log + BM25 + HNSW hybrid recall + K-LRU prefix tree + Myers diff. |
| `gauss-audit`        | SHA-256 chain + Ed25519 [`SignedReceipt`] + RFC 3161 / `OpenTimestamps` anchor traits + offline simulator (`SimulatorTsaClient`) + public verifier API. |
| `gauss-provider`     | Provider adapters — `ToyProvider` ships now; vendor adapters in Phase 8.             |
| `gauss-sandbox`      | Composite sandbox — WASM (wasmi) + Landlock + seccomp + bwrap + Seatbelt (T10).      |
| `gauss-hwca`         | HWCA worker contexts + schema gate (length cap, JSON Schema 2020-12, instruction-substring filter, taint join) + IPI corpus (A7, T9). |
| `gauss-conformance`  | Axiom-by-axiom test harness (A1–A9, T1–T12).                                         |

## Database — SurrealDB

The Trinity Memory Substrate stores its event log in **SurrealDB** (embedded
mode for tests via `kv-mem`; `kv-surrealkv` and `kv-rocksdb` are additive
feature flags landing in later phases). One engine covers four storage
primitives that the design needs:

| SPECS §                | SurrealDB primitive                                                    |
|------------------------|------------------------------------------------------------------------|
| §8.1 append log        | `DEFINE TABLE turn_record SCHEMAFULL` + UNIQUE indexes on `turn_id`/`seq`. |
| §8.2 FTS keyword index | `DEFINE ANALYZER lower_alphanum` + `DEFINE INDEX … SEARCH ANALYZER BM25`.|
| §8.3 HNSW vector index | `DEFINE INDEX … HNSW DIMENSION 384 TYPE F32 DISTANCE COSINE`.            |
| §VI capability grants  | `DEFINE TABLE agent` + `RELATE` edges to `capability_grant`.              |
| paper §VII lineage     | `DEFINE TABLE derived_from TYPE RELATION FROM turn_record TO turn_record`.|

See [`docs/adr/0006-surrealdb-storage.md`](docs/adr/0006-surrealdb-storage.md)
for the rationale.

## Composite sandbox

The Phase-3 sandbox (`gauss-sandbox`) composes four orthogonal confinement
layers per Theorem T10. The required depth comes from
[`gauss_traits::min_sandbox_for(cap)`](crates/gauss-traits/src/lib.rs):

| Cap                                       | Class | Layers                                        |
|-------------------------------------------|-------|------------------------------------------------|
| `FILESYSTEM_READ`, `CANVAS_RENDER`        | L1    | WASM (wasmi 0.46, fuel-metered)               |
| `FILESYSTEM_WRITE`, `NETWORK_GET`, …      | L2    | + Landlock 5.13+ / Seatbelt                   |
| `NETWORK_POST`, `SUBPROCESS_SPAWN`        | L3    | + bubblewrap (ns) + seccomp                   |
| `CRYPTO_SIGN`                             | L4    | + TEE attestation (Phase 10)                  |

ADR-0009 documents the wasmi (vs wasmtime) and seccompiler (vs libseccomp-rs)
choices and the Phase-10 migration plan.

## HWCA worker boundary

Phase 4 (`gauss-hwca`) implements per-tool worker contexts that locks Axiom A7
and proves Theorem T9 (IPI containment, `≤ 2.19%`). Every tool invocation goes
through:

1. **Worker spawn** — `WorkerSpawner::spawn_and_invoke` allocates a fresh
   `Worker`, increments an `Arc<AtomicU32>` live counter via an RAII guard
   (no `unsafe`; workspace lints forbid it), and enforces a recursion-depth
   bound (default 8).
2. **Schema gate** — four cheap-first checks: per-field length cap, JSON
   Schema 2020-12 (via `jsonschema` 0.46, pure Rust), instruction-substring
   filter for free-text fields, taint join (`incoming ∨ Web`).
3. **Boundary** — only the `ValidatedValue` crosses back to the parent; raw
   tool output is dropped with the worker.

The Phase-4 IPI corpus ships 20 hand-curated attempts across three families
(AgentDojo, EchoLeak, tool-call hijacking); empirical escape rate is `0/20`,
meeting T9's bound with full margin. The AgentDojo + EchoLeak ~10⁵-scenario
integration is a Phase-6 follow-up (ADR-0010).

## Signed receipts + chain anchoring (Phase 5)

Phase 5 (`gauss-audit`) locks Axiom A9 and proves Theorem T11 by adding
Ed25519 signatures and external anchors on top of the Phase-2 SHA-256
chain — without changing the underlying chain primitives.

```text
┌───────────────────────────────────────────────────────────┐
│ run_turn ──► WAL append ──► sign_append ──► (every N)         │
│                                              tsa.anchor(head) │
└───────────────────────────────────────────────────────────┘
```

Three pluggable surfaces, all in `gauss-audit`:

- **`SigningBackend`** — Ed25519 via dalek 2.x (`Ed25519Signer`) is the
  default; HSM / OS keyring / cloud-KMS backends implement the trait
  directly. `ReceiptSigner<B>` is type-erased through
  `DynSigningBackend` so `TurnEngine` carries a single `Arc<…>` regardless
  of backend.
- **`TsaClient`** — async trait producing an `Anchor { kind, head, token,
  … }`. Phase 5 ships `SimulatorTsaClient` (offline Ed25519 simulator,
  test-only); RFC 3161 + `OpenTimestamps` clients are additive Phase-9
  / Phase-10 impls.
- **`AnchorPolicy`** — `SPECS_DEFAULT::every_n_appends = 1000` per SPECS
  §IX.D; `EVERY_APPEND` for high-frequency conformance tests.

The public verifier API is a set of pure functions
(`verify_receipt`, `verify_chain`, `verify_anchor_replay`,
`verifying_key_from_bytes`) — the same surface the Phase-9 HTTP wrapper
calls verbatim. ADR-0011 documents the wire format and migration path.

## Trinity Memory recall (Phase 6)

Phase 6 (`gauss-memory`) locks Axiom A5 and proves Theorems T5 + T12. The
`MemoryBackend` trait gains three recall methods, all of which default to
empty for backends that opt out and override to real SurrealDB queries for
`SurrealMemory`:

- **BM25 keyword recall** via `fts_search(query, limit)` — uses
  SurrealDB's `@0@` match operator over the FTS index built on
  `payload_text`.
- **HNSW vector recall** via `vector_search(query, k)` — uses SurrealDB's
  `<|k|>` k-nearest-neighbour operator over the HNSW index on
  `embedding`. Score is mapped to `1 - cosine_distance` so higher = closer.
- **Hybrid union** via `hybrid_recall(HybridQuery { text, embedding, k,
  alpha })` — deduplicates by `turn_id` and merges scores as
  `α · BM25 + (1 - α) · cosine`. The score-merge helper
  `gauss_traits::merge_hybrid` is the single source of truth.

A `K`-LRU **prefix tree** (paper §VIII.C, default `K = 128`,
`capacity = 512`) caches recently-seen turn prefixes so a warm-context
switch can be served without replaying the entire chain.
[`gauss_memory::klru::PrefixTree<S>`] tracks hit/miss/eviction counts;
the conformance suite asserts the warm-path latency stays well inside the
Theorem T12 `≤ 10 ms p95` bound on a 256-node cache.

A proper Myers `O((N + M) · D)` greedy diff lives at
[`gauss_memory::myers`] alongside the Phase-2 line diff. The patch format
(`Equal { count }` / `Insert { token }` / `Delete { token }`) is the
delta payload of the K-LRU `Node::Delta` variant.

ADR-0012 documents the K-LRU policy + cadence rationale.

## Design tenets

1. **Axioms before features.** Every subsystem traces back to an axiom (A1–A9)
   or theorem (T1–T12) in the paper. See `SPECS.md` §14.
2. **No `unsafe` in privileged crates.** Workspace lints set
   `unsafe_code = "forbid"`.
3. **Lock-free where the CAS pattern is clean.** The three-plane scheduler
   packs `(tokens, epoch_ms)` into one `AtomicU64` and uses CAS loops.
4. **Property-tested algebra.** Lattice laws and chain integrity have
   `proptest` coverage from Phase 0.
5. **Type-state where possible.** The DTE encodes turn-phase ordering in the
   type system, not in runtime branches.
6. **`#[non_exhaustive]` on public enums/structs.** Field/variant additions
   stay semver-minor; explicit constructors are required.
7. **WAL barrier is structural, not behavioural.** Tool execution (sandboxed)
   is unreachable until `memory.append(...)` returns — Axiom A1 by construction.
8. **Worker boundary is structural too.** A tool's raw output cannot reach the
   parent context; only the schema-gate `ValidatedValue` can — Axiom A7 by
   construction (`gauss-hwca`).
9. **Receipts are EUF-CMA from the kernel side.** When a `Signer` is wired,
   every committed turn emits a `SignedReceipt` whose canonical bytes are
   layout-stable across languages and whose Ed25519 signature can be
   verified off-line by any third party — Axiom A9 / Theorem T11 by
   construction (`gauss-audit`).
10. **Recall is monoidal + hybrid.** Memory composition is associative
    with the empty log as identity — Axiom A5 by construction. Recall
    fuses BM25 and HNSW into a single weighted union over the
    deduplicated set of hits — Theorem T5 by construction (`gauss-memory`).

## Quality gates

CI enforces, on every PR:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
  (with `clippy::pedantic` + `clippy::nursery` at warn level)
- `cargo test --workspace`
- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings`
- `cargo deny check`
- MSRV check on 1.83

Later phases add `cargo fuzz`, the fifteen-axis scorecard regression, and
external pen-testing.

## Conformance coverage

| Axiom / Theorem | Status                                                              | Phase locked |
|-----------------|---------------------------------------------------------------------|--------------|
| A1 / T1         | WAL-before-effect; crash-injection harness                          | Phase 2 ✅    |
| A2 / T2         | Capability monotonicity (contract-only grant; CAS-protected)        | Phase 1 ✅    |
| A3              | Receipt-chain tamper-evidence (replay + inclusion witness)          | Phase 2 ✅    |
| A4 / T4         | Plane fairness separation; `B/ρ` worst-case wait bound              | Phase 1 ✅    |
| A6              | Information-flow lattice + antitone declass verifier                | Phase 1 ✅    |
| T3              | Merkle tamper-evidence (proptest: any mutation diverges the head)   | Phase 0/2 ✅  |
| T10             | Composite sandbox bound (cap → class, layer invariants)             | Phase 3 ✅    |
| A7 / T9         | Worker-context isolation + IPI bound (0/20 ≤ 2.19%)                 | Phase 4 ✅    |
| A9 / T11        | Ed25519 signed receipts + chain replay + TSA anchor                 | Phase 5 ✅    |
| A5 / T5 / T12   | Memory monoid + hybrid recall + K-LRU warm-cache bound              | Phase 6 ✅    |
| A8              | Supervised-autonomy gradient                                        | Phase 7      |
| T7              | Provider adjunction                                                 | Phase 8      |
| T6              | Stateless-turn scaling                                              | Phase 10     |
| T8              | Pareto-dominance scorecard                                          | Phase 9      |

## Licence

Dual-licensed under Apache-2.0 or MIT, at the user's option.
