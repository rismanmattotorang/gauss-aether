# Gauss-Agent0 Integration Strategy

> **Companion to:** `Gauss-Agent0-PaperV1.0.pdf` (Gaussian Technologies R&D),
> `gauss-aether/SPECS.md`, `gauss-aether/ROADMAP.md`, and the GaussClaw
> `STRATEGY.md` / `ROADMAP.md`.
>
> **Status:** Phases 0–6 landed in the `gauss-rsi` crate (deterministic,
> I/O-free engine core — 83 unit/integration tests, clippy + rustdoc clean
> under the workspace pedantic+nursery+`-D warnings` profile). The remaining
> work is *live-backend wiring* (embedded SurrealDB, live OpenRouter, Axum
> routes, Ratatui rendering, `gauss-bench` drivers), which layers additively
> over the proven core. See the per-phase ✅ markers below.

---

## 1. What the paper proposes

**Gauss-Agent0** is a framework for **weight-frozen recursive
self-improvement (RSI)**. Instead of fine-tuning model weights (expensive,
unauditable, contamination-prone, hard to roll back), capability accrues in
an **external, verifiable knowledge-and-skill state** `x = (K, S)` composed
from a pool of frozen frontier LLMs reached through a mixture-of-experts
router.

One improvement cycle is an operator **Φ** on states. The paper proves three
results under explicit, falsifiable assumptions:

| Result | Statement | System quantity it constrains |
|---|---|---|
| **Theorem 1** | Φ's gap dynamics are a Banach contraction: `E[gₜ] ≤ (1−ρ)ᵗ g₀`; converges geometrically to the verifiable composition closure with explicit cycle bound `T(ε)`. | productivity `ρ` |
| **Theorem 2** | If the synergy set `Σ` is non-empty, the composed state strictly contains the union of per-model knowledge, with `I(K⋆;T) ≥ maxᵢ I(Kᵢ;T)`. | synergy count `μ(Σ∩K_T)` |
| **Theorem 3** | A cost-aware LinUCB contextual-bandit router attains `Õ(d√T)` regret and a strictly-positive routing advantage `ΔR` under expert heterogeneity. | `ΔR`, regret |

Seven components each discharge exactly one assumption (paper Table III):
**QueryRouter** (Alg. 3, exploration floor `εₓ`, regret), **KnowledgeGraph**
(finite auditable state, provenance), **RSI Loop Engine** (Φ, convergence,
rollback), **DualRAG** (premise recall `r_L`), **CriticAgent** (uncertainty
`p̂`, drift GDI, re-audit `η`), **VerifierAgent** (soundness, completeness
`c_v`, PAC skill gate), and a **UI layer** (observability, human cadence).

Reference stack in the paper: Tokio + Axum + Ratatui, SurrealDB
(graph + HNSW vector), OpenRouter multi-provider routing, SAHOO-style
goal-drift gating with checkpointed rollback.

---

## 2. Assessment — is this high-potential for Gauss-Aether / GaussClaw?

**Verdict: yes, high potential, and unusually low integration risk.** The
paper's reference architecture is almost exactly the stack this repository
already ships. The match is component-for-component:

| Agent0 component | Reference tech | Existing crate(s) here | Gap to close |
|---|---|---|---|
| KnowledgeGraph `(K,S)` | SurrealDB graph + HNSW | **`gauss-memory`** (SurrealDB, HNSW index reserved, FTS, `RELATE` graph lineage, SHA-256 chain head) | Add `claim`/`skill`/`concept`/`model` tables + provenance + typed edges |
| DualRAG vector path | HNSW k-NN | **`gauss-memory`** (Phase-6 hybrid recall: BM25 + HNSW) | Reuse directly |
| DualRAG graph path + fusion | beam search + RRF | **`gauss-memory`** lineage + **new `gauss-rsi::fusion`** | RRF + cross-encoder rerank |
| QueryRouter (LinUCB) | cost-aware bandit | **`gaussclaw-providers-meta`** (`NotDiamondProvider`, `SelectionStrategy` trait) + **new `gauss-rsi::router`** | Plug LinUCB as a `SelectionStrategy` |
| Expert pool / OpenRouter | OpenRouter gateway | **`gauss-provider`** (OpenRouter named for Phase 8), **`gaussclaw-providers-meta::OpenRouterProvider`** | Wire pool config (Appendix C) |
| VerifierAgent (tiered) | exec/quorum/judge | **`gauss-poly`** (probe-equivalence verifier), **`gauss-exec`** (sandboxed execution), **`gauss-sandbox`** (4-layer) | Tier-1 exec + Tier-2 cross-family quorum + Tier-3 judge |
| PAC skill certification | Hoeffding bound | **`gaussclaw-skill`** (manifests), **`gauss-curator`** (consolidation) | Add `ci_low` / `m_tests` PAC gate |
| CriticAgent (GDI, `p̂`) | SAHOO drift | **`gauss-sag`** (risk lattice), **`gauss-learnt`** (learnt scorer) | Add 4-component GDI estimator |
| Checkpoint / rollback | DB snapshot | **`gauss-checkpoint`** (content-addressed, cap-separated, O(1)) | Reuse directly |
| RSI Loop Engine (Φ) | Tokio orchestration | **`gauss-turn`** (turn engine), **new `gauss-rsi`** | The operator itself |
| Audit / provenance | — (paper: DB rows) | **`gauss-audit`** (Ed25519 + Merkle + TSA) | **Superset** of the paper |
| UI (Web + TUI) | Axum + Ratatui + HTMX | **`gaussclaw-web`**, **`gaussclaw-tui`**, **`gauss-canvas`** | Add RSI dashboard panels |
| Safety gates | SAHOO thresholds | **`gauss-sag`** + **`gauss-hwca`** + **`gauss-kernel`** admit gate | Wire GDI into the gate |

**Why this is a genuine capability upgrade, not a re-skin:**

1. **It closes GaussClaw's single biggest acknowledged gap.** GaussClaw's own
   `STRATEGY.md` records `gaussclaw-skill` as *"Manifest parser only. No
   synthesise / promote loop"* and flags the "self-improving" claim as the
   one that is currently oversold. Agent0 **is** the closed synthesis loop,
   with a convergence proof and a verification gate — exactly the missing
   piece.

2. **Every Agent0 safety assumption maps onto a property this repo already
   enforces, and we strictly exceed the paper on auditability.** The paper
   stores provenance in plain SurrealDB rows; here, every admitted item can
   carry an Ed25519-signed, Merkle-chained, TSA-anchored receipt
   (`gauss-audit`). Agent0's "rollback is a DB operation" becomes
   *content-addressed, cap-separated, tamper-evident* rollback
   (`gauss-checkpoint`). Agent0's verifier becomes a verifier whose Tier-1
   execution runs inside the 4-layer sandbox (`gauss-sandbox`).

3. **The hard parts already exist and are conformance-tested.** SurrealDB
   store, HNSW recall, sandboxed execution, the router trait surface, the
   approval/risk lattice, checkpoint/rollback — all shipped (Phases 0–11 of
   the Gauss-Aether roadmap are marked done). Agent0 is mostly *orchestration
   and a handful of new deterministic algorithms* on top of shipped
   substrate, which is the cheapest possible integration profile.

4. **It composes cleanly with the existing safety story rather than
   fighting it.** The RSI operator Φ runs *through* the kernel admit gate:
   every expert call, every knowledge write, every skill execution is already
   cap-gated and taint-tracked. A self-improving loop that cannot widen its
   own capabilities is precisely the safety posture the SAHOO section of the
   paper asks for, and it is enforced here by the type system, not by policy.

**Risks / honest caveats:**

- **Verifier soundness is load-bearing** (Assumption 1 / Proposition 2). A
  bad Tier-2/3 admission propagates along `derived_from` edges. Mitigation is
  already designed: derivation-cascade quarantine + the re-audit rate `η` +
  Tier-1-first admission, plus our existing receipt chain makes any
  contaminated item *traceable*.
- **Closure may be small on open-ended domains.** Expect H to hold first on
  verification-rich domains (math, code) — exactly where Tier-1 execution +
  `gauss-exec` give us cheap ground truth.
- **Cost is the binding scale limit** (Prop. 3). The router's cost-adjusted
  reward (Eq. 4) and the live `T(ε)` forecast (Eq. 8) make spend a
  *dashboard quantity decided before, not after*, the run.

---

## 3. Phased implementation strategy

Following this repo's culture (axiom/theorem-traced phases, deterministic
no-I/O cores, conformance-gated exits). Each phase names the paper result it
discharges and the crate it lands in.

### Phase 0 — Engine foundations ✅ *(landed: `gauss-rsi` `state`/`productivity`/`converge`/`gdi`/`event`)*

**Goal:** a deterministic, I/O-free mathematical core for the RSI loop, so
every later phase wires real backends behind already-proven algorithms.

- **New crate `gauss-rsi`** (`gauss-aether/crates/gauss-rsi`):
  - `state` — the state `x = (K, S)` (Eq. 1), the gap metric `d(x,x′)` as
    symmetric-difference mass, and the Φ update (Eq. 2).
  - `productivity` — Lemma 1 factorization `ρ ≥ β·εₓ·r_L·p_g·c_v`.
  - `converge` — Theorem 1: geometric gap forecast `(1−ρ)ᵗg₀`, the cycle
    bound `T(ε)` (Eq. 8), an online EWMA `ρ̂` estimator, and the
    patience-`k` convergence detector.
  - `gdi` — the SAHOO Goal Drift Index (Eq. 17) and its `τ` gate.
  - `event` — the `CycleEvent` bus enum (paper Appendix B) for Web/TUI.
- Full unit-test coverage of the contraction bound, the metric laws, the
  productivity product, and the GDI gate. Wired into the workspace.

**Exit gate:** `cargo build/test/clippy -p gauss-rsi` green under the
workspace's pedantic+nursery+`-D warnings` profile.

### Phase 1 — Routing + retrieval-fusion algorithms ✅ *(landed: `gauss-rsi` `router`/`fusion`)*

**Goal:** the two self-contained Agent0 algorithms that need no live LLM or
DB — so they can be proven in isolation before integration.

- `gauss-rsi::router` — **Algorithm 3**: cost-aware LinUCB with
  Sherman-Morrison rank-1 updates, the `εₓ` exploration floor (deterministic,
  caller-supplied draw — matching the repo's "explicit clock" convention),
  cost-adjusted reward (Eq. 4), soft fan-out to the UCB-near-optimal set, and
  an empirical routing-advantage `Δ̂R` estimator (Eq. 13). *(Theorem 3.)*
- `gauss-rsi::fusion` — **Algorithm 2** fusion stage: reciprocal-rank fusion
  of the vector-path and graph-path rankings, premises-first packing.
  *(supplies `r_L`, Lemma 1 / Theorem 2(b).)*

**Exit gate:** property tests for regret monotonicity, `ΔR ≥ 0`, and RRF
ranking invariants.

### Phase 2 — KnowledgeGraph state ✅ *(landed: `gauss-rsi::kg` models + `SCHEMA_SURREALQL` + in-memory store; live `gauss-memory` backend pending)*

Add the Agent0 `claim` / `skill` / `concept` / `model` / `task` / `snapshot`
tables and the typed edge tables (`about`, `relates`, `supports`,
`contradicts`, `derived_from`, `evidences`, `requires`, `certified_on`,
`involves`, `emitted_by`) to the SurrealDB schema (paper Appendix A), each
claim carrying provenance + cycle index. *(Assumption 2; finite auditable
state.)* Reuse the reserved HNSW index. Rollback = `gauss-checkpoint`.

### Phase 3 — DualRAG retrieval ✅ *(landed: `gauss-rsi::dualrag` over the store + fusion; live OpenRouter/`providers-meta` wiring pending)*

`router` plugged in as a `gaussclaw-providers-meta::SelectionStrategy`;
`fusion` driving graph-path beam search + HNSW vector path over
`gauss-memory`. OpenRouter pool from Appendix C wired through
`gaussclaw-providers-meta::OpenRouterProvider`.

### Phase 4 — VerifierAgent + CriticAgent ✅ *(landed: `gauss-rsi::verify` + `gauss-rsi::critic`; Tier-1 exec on `gauss-exec`/`gauss-sandbox` pending)*

Tier-1 (exec) on `gauss-exec` inside `gauss-sandbox`; Tier-2 cross-family
quorum; Tier-3 LLM judge with capped confidence. PAC skill certification
(Eq. 11) into `gaussclaw-skill` (`pass_rate`, `ci_low`, `m_tests`).
GDI estimator into `gauss-sag` / `gauss-learnt` as a drift classifier;
re-audit stream at rate `η` (Proposition 2 quarantine).

### Phase 5 — RSI Loop Engine (Φ end-to-end) ✅ *(landed: `gauss-rsi::engine`, deterministic; async Tokio wiring + kernel-admit/`gauss-audit` receipts pending)*

`gauss-rsi::engine` iterating Algorithm 1 over Tokio: curriculum batch →
route → DualRAG → generate → critique → verify → admit/checkpoint → GDI
gate → convergence detector. Every mutating step routed through the kernel
admit gate; every admission emits a `gauss-audit` receipt. New caps
(ADR-gated): `KNOWLEDGE_WRITE`, `RSI_CYCLE_RUN`.

### Phase 6 — Surfaces + pre-registered evaluation ✅ *(landed: `gauss-rsi::eval` + `gauss-rsi::surface` DTOs/logic; Axum/Ratatui rendering + `gauss-bench` drivers pending)*

RSI dashboard panels (paper Appendix D / Table V) in `gaussclaw-tui` and the
`gaussclaw-web` dashboard; REST/WS endpoints (Appendix E) in
`gaussclaw-surfaces`. The budget-matched evaluation harness (paper §VI:
MMLU / ARC / MATH / GSM8K + the ΔK/ΔS metric, B1–B4 baselines, NOGRAPH /
NOVERIFIER / … ablations) into `gauss-bench`.

---

## 4. Component → assumption → crate traceability

This is the Agent0 "design rule" (paper Table III) re-expressed against this
repository, so every later PR can cite the row it advances:

| Agent0 component | Discharges | Lands in |
|---|---|---|
| QueryRouter | exploration floor `εₓ`; regret (Thm 3) | `gauss-rsi::router` → `gaussclaw-providers-meta` |
| KnowledgeGraph | finite auditable state (Assm 2); provenance | `gauss-memory` + `gauss-audit` |
| RSI Loop Engine | Φ; convergence (Thm 1); rollback | `gauss-rsi::engine` + `gauss-checkpoint` |
| DualRAG | premise recall `r_L` (Lemma 1) | `gauss-rsi::fusion` + `gauss-memory` |
| CriticAgent | `p̂`; GDI (Eq. 17); re-audit `η` (Prop 2) | `gauss-sag` + `gauss-learnt` |
| VerifierAgent | soundness (Assm 1); `c_v`; PAC (Prop 1) | `gauss-poly` + `gauss-exec` + `gauss-sandbox` + `gaussclaw-skill` |
| UI layer | observability; human cadence | `gaussclaw-tui` + `gaussclaw-web` + `gauss-canvas` |

---

*Phase 0 deliverable accompanies this document: the `gauss-rsi` crate with
the state operator, productivity factorization, convergence detector, GDI
gate, LinUCB router, and DualRAG fusion — all deterministic and unit-tested.*
