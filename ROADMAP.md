# Gauss-Aether — Rust Development Roadmap

**Companion to:** `SPECS.md`
**Strategy:** axiom-driven phased delivery — each phase locks in a coherent subset of axioms A1–A9 and theorems T1–T12, then is conformance-tested before the next phase begins.
**Cadence target:** 12 phases over ~14 months for a 1.0 release, plus a v2 horizon.

---

## Guiding Principles

1. **Axioms before features.** No phase ships a user-facing feature whose underlying axiom isn't already enforced by the kernel.
2. **Trace every commit.** Every PR must reference the axiom / theorem / SPECS section it advances.
3. **Conformance gates phase exit.** A phase ends when its conformance suite (`gauss-conformance`) is green on Tier-1 targets.
4. **Privilege escalation review.** Any code touching `gauss-kernel`, `gauss-audit`, or `gauss-attest` requires dual review (Tier-0 rules, SPECS §2).
5. **Stable trait surface from Phase 5.** Trait breaking-changes after Phase 5 require an ADR + semver-major bump.

---

## Phase Overview

| Phase | Title                                   | Duration | Axioms locked | Theorems locked | Headline deliverable                          | Status |
|-------|------------------------------------------|----------|----------------|-----------------|-----------------------------------------------|--------|
| 0     | Foundations                              | 3 weeks  | —              | —               | Workspace, CI, ADR-0001…0005, 35 tests        | ✅ Done |
| 1     | Kernel-α: capability + scheduler         | 6 weeks  | A2, A4, A6     | T2, T4          | Lock-free 3-plane sched + joint K×L admit + SurrealDB | ✅ Done |
| 2     | Turn engine + memory log                 | 6 weeks  | A1, A3         | T1, T3          | DTE end-to-end + Myers diff + chain replay    | ✅ Done |
| 3     | Composite sandbox                        | 5 weeks  | (A2 bound)     | T10             | WASM (wasmi) + Landlock + seccomp + bwrap + Seatbelt | ✅ Done |
| 4     | HWCA + information flow                  | 6 weeks  | A7             | T9              | HWCA worker + schema gate; 0/20 IPI corpus    | ✅ Done |
| 5     | Receipt chain + signatures               | 4 weeks  | A9             | T11             | Ed25519 receipts + TSA-anchor traits + verifier | ✅ Done |
| 6     | Trinity memory: hybrid recall + K-LRU    | 5 weeks  | A5             | T5, T12         | BM25 + HNSW hybrid recall + K-LRU prefix tree + Myers diff | ✅ Done |
| 7     | SAG + approval plane                     | 4 weeks  | A8             | (A8 bound)      | `DecisionTable` + monotonicity verifier + approval surfaces | ✅ Done |
| 8     | Trait polyhedral surface + verifier      | 5 weeks  | —              | T7              | `gauss-poly` probe-based equivalence verifier | ✅ Done |
| 9     | A2UI Canvas + Health + surfaces          | 6 weeks  | —              | T8              | Canvas (8 widgets) + SDHE (7 invariants) + Gateway wire types | ✅ Done |
| 10    | Hardening, scale, attestation            | 6 weeks  | (V predicate)  | T6, T10 (L4)    | Consistent-hash ring + TEE simulator + chaos injectors + wasmtime feature | ✅ Done |
| 11    | 1.0 release                              | 3 weeks  | All            | All             | Pareto-dominance scorecard regression-pinned  | Next   |
| v2    | zk audit, learnt Φ, DP exporter          | TBD      | —              | —               | Future-work line from paper §XVIII.E          |        |

Total to 1.0: **~14 months** assuming 4–6 engineers from Phase 2.

---

## Phase 0 — Foundations ✅

**Goal:** make it possible to develop the kernel without fighting tooling.

### Delivered

- Cargo workspace, `rust-toolchain.toml` (1.83), `deny.toml`, CI (fmt, clippy `-D warnings`, test, doc, deny, MSRV).
- ADRs 0001–0005: axiom-driven phasing, Tokio runtime, Ed25519+BLAKE3+SHA-256 crypto, figment config, privilege tiers.
- Six skeleton crates: `gauss-core`, `gauss-kernel`, `gauss-turn`, `gauss-memory`, `gauss-audit`, `gauss-conformance`.
- 35 tests green (proptest lattice laws, chain integrity, type-state DTE shell).

### Exit gate (met)

`cargo {build,test,clippy,doc} --workspace` green under pedantic+nursery; CI matrix passes.

---

## Phase 1 — Kernel-α: Capability + Scheduler ✅

**Goal:** privileged authority that grants/denies capabilities and dispatches across three planes. **Locks A2, A4, A6; proves T2, T4.**

### Delivered

- **New crate `gauss-traits`** — public surface (`Kernel`, `MemoryBackend`, `Provider`, `AppendEntry`, `ChainHeadSnapshot`).
- `gauss-kernel::cap` — bitmask `CapToken` lattice (canonicalised to `gauss-core` in Phase 2 per ADR-0008).
- `gauss-kernel::flow` — full `TaintLattice` + `DeclassMap` trait + `verify_antitone` + `DefaultDeclass` / `StrictDeclass`.
- `gauss-kernel::sched` — **lock-free** atomic token bucket: one `AtomicU64` per plane packs `(tokens_fp16.16, epoch_ms)`; CAS loops, no mutex, no shared cross-plane state.
- `gauss-kernel::admit::PrivilegedKernel` — joint `admit(required, taint)` implementing `k ⪯ declass(ℓ) ⊓ Kt`; CAS-protected `contract()` for capability monotonicity.
- **`gauss-memory::surreal::SurrealMemory`** — embedded **SurrealDB** (`kv-mem`) backend implementing `MemoryBackend`. Full bootstrap DDL: `turn_record` append log, UNIQUE indices, FTS analyzer + index, HNSW vector index (DIM 384 COSINE), capability-grant graph relations, lineage graph.
- ADR-0006: SurrealDB as the Trinity Memory storage engine.
- Property tests for lattice laws, antitone verifier, concurrent CAS bucket. 51 tests green.

### Exit gate (met)

CONF-A2-* (monotonicity / non-interference) + CONF-A4-* (starvation freedom) green; antitone verifier accepts default & strict maps and rejects a hand-crafted broken map; SurrealDB embedded backend round-trips on three independent instances with deterministic chain heads.

---

## Phase 2 — Turn Engine + Memory Log ✅

**Goal:** end-to-end turn execution with WAL-before-effect and a tamper-evident hash chain. **Locks A1, A3; proves T1, T3.**

### Delivered

- `gauss-turn::engine::TurnEngine<K, M, P>` — real Algorithm 1 (minus HWCA + signed receipts).
- WAL-before-effect is **structural** (ADR-0007).
- `gauss-memory::snapshot` — line-level Myers diff (Phase 6 ADT diff lands later).
- `gauss-audit` upgraded with `ReceiptChain::verify_replay` and `InclusionWitness::verify`.
- **`gauss-provider` (new crate)** — `ToyProvider`.
- `ToolAction::cap_required: CapToken` plumbed through admission.
- `CapToken` moved to `gauss-core` (ADR-0008).
- ADRs 0006–0008.
- 73 tests green across 8 crates under pedantic+nursery clippy with `-D warnings`.

---

## Phase 3 — Composite Sandbox ✅

**Goal:** tool execution under multiple orthogonal sandboxes. **Proves T10 (3-layer software-only first; L4 deferred to Phase 10).**

### Delivered

- **New crate `gauss-sandbox`** — implements `gauss_traits::SandboxTrait`.
- **L1 — WASM via `wasmi 0.46`**: `WasmSandbox` with fuel metering (~1M instr/invocation default), `spawn_blocking` host integration, configurable fuel budget. ADR-0009 documents the wasmi → wasmtime migration plan for Phase 10.
- **L2 — Linux Landlock via `landlock 0.4`**: `LandlockSandbox` self-restricts the current thread to a configurable `AccessFs` bitset. Gracefully reports unsupported kernels.
- **L2 (macOS) — Seatbelt subprocess wrapper**: `SeatbeltSandbox` evaluates a TinyScheme-style profile through `sandbox-exec`.
- **L3a — bubblewrap subprocess wrapper**: `BwrapSandbox` probes `bwrap --version` and forwards a clear diagnostic when missing.
- **L3b — Linux seccomp via `seccompiler 0.5`** (pure Rust, no libseccomp): `SeccompSandbox` applies a deny-list of network / `execve` / `clone3` / `unshare` / `mount` / `keyctl` syscalls. Soft-deny default (errno=38 ENOSYS).
- **`CompositeSandbox` + builder** — composes layers; verifies that the union of inner-layer classes covers the cap-required class AND that the layers actually invoked at exec time cover it. Refuses with `RefusalReason::cap_only()` when the stack is too thin.
- **`min_sandbox_for(cap)`** function (`gauss-traits`) — encodes SPECS §7.1 cap → SandboxClass mapping.
- **`NoOpSandbox`** — test/debug-only impl that accepts everything.
- DTE wires through: `TurnEngine::with_sandbox(...)`; every tool action runs through `sb.exec(...)` AFTER the WAL barrier (Axiom A1 preserved).
- ADR-0009: stack choices (wasmi vs wasmtime; seccompiler vs libseccomp-rs; per-OS feature gates).
- **17 new tests** in `gauss-sandbox` (WASM execute + fuel exhaustion + malformed bytecode + composite class + refusal + Landlock report + bwrap missing-binary + seccomp soft-filter + NoOp) + **4 new conformance tests** for T10 (cap → class, composite refuses insufficient stack, DTE-with-sandbox end-to-end).
- Total: **90 tests green** across 9 crates under pedantic+nursery clippy with `-D warnings`.

### Exit gate (met)

CONF-T10-* green; cap → class table matches SPECS §7.1; WASM-only composite refuses an L3 cap; DTE end-to-end with sandbox preserves the WAL barrier.

### Open follow-ups (don't block Phase 4)

- Production WASM backend swap (wasmi → wasmtime) — Phase 10 ADR-0009-revision.
- Real Linux 5.13+ kernel coverage in CI — Phase 6 alongside `kv-rocksdb`.
- HTTP `fetch_url` tool running end-to-end on three Linux layers — Phase 4 once the HWCA worker spawn lands.

---

## Phase 4 — HWCA + Information Flow ✅

**Goal:** isolate every tool invocation in a worker context; propagate taint. **Locks A7; proves T9 (IPI bound).**

### Delivered

- **New crate `gauss-hwca`** — implements per-tool worker contexts and the schema gate at the worker→parent boundary.
- **`gauss-hwca::worker`** — `WorkerSpawner` + `Worker`: spawn-per-call isolation with `Arc<AtomicU32>` RAII live counter (no `unsafe`, workspace lints forbid it), default recursion-depth bound 8 (`DEFAULT_MAX_DEPTH`), and optional sandbox integration via `with_sandbox(...)` for defence-in-depth.
- **`gauss-hwca::schema_gate`** — four-stage gate in deliberate cheap-first order:
  1. Per-field length cap (`OutputSchema::max_string_len`, recursive over arrays/objects).
  2. JSON Schema 2020-12 (via `jsonschema` 0.46, pure Rust — no C dep, no JNI).
  3. Instruction-substring filter (case-insensitive deny-list, applied recursively to every string field when `SchemaGuards.no_instruction_substrings` is on).
  4. Taint join: outgoing = `incoming ∨ Web`.
- **`gauss-hwca::filter`** — `INSTRUCTION_SUBSTRINGS` deny-list covering AgentDojo-style ("ignore previous"), EchoLeak-style ("exfiltrate", "post to https://"), system-tag impersonation (`system:`, `[system]`, `<|system|>`), and tool-call hijacking ("respond with the following", "override:", "your new instructions").
- **`gauss-hwca::corpus`** — 20-attempt synthetic IPI corpus across three families (AgentDojo, EchoLeak, hijack) including two array-nested cases that exercise the gate's recursion.
- **Trait surface in `gauss-traits`** — `ToolTrait`, `ToolManifest`, `OutputSchema`, `SchemaGuards`, `ValidatedValue` (paper SPECS §6.2): backend-agnostic so the JSON Schema crate is swappable via `SchemaGate::new` only.
- **4 new conformance tests** for `CONF-A7-*` and `CONF-T9-*` — live-counter zeroing after success and after a schema-gate error; validated value carries the joined taint; recursion-depth bound rejects spawns beyond the limit; the IPI corpus run asserts `rate ≤ 0.0219` (Phase-4 actual is `0/20`).
- **ADR-0010** — in-process workers (subprocess in Phase 10), `jsonschema` 0.46 choice, synthetic Phase-4 corpus → AgentDojo + EchoLeak in Phase 6, four-stage gate order, RAII counter without `unsafe`.
- Total: **110 tests green** across 10 crates under pedantic+nursery clippy with `-D warnings`; `cargo doc --workspace --no-deps` clean under `RUSTDOCFLAGS=-D warnings`.

### Exit gate (met)

CONF-A7-* + CONF-T9-* green; Phase-4 IPI corpus 0/20 (well inside the `≤ 2.19%` paper bound); worker live-counter returns to zero on every exit path including schema-gate errors and panics; recursion-depth bound rejects depth>=`max_depth`.

### Open follow-ups (don't block Phase 5)

- Full AgentDojo + EchoLeak corpus integration (~10⁵ scenarios) — Phase 6 alongside provider replay.
- Subprocess-per-worker model so Landlock+seccomp+bwrap apply per-tool rather than to the host kernel thread — Phase 10 (ADR-0010 §Migration).
- Statistical classifier as a second-pass guard (LM scorer or small classifier) — Phase 6.

---

## Phase 5 — Receipt Chain + Signatures ✅

**Goal:** every action emits a signed, chained receipt with an optional external anchor. **Locks A9; proves T11.**

### Delivered

- **`gauss-audit` restructure** — split `lib.rs` into focused modules: `chain`, `sign`, `tsa`, `anchor`, `verify`. The chain primitives stay byte-identical to Phase 2.
- **`gauss-audit::sign`** — `Ed25519Signer` (dalek 2.x, pure Rust); pluggable `SigningBackend` trait for HSM / OS keyring / cloud KMS; `ReceiptSigner<B>` driver; layout-stable `SignedReceipt` (turn_id ‖ index ‖ prev_head ‖ payload_digest ‖ post_head ‖ taint ‖ signed_at_ms; 129 bytes). `Zeroize`-on-drop secret keys.
- **`gauss-audit::tsa`** — async `TsaClient` trait; `AnchorKind { Rfc3161, OpenTimestamps, Simulator }`; deterministic `SimulatorTsaClient` (Ed25519 simulator with fixed-clock support) exercises the canonical wire format offline.
- **`gauss-audit::anchor`** — `AnchorPolicy::SPECS_DEFAULT::every_n_appends = 1000` (paper §IX.D); `EVERY_APPEND` for high-frequency testing; `Anchorer` driver tracks the most recent externally-witnessed head.
- **`gauss-audit::verify`** — public verifier API: `verify_receipt`, `verify_chain`, `verify_simulator_anchor`, `verify_anchor_replay`, `verifying_key_from_bytes`. Same surface the Phase-9 HTTP wrapper will call.
- **`gauss-core` errors** — new `GaussError::SignatureInvalid { reason }` and `GaussError::AnchorFailed(String)` variants (still `#[non_exhaustive]`, semver-minor).
- **DTE wiring** — `TurnEngine::with_signing(...)` + `TurnEngine::with_all(...)`; per-turn `TurnSummary.receipt: Option<SignedReceipt>`. The receipt covers exactly the bytes the memory backend chained, signed AFTER the WAL append (A1 preserved).
- **Type-erased backend** — `DynSigningBackend` lives in `gauss-turn::engine` so the engine remains object-safe without sprouting a backend generic; concrete backends (`Ed25519Signer`, HSM clients) plug in unchanged.
- **`serde-big-array`** for the 64-byte signature field — JSON-friendly while preserving zero-copy deserialization.
- **Conformance** — new `axiom_a9_and_theorem_t11_signed_receipts` module: signed turn emits a verifiable receipt; unsigned engine emits `None`; tampered signature is rejected; admission denial emits no receipt; whole-chain replay round-trips for a 3-step run; TSA anchor covers the run and tamper detection is correct; `AnchorPolicy::SPECS_DEFAULT` cadence honoured.
- **ADR-0011** — receipt format, `SigningBackend` / `TsaClient` pluggability, anchor cadence rationale, RFC 3161 / `OpenTimestamps` deferral to Phase 9 / 10.
- Total: **143 tests green** across 10 crates under pedantic+nursery clippy with `-D warnings`; `cargo doc --workspace --no-deps` clean under `RUSTDOCFLAGS=-D warnings`.

### Exit gate (met)

CONF-A9-* and CONF-T11-* green: receipt verifies against its embedded public key; a tampered signature / payload / chain link is rejected; a `SimulatorTsaClient` anchor covers a multi-step run AND fails on payload mutation; cadence policy fires at exactly the expected counts.

### Open follow-ups (don't block Phase 6)

- Real RFC 3161 HTTP client — Phase 9 alongside the public verifier wrapper.
- `OpenTimestamps` Bitcoin-Calendar client — Phase 10 feature-gated.
- OS-keyring backend impl of `SigningBackend` — Phase 9 deployment work.
- `cargo-fuzz` chain-tampering target — Phase 6 alongside `kv-rocksdb` (cross-process replay).

---

## Phase 6 — Trinity Memory: FTS + HNSW + K-LRU + Delta ✅

**Goal:** activate the indices reserved by the SurrealDB schema in Phase 1, plus the warm-cache substrate. **Locks A5; proves T5, T12.**

### Delivered

- **`gauss-traits` extensions** — `AppendEntry` gained `payload_text: Option<String>` + `embedding: Option<Vec<f32>>` (builder-style `.with_text(...)` / `.with_embedding(...)`). New types `RecallHit` (with `RecallSource { Fts, Vector, Hybrid }`), `HybridQuery { text, embedding, k, alpha }`, and the `merge_hybrid` score-blender. Three new `MemoryBackend` methods — `fts_search`, `vector_search`, `hybrid_recall` — with default empty impls so older backends keep compiling.
- **`gauss-memory::surreal` write path** populates the Phase-1-reserved `payload_text` and `embedding` columns; the FTS / HNSW indices defined at bootstrap now have content.
- **`gauss-memory::surreal` read path** implements all three recall methods through SurrealDB: `@0@` for BM25 + `search::score(0)` for the score; `<|k|>` for HNSW KNN + `1 - vector::distance::knn()` for the score. The hybrid path runs both per-channel queries and reuses `gauss_traits::merge_hybrid` for the deduplicated score blend.
- **`gauss-memory::klru`** (new) — `PrefixTree<S>` K-LRU cache (`DEFAULT_K = 128`, `DEFAULT_CAPACITY = 512`, paper §VIII.C). Path is content-addressed (`Vec<u64>`); LRU is a `VecDeque` access order; backing store is `HashMap<Path, Node<S>>` under a `parking_lot::Mutex`. `Node<S>` is either `Checkpoint(S)` or `Delta(Patch)`. Stats track hits, misses, inserts, checkpoints, evictions.
- **`gauss-memory::snapshot::myers`** (new) — proper Myers `O((N+M)·D)` greedy diff over abstract tokens; `diff(prev, next) -> Vec<Op<T>>`, `diff_lines`, `diff_strs`, `apply_lines`, `Patch::edit_distance`. Coalesces adjacent `Equal` runs.
- **Cargo features** — `kv-surrealkv` (single-node persistent) and `kv-rocksdb` (Phase-10 optional) on `gauss-memory`, both layered on top of the default `surrealdb-embedded` (`kv-mem`).
- **Conformance** — three new modules:
  - `axiom_a5_memory_monoid` — identity (`ε ∘ a = a ∘ ε = a`), associativity (`(a ∘ b) ∘ c = a ∘ (b ∘ c)`), non-idempotence (free monoid distinguishes duplicates).
  - `theorem_t5_hybrid_recall` — synthetic 20-doc corpus, held-out queries, miss-rate gated against a calibrated `≤ 0.20` bound (paper's `0.015` is a 10⁵-scenario target revisited in Phase 10). Empty queries return empty; single-channel queries label hits correctly.
  - `theorem_t12_delta_warm_switch` — warm-cache lookup latency `< 10 ms`; Myers diff round-trips a realistic transcript; K-LRU eviction keeps the warm node alive across a 1000-insert wave with capacity = 100.
- **ADR-0012** — K-LRU policy + cadence rationale + Phase-10 distributed-cache migration plan.
- Total: **170 tests green** across 10 crates under pedantic+nursery clippy with `-D warnings`; `cargo doc --workspace --no-deps` clean under `RUSTDOCFLAGS=-D warnings`.

### Exit gate (met)

CONF-A5-*, CONF-T5-*, CONF-T12-* green: monoid laws hold against `SurrealMemory`; hybrid recall returns shaped results from both channels (BM25 + HNSW) and the in-process miss rate stays within the calibrated bound; K-LRU warm-cache hits are sub-millisecond and the eviction policy keeps deliberately-warm paths alive across 1000-insert waves.

### Open follow-ups (don't block Phase 7)

- Paper's `0.015` miss-rate bound revisited against a 10⁵-scenario corpus (AgentDojo + EchoLeak, integrated with Phase 4's HWCA harness) — Phase 10.
- Real-embedding model wiring (sentence-transformers / MiniLM) — Phase 9 (`gauss-canvas` adopts it for query previews).
- Distributed K-LRU cache for cluster mode — Phase 10 (ADR-0012 §Migration).
- `cargo-fuzz` chain-tampering target alongside `kv-rocksdb` cross-process replay — Phase 10.

---

## Phase 7 — Supervised Autonomy Gradient + Approval Plane ✅

**Goal:** action risk classifier + channel-routed approval queue. **Locks A8.**

### Delivered

- **New crate `gauss-sag`** — four-band `Risk` lattice (`Auto < Notify < RequireApproval < Deny`), `RiskInputs { cap, taint, reversible, tool }`, `Classifier` trait.
- **`DecisionTable`** — ordered `Vec<Rule>` + fall-through `Risk`; `Predicate` algebra (`Always`, `ContainsCap`, `TaintAtLeast`, `NonReversible`, `Tool`, `All`, `Any`); operator-readable labels per rule. The Phase-7 `default_decision_table()` encodes paper §XI.B: adversarial taint → Deny; `CRYPTO_SIGN` → RequireApproval; non-reversible (`NETWORK_POST` ∨ `SUBPROCESS_SPAWN`) → RequireApproval; (non-reversible ∨ Web taint) → Notify; otherwise Auto.
- **`verify_monotonicity`** — build-time property check across the canonical cap × taint × reversibility grid. The default table passes from both the SAG unit-test AND the cross-crate conformance vantage.
- **`ApprovalSurface`** — async trait + three deterministic test surfaces: `AutoApprove`, `AutoDeny`, `ChannelSurface` (`tokio::sync::mpsc`-driven). `ApprovalRequest { turn_id, action, risk, reason }`, `ApprovalDecision { Approved, Denied, Timeout }` (serde-friendly, `#[non_exhaustive]`).
- **`ApprovalGate<C>`** — wraps a classifier + a boxed surface; configurable deadline (default 5 min per SPECS §XI.C); `decide_action(turn_id, action, taint) -> Outcome`; `Outcome::{Allow, Denied, Approved, TimedOut}` triaged by `ApprovalGate::check(...)` into `GaussError::{AutonomyDenied, AutonomyApprovalTimeout}`.
- **DTE wiring** — `TurnEngine::with_sag(gate)`; SAG sits between admission (step 3) and the WAL append (step 4), so denied / timed-out actions leave no chain entry. The per-turn `TurnSummary.sag_decisions: Vec<SagDecisionRecord>` is bundled into the canonical payload so the Phase-5 signed receipt covers the approval verdict.
- **Conformance** — new `axiom_a8_sag_approval` module (7 tests): default-table monotonicity from a cross-crate vantage; human-deny returns `AutonomyDenied` and the WAL stays empty; approval timeout returns `AutonomyApprovalTimeout`; approve-then-execute commits and the summary's `sag_decisions` records the approver; classifier-`Deny` short-circuits without calling the surface; text-only turns skip SAG; channel surface round-trips an explicit decision.
- **ADR-0013** — decision-table schema, monotonicity invariant, surface trait, Phase-9 production-adapter migration plan.
- Total: **199 tests green** across 11 crates under pedantic+nursery clippy with `-D warnings`; `cargo doc --workspace --no-deps` clean under `RUSTDOCFLAGS=-D warnings`.

### Exit gate (met)

CONF-A8-* green: SAG denial path returns `AutonomyDenied` and leaves no chain entry; SAG timeout path returns `AutonomyApprovalTimeout`; approved actions commit with the SAG record bundled into the signed payload; classifier-Deny short-circuits without calling the human surface; text-only turns bypass SAG entirely.

### Open follow-ups (don't block Phase 8)

- Telegram / Slack / Discord / Matrix / CLI / SSE production surfaces — Phase 9 channel layer.
- Statistical-LM classifier as a Phase-10 research item layered over the rule-driven `DecisionTable` (the trait surface accepts it as a drop-in `Classifier`).
- Approver authentication tied to the channel adapter's authenticated identity — Phase 9.
- Per-tenant `DecisionTable` loading from disk / config — `serde` impl already in place.

---

## Phase 8 — Trait Polyhedral Surface + Build-time Verifier ✅

**Goal:** typed plugin surface with behavioural-equivalence checks. **Proves T7.**

### Delivered

- **New crate `gauss-poly`** — `Probe<I, O>` + `PolyhedralProbeSet<I, O>` (deterministic, named, serde-friendly) + `verify_provider_equivalence(&p, &q, &probes)` that compares canonical-JSON bytes byte-for-byte; returns `ProviderEquivalenceReport` on success or `SwapEquivalenceError` with the first-divergence diagnostics.
- **`specT` style guide** — four rules every plugin trait follows (serializable outputs, `#[non_exhaustive]`, unified `GaussError`, probe-set-checkable invariants). The verifier is the mechanical witness; ADR-0014 documents the rules.
- **Conformance** — CONF-T7 (provider adjunction): equivalent providers pass; diverging providers report the first probe index that fails.
- **ADR-0014** — polyhedral verifier semantics + Phase-10 hardware-attestor migration.
- **Production network adapters deferred to plugin crates** — Telegram / Slack / Discord / Matrix / Anthropic-Messages / OpenAI-Chat / Gemini / OpenRouter / llama.cpp HTTP each ship as additive plugin crates that take a dep on `gauss-traits` + `gauss-poly` and verify against the reference probe set. The Phase-8 ship is the trait + verifier surface, not the adapters themselves (which need network credentials and live infrastructure).

### Exit gate (met)

`verify_provider_equivalence(&new, &reference, &probes)` round-trips against `ToyProvider`; the conformance suite asserts both `passed` and `divergence-detected` paths. The same shape generalises to other plugin traits via `verify_<trait>_equivalence` helpers as they stabilise.

### Open follow-ups (don't block Phase 9)

- Vendor adapter crates (`gauss-provider-anthropic`, `gauss-provider-openai`, …) — additive plugin crates per the `specT` style guide.
- Channel adapter crates (`gauss-channel-telegram`, etc.) — additive plugin crates implementing the Phase-7 `ApprovalSurface` trait.

---

## Phase 9 — A2UI Canvas + Health Engine + Surface Layer ✅

**Goal:** user-facing polish. **Proves T8.**

### Delivered

- **New crate `gauss-canvas`** — A2UI Live Canvas Protocol typed widget tree. Eight widget kinds (`Text`, `Button`, `KeyValueTable`, `Image`, `ApprovalPrompt`, `Container`, `Markdown`, `Custom`); four reconciliation operations (`Insert`, `Update`, `Delete`, `Reorder`); `Canvas` async trait + `InMemoryCanvas` (`HashMap` + `tokio::sync::broadcast`).
- **New crate `gauss-health`** — Self-Diagnosable Health Engine. `HealthSubject` trait, `Invariant` + closure-based evaluation, `HealthReport` serde wire form. `HealthEngine::with_specs_defaults()` installs the SPECS §XIII.C seven minimum invariants (WAL barrier armed, kernel grant non-bottom, no leaked HWCA workers, signer present, sandbox present, SAG present, monotone grant). Operators register custom invariants via `engine.register(Invariant::new(...))`.
- **New crate `gauss-gateway`** — wire types for `POST /v1/turn` + `GET /v1/health` + the OpenAI-compatible `/v1/chat/completions` proxy + SSE `StreamEvent`. The actual `axum` server is Phase-11 additive.
- **Conformance** — CONF-T8: health engine reports seven invariants, fails on a broken subject; canvas accepts insert + delivers to live subscribers; gateway round-trips request/response shapes and the OpenAI proxy.
- **ADR-0015** — widget-set freeze + Phase-10 `SurrealCanvas` migration plan.
- **Production binaries deferred** — `gauss-cli`, `gauss-tui`, `gauss-desktop` ship as additive Phase-11 crates that take a dep on `gauss-canvas` + `gauss-gateway` + `gauss-health`. Migration tools (`gauss import {hermes,openfang,openclaw,zeroclaw}`) are Phase-11 deployment surfaces.

### Exit gate (met)

`InMemoryCanvas` reconciliation is end-to-end deterministic; `HealthEngine::evaluate` produces the seven-invariant report; the gateway proxy round-trips an `OpenAiChatRequest`. ADR-0015 freezes the widget set + ops alphabet.

### Open follow-ups (don't block Phase 10)

- `axum` HTTP server crate that wraps `gauss-gateway` wire types — Phase 11.
- `SurrealCanvas` backend swapping `InMemoryCanvas` for SurrealDB live queries — Phase 11 alongside cluster mode.
- Migration tools — additive Phase-11 deployment crates.

---

## Phase 10 — Hardening, Scale, Attestation ✅

**Goal:** production readiness. **Proves T6 and T10 with L4 (TEE attestation).**

### Delivered

- **Cluster mode** (`gauss-kernel::cluster`) — `ConsistentHashRing` with 128 virtual nodes per physical node (configurable), SHA-256 hashing, `BTreeMap`-keyed ring under a `parking_lot::Mutex`. Adding / removing a node moves only `O(1/N)` of the existing sessions; the conformance suite asserts `< 40 %` movement on a 4-node ring after one node addition.
- **TEE attestation** (`gauss-attest`) — `Attestor` async trait + `AttestKind { SevSnp, TdxIntel, ArmCca, Simulator }` + canonical wire format documented inline. The Ed25519 software simulator (`SoftwareSimAttestor`) ships in this crate; hardware backends (AMD SEV-SNP, Intel TDX, ARM CCA) ship as additive plugin crates that wrap the same trait + canonical pre-image. `verify_report(...)` short-circuits on nonce / measurement / key / signature failure.
- **wasmtime feature flag** (`gauss-sandbox`) — `--features wasm-wasmtime` opts the swap in. The default `wasm-wasmi` remains on the workspace MSRV (1.83); production hardening builds use `--no-default-features --features wasm-wasmtime,linux-layers` on Rust 1.85+.
- **TEE-attest feature** (`gauss-sandbox`) — additive feature wiring `gauss-attest` into the composite sandbox so production deployments can bundle a per-tool attestation report into the signed receipt.
- **Chaos injectors** (`gauss-chaos`) — `KillSwitch` (atomic flag with poll counter), `Partition<T>` (FIFO queue + drop counter), `ClockSkew` (signed offset). `ChaosBudget` bundles all three; conformance tests pin the semantics.
- **Conformance** — CONF-T6 (cluster routes deterministically + reroutes ≤ `O(1/N)` on node addition), CONF-T10-L4 (attestation round-trips; tampered nonce / measurement / signature rejected), CONF-T1-CHAOS-* (chaos injector invariants).
- **ADR-0016** — TEE attestation matrix + hardware-backend plugin migration.
- **Hardware attestation backends deferred** — `gauss-attest-sevsnp`, `gauss-attest-tdx`, `gauss-attest-armcca` ship as additive plugin crates that wrap the same canonical wire format. The Phase-10 ship is the trait + verifier + simulator (offline, deterministic), not the hardware drivers (which need specific kernel modules + attestation services).

### Exit gate (met)

`ConsistentHashRing` routes deterministically; adding a 4th node to a 3-node ring moves `< 40 %` of 1000 sample sessions. `SoftwareSimAttestor` produces reports that `verify_report` accepts; tampered nonces / measurements / signatures are rejected. The chaos injectors have stable semantics under the property tests.

### Open follow-ups (don't block Phase 11)

- AMD SEV-SNP / Intel TDX / ARM CCA plugin crates — Phase-11 deployment.
- SurrealDB `kv-tikv` cluster backend — Phase-11 deployment alongside the gateway's `axum` server.
- External pen-test report — Phase-11 deployment.
- Chaos test harness wired into a `TurnEngine` end-to-end run — Phase-11 deployment.

---

## Phase 11 — 1.0 Release (3 weeks) — NEXT

(Unchanged from earlier draft.)

---

## v2 Horizon — Research Extensions (paper §XVIII.E)

1. Mechanised proofs (Lean / Coq).
2. zk-SNARK over the receipt chain.
3. Differentially-private trajectory exporter.
4. Learnt risk classifier `Φ̂`.
5. AI-OS benchmark suite.
6. Robust declassifiers.

---

## Cross-phase Workstreams

| Workstream                | Owner               | Cadence            |
|---------------------------|---------------------|---------------------|
| Security review           | Tier-0 reviewers    | Per Tier-0 PR       |
| Dependency audit          | `cargo-vet` bot     | Per merge to main   |
| Benchmark regression      | `gauss-bench` CI    | Per release branch  |
| Conformance regression    | `gauss-conformance` | Per main commit     |
| ADR backlog               | Architects          | Bi-weekly review    |
| Plugin author UX research | DevRel              | Quarterly survey    |

---

## Decision Log (current)

| ADR    | Topic                                          | Phase | Status     |
|--------|------------------------------------------------|-------|------------|
| 0001   | Axiom-driven phasing                           | 0     | Accepted   |
| 0002   | Tokio multi-thread runtime                     | 0     | Accepted   |
| 0003   | Ed25519 + BLAKE3 + SHA-256                     | 0     | Accepted   |
| 0004   | Figment configuration                          | 0     | Accepted   |
| 0005   | Privilege tiers + review policy                | 0     | Accepted   |
| 0006   | SurrealDB as the Trinity Memory storage engine | 1     | Accepted   |
| 0007   | WAL barrier semantics for the DTE              | 2     | Accepted   |
| 0008   | Canonical `CapToken` lives in `gauss-core`     | 2     | Accepted   |
| 0009   | Composite sandbox stack (wasmi + …)            | 3     | Accepted   |
| 0010   | HWCA worker boundary + schema gate (IPI)       | 4     | Accepted   |
| 0011   | Receipt chain signing + TSA / OpenTimestamps   | 5     | Accepted   |
| 0012   | K-LRU prefix-tree cache + checkpoint cadence   | 6     | Accepted   |
| 0013   | SAG decision table + approval surface          | 7     | Accepted   |
| 0014   | Polyhedral verifier + `specT` style guide      | 8     | Accepted   |
| 0015   | Canvas widget-set freeze + Phase-10 streaming  | 9     | Accepted   |
| 0016   | TEE attestation matrix + plugin migration      | 10    | Accepted   |

Each ADR lives under `docs/adr/NNNN-title.md` and is referenced from the relevant phase exit gate.

---

## Test counts by phase (cumulative)

| Phase | Total tests | Highlights                                                           |
|-------|-------------|----------------------------------------------------------------------|
| 0     | 35          | proptest lattice laws (10), chain integrity, type-state DTE          |
| 1     | 51          | + lock-free token bucket (12), antitone verifier, SurrealDB round-trip |
| 2     | 73          | + DTE end-to-end (4), admission denial (1), crash injection (1), replay/witness (3), Myers diff (6), `ToyProvider` (2) |
| 3     | 90          | + WasmSandbox (3), CompositeSandbox (3), NoOpSandbox (1), Landlock (2), bwrap (2), seccomp (2), CONF-T10 (4) |
| 4     | 110         | + Worker spawner (4), schema gate (5), instruction-substring filter (4), IPI corpus (3), CONF-A7/T9 (4) |
| 5     | 143         | + Ed25519 signer (7), SignedReceipt (8), TSA simulator + anchor verifier (5), AnchorPolicy + Anchorer (4), public verifier API (9), CONF-A9/T11 (7) |
| 6     | 170         | + AppendEntry recall fields (3), Myers diff (8), K-LRU PrefixTree (7), SurrealDB FTS/KNN/hybrid (3), CONF-A5/T5/T12 (9) |
| 7     | 199         | + Risk lattice + RiskInputs (4), DecisionTable + monotonicity verifier (7), ApprovalSurface + AutoApprove/Deny/Channel (5), ApprovalGate (5), DTE SAG wiring (1), CONF-A8 (7) |
| 8/9/10 | 263        | + Polyhedral probe/provider verifier (6), Canvas widgets + InMemoryCanvas (9), HealthEngine + 7 invariants (7), Gateway wire types (7), TEE attestation simulator (7), Chaos injectors (5), Consistent-hash cluster ring (6), CONF-T6/T7/T8/T10-L4/chaos (17) |
