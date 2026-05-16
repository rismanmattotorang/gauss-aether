# Gauss-Aether

An axiomatic operating system for trustworthy autonomous LLM agents,
implemented in Rust.

> Status: **Phase 3 complete** — workspace, lock-free three-plane kernel with
> joint capability/taint admission, Differential Turn Engine with WAL-before-effect
> barrier, SurrealDB-backed Trinity Memory log (graph + vector + FTS + chain),
> SHA-256 receipt chain with replay + inclusion-witness verification, line-level
> Myers diff for transcript snapshots, deterministic toy provider, and a
> **composite sandbox** (WASM via wasmi ∧ Linux Landlock ∧ seccomp ∧ bubblewrap
> ∧ macOS Seatbelt) with capability-bound depth (T10). **90 tests pass** across
> 9 crates; Phases 4–11 add HWCA, signed receipts, hybrid recall, SAG, trait
> verifier, A2UI Canvas, SDHE, and 1.0 release (see [`ROADMAP.md`](./ROADMAP.md)).

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

## Workspace layout (after Phase 3)

| Crate                | Purpose                                                                              |
|----------------------|--------------------------------------------------------------------------------------|
| `gauss-core`         | Shared types: identifiers, actions, observations, taint, `CapToken` lattice, errors. |
| `gauss-traits`       | Public trait surface — `Kernel`, `MemoryBackend`, `Provider`, `SandboxTrait`.        |
| `gauss-kernel`       | Privileged kernel: joint K×L admission, lock-free 3-plane token bucket, declass map. |
| `gauss-turn`         | Differential Turn Engine — Algorithm 1 with optional sandbox executor.               |
| `gauss-memory`       | Trinity Memory: SurrealDB-backed append log + HNSW + FTS + graph lineage + Myers diff.|
| `gauss-audit`        | SHA-256 chain + replay verification + inclusion witnesses (Ed25519 in Phase 5).      |
| `gauss-provider`     | Provider adapters — `ToyProvider` ships now; vendor adapters in Phase 8.             |
| `gauss-sandbox`      | Composite sandbox — WASM (wasmi) + Landlock + seccomp + bwrap + Seatbelt (T10).      |
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
| A7 / T9         | Worker-context isolation + IPI bound                                | Phase 4      |
| A5 / T5 / T12   | Hybrid recall + delta context-switch                                | Phase 6      |
| A8              | Supervised-autonomy gradient                                        | Phase 7      |
| T7              | Provider adjunction                                                 | Phase 8      |
| A9 / T11        | EUF-CMA receipts + TSA anchor                                       | Phase 5      |
| T6              | Stateless-turn scaling                                              | Phase 10     |
| T8              | Pareto-dominance scorecard                                          | Phase 9      |

## Licence

Dual-licensed under Apache-2.0 or MIT, at the user's option.
