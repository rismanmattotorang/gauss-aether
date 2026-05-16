# Gauss-Aether

An axiomatic operating system for trustworthy autonomous LLM agents,
implemented in Rust.

> Status: **Phase 10 complete** — full eleven-tuple stack locked. The
> kernel admits + the WAL appends + the sandbox runs + the schema gate
> filters + the receipt chain signs + Trinity Memory recalls + SAG
> gates approvals + the polyhedral verifier proves provider
> equivalence (T7) + the live canvas + health engine + gateway
> render the surface layer (T8) + the cluster ring scales statelessly
> (T6) + the attestation surface witnesses Layer 4 (T10 §L4). **263
> tests pass** across 17 crates; Phase 11 closes out the
> Pareto-dominance scorecard regression + 1.0 release (see
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

## Workspace layout (after Phase 10)

| Crate                | Purpose                                                                              |
|----------------------|--------------------------------------------------------------------------------------|
| `gauss-core`         | Shared types: identifiers, actions, observations, taint, `CapToken` lattice, errors. |
| `gauss-traits`       | Public trait surface — `Kernel`, `MemoryBackend`, `Provider`, `SandboxTrait`, `ToolTrait`, `OutputSchema`, `SchemaGuards`, `ValidatedValue`. |
| `gauss-kernel`       | Privileged kernel: joint K×L admission, lock-free 3-plane token bucket, declass map, **consistent-hash cluster ring (Phase 10, T6)**. |
| `gauss-turn`         | Differential Turn Engine — Algorithm 1 with optional sandbox + signed receipts + SAG approval gate. |
| `gauss-memory`       | Trinity Memory: SurrealDB-backed append log + BM25 + HNSW hybrid recall + K-LRU prefix tree + Myers diff. |
| `gauss-audit`        | SHA-256 chain + Ed25519 `SignedReceipt` + RFC 3161 / `OpenTimestamps` anchor traits + offline simulator + public verifier API. |
| `gauss-provider`     | Provider adapters — `ToyProvider` ships now; vendor adapters in Phase-9+ plugin crates. |
| `gauss-sandbox`      | Composite sandbox — WASM (wasmi) + Landlock + seccomp + bwrap + Seatbelt + optional TEE-attest feature (T10). |
| `gauss-hwca`         | HWCA worker contexts + schema gate (length cap, JSON Schema 2020-12, instruction-substring filter, taint join) + IPI corpus (A7, T9). |
| `gauss-sag`          | Supervised Autonomy Gradient — `DecisionTable` + monotonicity verifier + `ApprovalSurface` trait + test surfaces (A8). |
| `gauss-poly`         | **Phase 8** polyhedral trait-equivalence verifier — `Probe<I, O>` + `verify_provider_equivalence` (T7). |
| `gauss-canvas`       | **Phase 9** A2UI Live Canvas Protocol — typed widget tree + `Insert/Update/Delete/Reorder` update stream + `InMemoryCanvas` (T8). |
| `gauss-health`       | **Phase 9** Self-Diagnosable Health Engine — seven minimum invariants + custom registration + `HealthReport` serde (T8). |
| `gauss-gateway`      | **Phase 9** wire types — REST/WS/SSE shapes + OpenAI-compatible proxy + SSE stream events (T8). |
| `gauss-attest`       | **Phase 10** TEE attestation surface — `Attestor` trait + Ed25519 software simulator + canonical wire format (T10 §L4). |
| `gauss-chaos`        | **Phase 10** chaos-engineering harness — kill switch + partition + clock skew injectors. |
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
┌────────────────────────────────────────────────────────────┐
│ run_turn ──► WAL append ──► sign_append ──► (every N)         │
│                                              tsa.anchor(head) │
└────────────────────────────────────────────────────────────┘
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

## Supervised Autonomy Gradient (Phase 7)

Phase 7 (`gauss-sag`) locks Axiom A8 by interposing a four-band risk
classifier + human-approval round-trip between kernel admission and the
WAL append:

```text
admit ──► classify ──► (Auto/Notify) ──► WAL append ──► sandbox exec
                  │
                  ├─► (RequireApproval) ──► surface ──► approved? ─► WAL
                  │                                  └─► denied/timeout: AutonomyDenied / AutonomyApprovalTimeout
                  └─► (Deny) ──► AutonomyDenied
```

Three pluggable surfaces:

- **`DecisionTable`** — ordered `Vec<Rule>` + fall-through `Risk`.
  Predicates: `Always`, `ContainsCap`, `TaintAtLeast`, `NonReversible`,
  `Tool`, `All`, `Any`. The default table denies adversarial taint,
  requires approval for `CRYPTO_SIGN` and non-reversible
  `NETWORK_POST` / `SUBPROCESS_SPAWN`, notifies on Web taint or
  non-reversibility, and auto-fires the rest.
- **`verify_monotonicity`** — build-time property check: relaxing any
  input field never tightens the outcome (paper §XI.B).
- **`ApprovalSurface`** — async trait with three test impls
  (`AutoApprove`, `AutoDeny`, `ChannelSurface` driven by
  `tokio::sync::mpsc`). Production adapters (Telegram, Slack, Discord,
  Matrix, CLI/TUI, SSE) ship in Phase 9 as additive impls.

`TurnEngine::with_sag(gate)` attaches the gate. The per-turn `TurnSummary`
carries a `Vec<SagDecisionRecord>` bundled into the canonical payload so
the Phase-5 signed receipt covers the approval verdict as well as the
action set. Default deadline: 5 minutes (`DEFAULT_DEADLINE`), per SPECS
§XI.C; deny-on-timeout. ADR-0013 documents the policy + Phase-9 adapter
migration path.

## Polyhedral trait verifier (Phase 8)

`gauss-poly` ships the build-time check that proves Theorem T7
(provider adjunction): two `Provider` impls are **polyhedrally
equivalent** iff they produce byte-equal canonical actions on every
input in a finite probe set.

```rust
use gauss_poly::{verify_provider_equivalence, PolyhedralProbeSet};

let report = verify_provider_equivalence(&new_provider, &reference, &probes).await?;
assert!(report.ok(), "provider divergence: {:?}", report);
```

ADR-0014 documents the four `specT` style-guide rules every plugin
trait follows (serializable outputs, `#[non_exhaustive]`, unified
error, probe-set-checkable invariants). The same shape generalises to
every plugin trait — additional `verify_<trait>_equivalence` helpers
land as new plugin surfaces stabilise.

## A2UI Canvas + Health + Gateway (Phase 9)

Phase 9 (`gauss-canvas`, `gauss-health`, `gauss-gateway`) ships the
**surface layer** — the trait surfaces user-facing tooling consumes:

- **Canvas** (`gauss-canvas`): eight-widget typed tree (`Text`,
  `Button`, `KeyValueTable`, `Image`, `ApprovalPrompt`, `Container`,
  `Markdown`, `Custom`) + four ops (`Insert`, `Update`, `Delete`,
  `Reorder`). `InMemoryCanvas` + a `tokio::sync::broadcast` channel
  power the deterministic Phase-9 build; Phase-10 cluster mode swaps
  in a `SurrealCanvas` backed by SurrealDB live queries.
- **Health** (`gauss-health`): the SPECS §XIII.C seven minimum
  invariants — WAL barrier armed, kernel grant non-bottom, no leaked
  HWCA workers, signer present, sandbox present, SAG present,
  monotone grant — wired through a `HealthSubject` trait so
  deployments plug their `TurnEngine` adapter in. `HealthEngine::
  evaluate(&subject) -> HealthReport` is the `gauss doctor` shape.
- **Gateway** (`gauss-gateway`): the wire shapes for `POST /v1/turn`,
  `GET /v1/health`, the SSE stream events, and the OpenAI-compatible
  `/v1/chat/completions` proxy schema. The actual `axum` server is a
  Phase-11 additive crate; the wire types here are the stable
  contract.

ADR-0015 documents the widget-set freeze + Phase-10 streaming
migration.

## Hardening, scale, attestation (Phase 10)

- **Cluster routing** (`gauss-kernel::cluster`): a consistent-hash
  ring keyed on `SessionId` so a Phase-10 cluster horizontally scales
  stateless turn execution; adding / removing one node moves only
  `O(1/N)` of the existing sessions (Theorem T6).
- **TEE attestation** (`gauss-attest`): an `Attestor` trait + Ed25519
  software simulator with a layout-stable canonical wire format. The
  verifier short-circuits on nonce mismatch, measurement mismatch,
  untrusted key, or invalid signature. Hardware backends (AMD SEV-SNP,
  Intel TDX, ARM CCA) ship as additive plugin crates that wrap the
  same trait — ADR-0016 documents the migration.
- **wasmtime** WASM backend swap is a feature flag on `gauss-sandbox`
  (`--features wasm-wasmtime`); the wasmi default ships on the
  workspace MSRV.
- **Chaos** (`gauss-chaos`): `KillSwitch` + `Partition<T>` +
  `ClockSkew` injectors with deterministic semantics. Conformance
  tests exercise each injector's invariants; the Phase-11 chaos suite
  builds on this foundation.

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
11. **Autonomy is gated by an auditable table.** A monotone
    `DecisionTable` decides which actions auto-fire, which notify,
    which require human approval, and which are refused outright;
    approval verdicts are bundled into the signed payload — Axiom A8 by
    construction (`gauss-sag`).

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
| A8              | SAG decision table + approval round-trip + deny-on-timeout          | Phase 7 ✅    |
| T7              | Provider adjunction (polyhedral equivalence on probe set)           | Phase 8 ✅    |
| T8              | Surface scorecard (Canvas + Health + Gateway wire types)            | Phase 9 ✅    |
| T6              | Stateless-turn scaling (consistent-hash routing on `SessionId`)     | Phase 10 ✅   |
| T10 §L4         | TEE attestation simulator + verifier (hardware backends are plugins) | Phase 10 ✅   |

## Licence

Dual-licensed under Apache-2.0 or MIT, at the user's option.
