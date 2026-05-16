# Gauss-Aether — Rust Development Roadmap

**Companion to:** `SPECS.md`
**Strategy:** axiom-driven phased delivery — each phase locks in a coherent subset of axioms A1–A9 and theorems T1–T12, then is conformance-tested before the next phase begins.
**Cadence target:** 9 phases over ~14 months for a 1.0 release, plus a v2 horizon.

---

## Guiding Principles

1. **Axioms before features.** No phase ships a user-facing feature whose underlying axiom isn't already enforced by the kernel.
2. **Trace every commit.** Every PR must reference the axiom / theorem / SPECS section it advances.
3. **Conformance gates phase exit.** A phase ends when its conformance suite (`gauss-conformance`) is green on Tier-1 targets.
4. **Privilege escalation review.** Any code touching `gauss-kernel`, `gauss-audit`, or `gauss-attest` requires dual review (Tier-0 rules, SPECS §2).
5. **Stable trait surface from Phase 5.** Trait breaking-changes after Phase 5 require an ADR + semver-major bump.

---

## Phase Overview

| Phase | Title                                   | Duration | Axioms locked | Theorems locked | Headline deliverable                       |
|-------|------------------------------------------|----------|----------------|-----------------|--------------------------------------------|
| 0     | Foundations                              | 3 weeks  | —              | —               | Workspace, CI, ADR-0001…0005               |
| 1     | Kernel-α: capability + scheduler         | 6 weeks  | A2, A4         | T2, T4          | `gauss-kernel` with K-lattice + 3 planes   |
| 2     | Turn engine + memory log                 | 6 weeks  | A1, A3         | T1, T3          | DTE end-to-end on local toy provider       |
| 3     | Composite sandbox                        | 5 weeks  | (A2 bound)     | T10             | WASM ∧ Landlock ∧ ns/seccomp tool exec     |
| 4     | HWCA + information flow                  | 6 weeks  | A6, A7         | T9              | IPI bound `≤ 2.19%` on AgentDojo corpus    |
| 5     | Receipt chain + signatures               | 4 weeks  | A9             | T11             | Ed25519 chain + TSA anchor                 |
| 6     | Trinity memory + hybrid recall + K-LRU   | 5 weeks  | A5             | T5, T12         | Cold-start `≤ 10 ms`; recall `≤ 0.015`     |
| 7     | SAG + approval plane                     | 4 weeks  | A8             | (A8 bound)      | Approval queue on third scheduler plane    |
| 8     | Trait polyhedral surface + verifier      | 5 weeks  | —              | T7              | `cargo gauss-verify` SMT discharge         |
| 9     | A2UI Canvas + Health + surfaces          | 6 weeks  | —              | T8              | Live Canvas Protocol; `gauss doctor`       |
| 10    | Hardening, scale, attestation            | 6 weeks  | (V predicate)  | T6, T10 (L4)    | Θ(N) cluster mode; SEV-SNP/TDX attest      |
| 11    | 1.0 release                              | 3 weeks  | All            | All             | Pareto-dominance scorecard regression-pinned |
| v2    | zk audit, learnt Φ, DP exporter          | TBD      | —              | —               | Future-work line from paper §XVIII.E       |

Total to 1.0: **~14 months** assuming 4–6 engineers from Phase 2.

---

## Phase 0 — Foundations (3 weeks)

**Goal:** make it possible to develop the kernel without fighting tooling.

### Workstreams

- **Repo scaffolding.** Cargo workspace per SPECS §2; `rust-toolchain.toml` pinned to 1.83 stable; MSRV CI job.
- **CI.** GitHub Actions: `fmt`, `clippy -D warnings`, `test`, `cargo-deny`, `cargo-audit`, `cargo-vet`, MSRV check, Linux + macOS matrix.
- **ADRs.**
  - ADR-0001 — Adopt axiom-driven phasing.
  - ADR-0002 — Async runtime = Tokio (multi-thread); rationale.
  - ADR-0003 — Receipt scheme = Ed25519 + BLAKE3 record + SHA-256 chain.
  - ADR-0004 — Configuration via `figment` (TOML + env + CLI).
  - ADR-0005 — Privilege tiers and review policy.
- **Skeleton crates.** Empty `gauss-core`, `gauss-kernel`, `gauss-turn`, `gauss-memory`, `gauss-audit`, `gauss-conformance` with public types stubbed and `unimplemented!()` so the workspace compiles.
- **Documentation.** Render `SPECS.md` and `ROADMAP.md` via `mdbook` to a static site.

### Exit gate

`cargo build --workspace` green; `cargo test --workspace` runs zero tests successfully; CI matrix passes; ADR-0001 through 0005 merged.

### Risks

- Underestimating Z3 / proof-tool setup later → mitigated by spike in Phase 0 to confirm `z3` Rust bindings compile on Tier-1 targets.

---

## Phase 1 — Kernel-α: Capability + Scheduler (6 weeks)

**Goal:** privileged authority that grants/denies capabilities and dispatches across three planes. **Locks A2, A4; proves T2, T4.**

### Deliverables

- `gauss-kernel::cap` — `CapLattice` with meet, join (admin-only), `⪯`. Default cap namespace per SPECS §4.1.
- `gauss-kernel::sched::planes` — three independent token buckets (Conversation, Daemon, Approval). Lock-free implementation.
- `gauss-kernel::flow` — `TaintLattice` type (impl deferred to Phase 4); type-checked declass signature only.
- `gauss-traits::Kernel` — public surface re-exported.
- Property tests: lattice laws (associativity, commutativity, absorption), monotonicity of `reserve`, starvation freedom under cross-plane saturation.
- Crash-injection harness for `reserve` (no half-issued capabilities).

### Conformance checks introduced

- CONF-A2-* (capability monotonicity, non-interference of disjoint caps).
- CONF-A4-* (starvation freedom `≤ B/ρ` per plane under saturation).

### Exit gate

All CONF-A2-* and CONF-A4-* green; benchmark of `reserve()` ≤ 1 µs p99 on Tier-1 hardware.

### Risks

- Defining the *initial* cap namespace too narrowly → ADR-0006 must enumerate canonical caps before code freeze for Phase 2.

---

## Phase 2 — Turn Engine + Memory Log (6 weeks)

**Goal:** end-to-end turn execution with WAL-before-effect and a tamper-evident hash chain. **Locks A1, A3; proves T1, T3.**

### Deliverables

- `gauss-turn::engine` — Algorithm 1 of the paper, *minus* HWCA and signed receipts (stubs in their slots).
- `gauss-memory::log` — append-only WAL on SQLite via `sqlx`; row schema per SPECS §8.1.
- `gauss-memory::snapshot` — Myers diff (initially over plain string transcripts; ADT diff deferred to Phase 6).
- `gauss-audit::chain` — SHA-256 chain over raw records (un-signed; signatures added Phase 5).
- A *toy provider* (`gauss-provider::toy`) returning canned responses; lets the engine run without external dependencies.
- Crash-injection test harness (`kill -9` mid-turn) asserting post-recovery state ∈ {s, s′}.
- Hash-collision-bound fuzz target on the chain.

### Conformance checks introduced

- CONF-A1-* (idempotency, crash atomicity).
- CONF-A3-* (chain tamper-evidence).

### Exit gate

End-to-end demo: CLI sends prompt → toy provider responds → record + chain head visible via `gauss-audit` HTTP API; crash test passes 1000 iterations.

### Risks

- WAL semantics under cloud filesystems (NFS, gVisor) — document supported FS in ADR-0007.

---

## Phase 3 — Composite Sandbox (5 weeks)

**Goal:** tool execution under multiple orthogonal sandboxes. **Proves T10 (3-layer first; L4 deferred to Phase 10).**

### Deliverables

- `gauss-sandbox::wasm` — wasmtime integration, fuel + epoch interruption.
- `gauss-sandbox::landlock` — Linux 5.13+ ruleset builder; mandatory scoped filesystem access.
- `gauss-sandbox::seccomp` — `libseccomp-rs` filters per tool manifest.
- `gauss-sandbox::bwrap` — bubblewrap wrapper for namespace isolation.
- `gauss-sandbox::seatbelt` — macOS `sandbox-exec` profile generator.
- Cap → minimum sandbox class function per SPECS §7.1.
- Per-layer bypass test harness (each layer individually attacked; product bound asserted).

### Conformance checks introduced

- CONF-T10-* (composite bound with `p_T = 1`, i.e. software-only).

### Exit gate

A *real* tool (HTTP `fetch_url`) runs under all three Linux layers; bypass attempts logged at each layer; bench shows ≤ 3 ms composition overhead.

### Risks

- Landlock support varies by distro kernel version → ADR-0008 sets minimum kernel and gracefully degrades on older.

---

## Phase 4 — HWCA + Information Flow (6 weeks)

**Goal:** isolate every tool invocation in a worker context; propagate taint. **Locks A6, A7; proves T9 (IPI bound).**

### Deliverables

- `gauss-hwca::worker` — spawn-per-call worker with schema gate (JSON Schema 2020-12 via `jsonschema`).
- `gauss-kernel::flow::TaintLattice` — full implementation: total chain `Trusted ≤ User ≤ Web ≤ Adversarial`.
- `declass : L → K` configurable per tenant; build-time antitone check.
- Statistical-filter guard for instruction-substring detection in free-text fields.
- Recursion-depth bound (default 8) with explicit overflow handling.
- AgentDojo + EchoLeak corpus harness in `gauss-conformance` (IPI bound `≤ 2.19%`).

### Conformance checks introduced

- CONF-A6-*, CONF-A7-*, CONF-T9-* (IPI corpus).

### Exit gate

IPI corpus run: success rate ≤ 2.19%; no parent-context contamination across 10⁵ tool invocations.

### Risks

- Free-text fields legitimately needed by `sendEmail`-style tools → SAG (Phase 7) must require approval for any free-text-bound action with taint ≥ Web.

---

## Phase 5 — Receipt Chain + Signatures (4 weeks)

**Goal:** every action emits a signed, chained receipt with TSA anchor. **Locks A9; proves T11.**

### Deliverables

- `gauss-audit::sign` — Ed25519 via `ed25519-dalek` v2; key storage via OS keyring + optional HSM trait.
- `gauss-audit::tsa` — RFC 3161 client; OpenTimestamps fallback.
- Anchoring cadence configurable per tenant; default 1000 receipts.
- Public verifier API (HTTP) per SPECS §9.3.
- EUF-CMA test-vector pack; chain-tampering fuzz target.

### Conformance checks introduced

- CONF-A9-*, CONF-T11-* (forgery negl(λ), chain tamper bound `n·2^{-λ+1}`).

### Exit gate

Regulator-style audit demo: presented `(ρ, c_prev, c_next, tsa_token)`, third-party verifier accepts; tamper attempt detected.

### Risks

- TSA availability for offline / air-gapped deployments → support OpenTimestamps + internal-only anchoring with documented trust trade-off.

---

## Phase 6 — Trinity Memory: FTS + HNSW + K-LRU + Delta (5 weeks)

**Goal:** full memory substrate with hybrid recall and warm-cache fast switch. **Locks A5; proves T5, T12.**

### Deliverables

- `gauss-memory::fts` — `tantivy` 0.22+ incremental index per turn.
- `gauss-memory::vec` — `hnsw_rs` HNSW (M=16, ef_construction=200); pluggable embedding trait.
- `gauss-memory::klru` — K-LRU radix prefix tree; checkpoint every K=128 turns.
- `gauss-memory::hybrid` — `ρ_hyb = ρ_fts ∪ ρ_vec`; benchmark recall on labelled corpus.
- Cold-start bench harness; target ≤ 10 ms p95.
- Postgres backend behind a feature flag.

### Conformance checks introduced

- CONF-A5-*, CONF-T5-* (recall bound), CONF-T12-* (warm/cold separation).

### Exit gate

Recall miss ≤ 0.015 on benchmark corpus; cold-start ≤ 10 ms warm-cache p95 in `gauss-bench`.

### Risks

- Embedding cost variability across providers → cache embeddings keyed by content hash.

---

## Phase 7 — Supervised Autonomy Gradient + Approval Plane (4 weeks)

**Goal:** action risk classifier + channel-routed approval queue. **Locks A8.**

### Deliverables

- `gauss-sag::classify` — decision table per tenant; build-time monotonicity check.
- Approval-plane integration with `gauss-kernel::sched` (already three-plane since Phase 1).
- Approval surfaces:
  - Telegram inline-keyboard.
  - Slack interactive message.
  - Discord buttons.
  - CLI / TUI blocking prompt.
  - SSE web widget (placeholder; full Canvas in Phase 9).
- Approval responses are themselves signed receipts joined to the chain.
- Default 5-minute deadline; deny-on-timeout.

### Conformance checks introduced

- CONF-A8-* (monotone Φ; approval persistence; timeout behaviour).

### Exit gate

Demo: tool with `reversible = false` triggers approval; user denies via Telegram inline-keyboard; chain shows approval receipt; tool not executed.

### Risks

- Telegram/Slack bot lifecycle (token rotation, webhook reconnection) → use Phase 9 health invariants `ι_chan`.

---

## Phase 8 — Trait Polyhedral Surface + Build-time Verifier (5 weeks)

**Goal:** typed plugin surface with behavioural-equivalence checks. **Proves T7.**

### Deliverables

- Public traits frozen and documented: `ProviderTrait`, `ChannelTrait`, `ToolTrait`, `SandboxTrait`, `MemoryTrait`, `VoiceTrait`, `ApprovalTrait`, `CanvasTrait`.
- `gauss-poly` build-time verifier (`cargo gauss-verify`):
  - Each trait ships a relational spec `specT`.
  - Z3-discharged checks per impl.
  - Provider adjunction `τ ∘ σ = id` property-tested on corpus.
- Provider adapters: Anthropic Messages, OpenAI Chat, OpenAI Responses, Google Gemini, OpenRouter, local-Llama via `llama.cpp` HTTP.
- Channel adapters: Telegram, Discord, Slack, Matrix, IMAP, Signal (initial 6; more later).

### Conformance checks introduced

- CONF-T7-* (provider switch yields semantically equivalent output on benchmark prompts).

### Exit gate

Swap provider Anthropic ↔ OpenAI on a running deployment with no code change; verifier passes; benchmark suite shows ≤ 5% behavioural divergence.

### Risks

- Spec authoring is the bottleneck → start with minimal spec per trait (input parsability, output well-formedness) and tighten later.

---

## Phase 9 — A2UI Canvas + Health Engine + Surface Layer (6 weeks)

**Goal:** user-facing polish. **Proves T8 (Pareto-dominance against baselines on the fifteen-axis scorecard).**

### Deliverables

- `gauss-canvas` — A2UI Live Canvas Protocol server (JSON-RPC over WS/SSE).
  - Core widget registry (paper Table IX).
  - Capability gating per widget class.
  - Reference web client (Tauri + Tailwind).
- `gauss-health` — SDHE with the seven minimum invariants (paper Table X) and self-repair catalogue (Table XI).
- `gauss-gateway` — REST/WS/SSE, OpenAI-compatible proxy, ACP (JSON-RPC) for IDE integrations (Zed, Helix, Neovim).
- `gauss-cli`, `gauss-tui`, `gauss-desktop` (Tauri shell).
- Migration tools: `gauss import hermes`, `gauss import openfang`, `gauss import openclaw`, `gauss import zeroclaw` (paper §XVIII.C).
- Fifteen-axis scorecard regression test in `gauss-bench`.

### Conformance checks introduced

- CONF-T8-* (scorecard score ≥ 15.0; strict ≥ on every axis vs. each baseline).

### Exit gate

Demo: agent renders a Live Canvas table + approval widget; `gauss doctor` prints all green; scorecard ≥ each predecessor on every axis.

### Risks

- Canvas widget extensibility surface large → freeze a *core* set in 1.0; namespaced extensions in v1.1.

---

## Phase 10 — Hardening, Scale, Attestation (6 weeks)

**Goal:** production readiness. **Proves T6 (Θ(N) scale) and T10 with L4 (TEE attestation).**

### Deliverables

- Cluster mode: consistent-hash routing on `SessionId`; sticky workers within a turn but stateless across turns.
- Postgres backend promoted to default for clustered deployments.
- TEE attestation:
  - AMD SEV-SNP via `sev` crate.
  - Intel TDX via `tdx-guest`.
  - ARM CCA stub (hardware availability dependent).
- `V(s) = 1` gating on `A_high` turns.
- Composite-sandbox release-gate bound `≤ 1.1 × 10⁻⁷` with TEE.
- Chaos testing: random kernel-node kill, network partitions, clock skew.
- Security review (external).

### Conformance checks introduced

- CONF-T6-* (linear scale to 8 nodes, ≤ 10% efficiency loss vs. ideal).
- CONF-T10-* (TEE-bound regression).

### Exit gate

External pen-test report; chaos suite green; bench scale demonstrates Θ(N).

### Risks

- TEE availability in CI → use SEV-SNP-capable cloud runners + a "software-only" matrix lane that asserts the relaxed bound.

---

## Phase 11 — 1.0 Release (3 weeks)

**Goal:** ship.

### Activities

- Documentation freeze: SPECS.md, ROADMAP.md, user guide, operator guide, plugin author guide, audit-verifier guide.
- Release artifacts:
  - Static musl binaries for `linux-x86_64`, `linux-aarch64`.
  - Container image (distroless).
  - Tauri desktop builds for macOS / Windows / Linux.
- SBOM + SLSA L3 provenance.
- Public verifier reference implementation (read-only client, no kernel deps).
- Migration playbooks from Hermes, OpenFang, OpenClaw, ZeroClaw.
- Announcement: blog post, paper companion, recorded walkthrough.

### Release gates (all from SPECS §14.3)

- IPI ≤ 2.19%; cold-start ≤ 10 ms; sandbox ≤ 1.1·10⁻⁷ (TEE) / ≤ 10⁻⁹ (sw); recall miss ≤ 0.015; receipt forgery negl(λ); approval bounded; zero record loss on SIGKILL.

---

## v2 Horizon — Research Extensions (paper §XVIII.E)

Out-of-roadmap-band initiatives, sequenced after 1.0:

1. **Mechanised proofs.** Lean 4 or Coq formalisation of Axioms A1–A9 and Theorems T1–T12; mechanise T9 first (most tractable).
2. **zk-SNARK over the receipt chain.** Proof-of-inclusion without revealing receipt contents; target ~ms verification at 10⁴ receipts.
3. **Differentially-private trajectory exporter.** (ε, δ)-DP SFT/DPO/RL exports; cross-org collaborative training without leaking user data.
4. **Learnt risk classifier `Φ̂`.** Per-tenant supervised learning from historical approval decisions, constrained to remain monotone w.r.t. `⊑risk`.
5. **AI-OS benchmark suite.** Standardised benchmarks for IPI, capability escalation, audit forgery, plane starvation, crash recovery — publish for community comparison.
6. **Robust declassifiers.** Theory and tooling for safe `declass` maps that admit useful tools without weakening T9 in practice.

---

## Cross-phase Workstreams

These run continuously, not gated to a single phase.

| Workstream                | Owner               | Cadence            |
|---------------------------|---------------------|---------------------|
| Security review           | Tier-0 reviewers    | Per Tier-0 PR       |
| Dependency audit          | `cargo-vet` bot     | Per merge to main   |
| Benchmark regression      | `gauss-bench` CI    | Per release branch  |
| Conformance regression    | `gauss-conformance` | Per main commit     |
| ADR backlog               | Architects          | Bi-weekly review    |
| Plugin author UX research | DevRel              | Quarterly survey    |

---

## Staffing Sketch (for capacity planning)

Indicative; adjust to your org.

| Role                      | Phases 0–2 | Phases 3–6 | Phases 7–9 | Phases 10–11 |
|---------------------------|-----------:|-----------:|-----------:|-------------:|
| Kernel / privileged       | 2          | 2          | 2          | 2            |
| Runtime / turn engine     | 1          | 1          | 1          | 1            |
| Sandbox / OS integration  | 0.5        | 2          | 1          | 1            |
| Memory / indexing         | 0          | 1.5        | 1          | 0.5          |
| Crypto / audit            | 0.5        | 1          | 1          | 0.5          |
| Surfaces / UI / Canvas    | 0          | 0.5        | 2          | 1            |
| Plugins / providers       | 0          | 1          | 2          | 1            |
| DevOps / release          | 1          | 1          | 1          | 2            |
| Security / pen-test       | 0          | 0.25       | 0.5        | 1            |

---

## Decision Log (seed)

| ADR    | Topic                                  | Phase |
|--------|----------------------------------------|-------|
| 0001   | Axiom-driven phasing                   | 0     |
| 0002   | Tokio multi-thread runtime             | 0     |
| 0003   | Ed25519 + BLAKE3 + SHA-256             | 0     |
| 0004   | Figment configuration                  | 0     |
| 0005   | Privilege tiers + review policy        | 0     |
| 0006   | Canonical capability namespace         | 1→2   |
| 0007   | WAL fsync semantics + supported FS     | 2     |
| 0008   | Minimum Linux kernel for Landlock      | 3     |
| 0009   | Taint lattice initial shape            | 4     |
| 0010   | TSA + OpenTimestamps anchoring policy  | 5     |
| 0011   | K-LRU eviction policy + checkpoint K   | 6     |
| 0012   | SAG decision-table schema              | 7     |
| 0013   | Trait `specT` style guide              | 8     |
| 0014   | Canvas core widget set freeze for 1.0  | 9     |
| 0015   | TEE attestation matrix for 1.0         | 10    |

Each ADR lives under `docs/adr/NNNN-title.md` and is referenced from the relevant phase exit gate.
