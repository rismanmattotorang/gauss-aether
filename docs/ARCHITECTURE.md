# Gauss-Aether — Architecture Tour

A crate-by-crate walk through the workspace, with each crate's
contract, key types, and conformance test pin.

## Layer cake

```text
Surfaces (user-facing)
└── gauss-canvas · gauss-health · gauss-gateway        (Phase 9)

Verification + scorecard
└── gauss-poly · gauss-bench                           (Phase 8 / 11)

Autonomy + audit
└── gauss-sag · gauss-audit                            (Phases 5 / 7)

Workers + sandbox + memory
└── gauss-hwca · gauss-sandbox · gauss-memory          (Phases 3 / 4 / 6)

Turn execution + policy
└── gauss-turn · gauss-provider                        (Phase 2)

Kernel + trait surface + shared types
└── gauss-kernel · gauss-traits · gauss-core           (Phase 1)

Hardening + research
└── gauss-attest · gauss-chaos                         (Phase 10)
    gauss-zk · gauss-dp · gauss-learnt · gauss-robust  (v2 horizon)
```

---

## Bottom layer — types, traits, kernel

### `gauss-core`

The dependency-free root. Holds:

- `CapToken` — 64-bit capability lattice. `meet`, `join`, `leq`, and
  the canonical capability constants (`FILESYSTEM_READ`, `NETWORK_GET`,
  `CRYPTO_SIGN`, …).
- `TaintLabel { Trusted, User, Web, Adversarial }` — total-chain
  lattice. `join` is the supremum.
- `Action { Text, Tool }`, `Observation`, `TextAction`, `ToolAction`,
  `ToolId`, `TurnId`.
- `GaussError` — the unified error enum (`Denied`, `AutonomyDenied`,
  `SchemaValidation`, `AuditChainBroken`, `SignatureInvalid`,
  `WorkerDepthExceeded`, etc.).

All public structs are `#[non_exhaustive]`; the unsafe-code lint is
forbidden workspace-wide.

### `gauss-traits`

The plugin trait surface. Plugin authors take a dep on `gauss-core` +
`gauss-traits` and nothing else:

- `Kernel` — `current_grant()`, `admit(cap, taint)`.
- `MemoryBackend` — `append`, `chain_head`, `len`, plus Phase-6
  recall methods (`fts_search`, `vector_search`, `hybrid_recall`).
- `Provider` — `generate(observation) -> Vec<Action>`.
- `SandboxTrait` — `class(cap)`, `exec(request)`.
- `ToolTrait` — `manifest()`, `invoke_raw(args)`.
- `OutputSchema`, `SchemaGuards`, `ValidatedValue` — the HWCA worker
  boundary types.

### `gauss-kernel`

The privileged authority. Contains:

- `PrivilegedKernel` — joint K×L admission (`admit(required, taint)`)
  realising `k ⪯ declass(ℓ) ⊓ Kᵢ`.
- `Planes` + `PlanePool` — lock-free three-plane token bucket
  packing `(tokens_fp16.16, epoch_ms)` into one `AtomicU64`.
- `DefaultDeclass`, `StrictDeclass`, `verify_antitone` — info-flow
  declass maps.
- `ConsistentHashRing` (Phase 10) — `SessionId`-keyed cluster
  routing with `DEFAULT_VNODES = 128`.

**Conformance pins:** `axiom_a2_*`, `axiom_a4_*`, `axiom_a6_*`,
`theorem_t4_*`, `theorem_t6_*`.

---

## Middle layer — turn engine, memory, sandbox

### `gauss-turn`

The Differential Turn Engine. Algorithm 1 of the paper:

```text
1. INGEST    join taint(o) into ℓ
2. GENERATE  ask provider π for actions
3. ADMIT     kernel.admit(k(a), ℓ) for each tool action
3a. SAG      classifier + approval surface  (Phase 7)
4. WAL       memory.append(record(o, a, ℓ, sag_decisions))  ← A1 barrier
5. SIGN      receipt_signer.sign_append(...)                  (Phase 5)
6. COMMIT    sandbox.exec(...) for each tool action          (Phase 3)
```

The engine is generic over `K, M, P`; sandboxes and signers are
type-erased trait objects. Constructors: `new`, `with_sandbox`,
`with_signing`, `with_all`, `.with_sag(...)`.

The WAL barrier is **structural**: `apply_actions_locally(...)` is
unreachable before `memory.append(...).await?` returns.

**Conformance pin:** `axiom_a1_wal_before_effect`.

### `gauss-memory`

Trinity Memory — SurrealDB-backed substrate:

- `SurrealMemory` — embedded `kv-mem` default; persistent backends
  behind `kv-surrealkv` and `kv-rocksdb` features.
- Three indices share the same table: FTS (BM25 via `@0@`), HNSW
  KNN (via `<|k|>`), graph lineage (via `RELATE`).
- `myers::diff` / `myers::Patch` — `O((N+M)·D)` ADT-aware diff
  (Phase 6).
- `PrefixTree<S>` — K-LRU radix prefix cache, default K=128.
- `merge_hybrid(fts, vec, alpha, k)` — weighted union of BM25 + HNSW
  ranks.

**Conformance pins:** `axiom_a5_*`, `theorem_t5_*`, `theorem_t12_*`.

### `gauss-sandbox`

Composite sandbox:

- L1 — WebAssembly via `wasmi` 0.46 (default). Optional `wasmtime`
  via `--features wasm-wasmtime` (needs Rust 1.85+).
- L2 — Linux Landlock 5.13+ (`landlock` 0.4) / macOS Seatbelt
  (`sandbox-exec`).
- L3a — Linux user namespaces via `bubblewrap` subprocess.
- L3b — Linux seccomp via `seccompiler` 0.5 (pure Rust).
- L4 — TEE attestation hook via `gauss-attest` (`--features tee-attest`).

`min_sandbox_for(cap)` picks the minimum class; `CompositeSandbox`
refuses tools whose declared cap exceeds the layers.

**Conformance pin:** `theorem_t10_composite_sandbox`.

### `gauss-hwca`

HWCA per-tool worker contexts + four-stage schema gate:

1. Per-field length cap.
2. JSON Schema 2020-12 via `jsonschema` 0.46 (pure Rust).
3. Instruction-substring filter (deny-list — AgentDojo + EchoLeak +
   system-tag patterns).
4. Taint join: outgoing = incoming ∨ Web.

`WorkerSpawner` uses an `Arc<AtomicU32>` RAII live-counter (no
`unsafe`); `IpiCorpus` ships 20 synthetic IPI attempts; empirical
escape rate is 0 / 20 (paper bound ≤ 2.19 %).

**Conformance pin:** `axiom_a7_and_theorem_t9_hwca`.

---

## Audit + autonomy

### `gauss-audit`

The receipt chain:

- `ChainHead` + `link` + `ReceiptChain` (Phase 2).
- `SignedReceipt` + `Ed25519Signer` + `ReceiptSigner<B>` (Phase 5);
  pluggable `SigningBackend` for HSM / KMS.
- `Anchor` + `AnchorKind { Rfc3161, OpenTimestamps, Simulator }` +
  `TsaClient` async trait + Ed25519 `SimulatorTsaClient`.
- `AnchorPolicy` (default: every 1000 appends, SPECS §IX.D).
- `verify_receipt`, `verify_chain`, `verify_anchor_replay` — the
  public verifier API.

**Conformance pins:** `theorem_t3_*`, `axiom_a3` (in `gauss-audit`
unit tests), plus Phase-5 receipt round-trips.

### `gauss-sag`

Supervised Autonomy Gradient:

- `Risk { Auto, Notify, RequireApproval, Deny }`.
- `RiskInputs { cap, taint, reversible, tool }`.
- `Predicate` algebra: `Always`, `ContainsCap`, `TaintAtLeast`,
  `NonReversible`, `Tool`, `All`, `Any`.
- `DecisionTable` — ordered rules + fall-through `Risk`;
  `default_decision_table()` encodes paper §XI.B.
- `verify_monotonicity` — build-time grid check.
- `ApprovalSurface` trait + `AutoApprove`, `AutoDeny`,
  `ChannelSurface` test impls.
- `ApprovalGate<C>` — `decide_action(turn_id, action, taint) ->
  Outcome`; 5-minute default deadline (paper §XI.C).

**Conformance pin:** `axiom_a8_sag_approval`.

---

## Top layer — surfaces, verifier, scorecard

### `gauss-poly`

Polyhedral trait-equivalence verifier:

- `Probe<I, O>`, `PolyhedralProbeSet<I, O>`.
- `verify_provider_equivalence(&p, &q, &probes)` — canonical-JSON
  byte-equal comparison.
- `ProviderEquivalenceReport` + `SwapEquivalenceError` for diagnostics.

The same shape generalises to other plugin traits via
`verify_<trait>_equivalence` helpers.

**Conformance pin:** `theorem_t7_provider_adjunction`.

### `gauss-canvas`, `gauss-health`, `gauss-gateway`

The Phase-9 surface layer:

- **Canvas**: 8 widget kinds (`Text`, `Button`, `KeyValueTable`,
  `Image`, `ApprovalPrompt`, `Container`, `Markdown`, `Custom`) + 4
  ops (`Insert`, `Update`, `Delete`, `Reorder`). `Canvas` trait +
  `InMemoryCanvas` impl; `tokio::sync::broadcast` subscribers.
- **Health**: `HealthSubject` trait + `Invariant` + `HealthReport`
  serde wire. `HealthEngine::default()` installs the seven minimum
  invariants from SPECS §XIII.C.
- **Gateway**: wire types for `POST /v1/turn`, `GET /v1/health`,
  SSE `StreamEvent`, and the OpenAI-compatible `/v1/chat/completions`
  proxy. No HTTP server — Phase-11 additive crate.

**Conformance pin:** `theorem_t8_pareto_dominance`.

### `gauss-bench`

Phase-11 Pareto-dominance scorecard:

- `Axis` — 15 named axes from paper §XVIII.A.
- `Scorecard::pareto_dominates(&other)` — 1.0 release pin.
- `predecessor_baselines()` — Hermes, OpenFang, ZeroClaw, OpenClaw
  from paper §XVIII.B Table 4.
- `gauss_aether_one_point_zero()` — the regression-pinned 1.0
  scorecard.

**Conformance pin:** `phase11_release::one_point_zero_pareto_dominates_every_predecessor`.

---

## Hardening + research

### `gauss-attest`

TEE attestation:

- `Attestor` async trait + `AttestKind { SevSnp, TdxIntel, ArmCca,
  Simulator }`.
- `AttestationReport` with a layout-stable canonical pre-image:
  `kind ‖ measurement ‖ nonce ‖ ts ‖ workload ‖ version ‖ node`.
- `SoftwareSimAttestor` — Ed25519-backed deterministic simulator.
- `verify_report(report, nonce, trusted_keys, baseline)` — short-circuits
  on the first failure.

Production backends (`gauss-attest-sevsnp`, …) ship as additive
plugin crates that implement `Attestor` over the platform's
attestation service.

**Conformance pin:** `theorem_t6_stateless_scaling_and_attest`.

### `gauss-chaos`

Deterministic chaos injectors:

- `KillSwitch` — atomic arm flag + poll counter.
- `Partition<T>` — FIFO queue with drop counter on partition.
- `ClockSkew` — signed offset with saturating apply.
- `ChaosBudget` — bundle of the three.

**Conformance pin:** `chaos_phase10`.

### v2 horizon

- `gauss-zk` — Pedersen-style commitments + `Statement::InclusionInChain`
  / `HeadAtLength` + `verify(statement, witness)`. Production SNARK
  plugins (`gauss-zk-groth16`, `gauss-zk-halo2`) replace the cleartext
  witness with a succinct proof.
- `gauss-dp` — `Mechanism` trait + `Laplace` (ε-DP) + `Gaussian`
  ((ε,δ)-DP) + `PrivacyAccountant` (basic composition).
- `gauss-learnt` — `LogisticScorer` over four hand-engineered
  features; `LearntClassifier` joins (table, scorer) so the scorer
  can only *tighten* the rule table's verdict (monotone safety).
- `gauss-robust` — `RobustDeclass` wraps a base declass map and
  tightens it as adversarial-rejection counters cross a threshold.
- `proofs/lean/` — Lean 4 stubs of all nine axioms + twelve theorems.

---

## Cross-layer guarantees

A few invariants that hold across crate boundaries:

1. **WAL barrier** (`gauss-turn` × `gauss-memory`): `apply_actions_locally`
   is unreachable until `memory.append(...).await?` returns Ok.
2. **Worker boundary** (`gauss-turn` × `gauss-hwca`): only the
   `ValidatedValue` crosses back from the worker — the raw tool
   output is dropped at the boundary.
3. **Receipt covers approval** (`gauss-turn` × `gauss-sag` ×
   `gauss-audit`): the per-turn `SagDecisionRecord` is bundled into
   the canonical payload, so the Phase-5 signed receipt covers the
   approval verdict.
4. **Sandbox after WAL** (`gauss-turn` × `gauss-sandbox`): no tool
   side-effect fires before the WAL append commits.
5. **Antitone declass** (`gauss-kernel` × `gauss-robust`): the
   robust declassifier preserves the `T → CapToken` antitonicity
   even after adversarial tightening.

These are checked by the conformance suite, not by code review —
adding a new admission path that violates any of them fails CI.
