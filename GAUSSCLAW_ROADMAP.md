# GaussClaw — Hermes-to-Rust Porting Roadmap

**Companion to:** `GaussClaw.pdf`, `Gauss-Aether.pdf`, `SPECS.md`, `ROADMAP.md`
**Mission:** Port every Python module of [Hermes](https://github.com/nousresearch/hermes-agent) into a single Rust binary `gaussclaw` that runs on the Gauss-Aether kernel, **preserving Hermes surface convergence and SFT/DPO export semantics bit-for-bit** while closing the five Hermes architectural deficits with kernel-grade discipline (capability, accountability, information-flow, isolation, fairness).
**Cadence target:** 5 phases / 24 weeks / 4 milestones + GA — mirroring `GaussClaw.pdf` §V and operationalised against the existing Gauss-Aether 1.0 crate surface.

---

## Binding Constraints

These are **non-negotiable** for the duration of the port:

1. **Surface-Convergence Preservation (Principle 1).** For every Hermes surface σᵢ and every well-formed message *m*, the resulting GaussClaw trajectory record must contain — modulo cryptographic envelope — the identical conversation state, lineage edge, and provider response as the corresponding Hermes record.
2. **Trajectory schema bit-equality.** The SFT/DPO JSONL wire shape (`prompt`, `completion`, `surface`, `session_id`, `parent_id`, `ts`, lineage edges, DPO pairs) is preserved field-for-field. New material is *appended* in an optional envelope, never inlined into existing fields.
3. **Decorator ergonomics.** The Hermes `@tool` authoring surface survives literally. The Skill Manifest is a non-breaking *addition*.
4. **TOML config compatibility.** A Hermes deployment's top-level config keys continue to work. New keys (caps, taint, fallback chains, meta-routers) are *optional*.
5. **No axiom regression.** No PR ships that breaks any A1–A9 / T1–T12 conformance test already green in `gauss-conformance`.

Every PR must reference (a) the Hermes module being ported, (b) the Gauss-Aether axiom/theorem it lands under, and (c) the GaussClaw phase milestone (M1–M4/GA).

---

## Existing Substrate (already shipped in Gauss-Aether 1.0)

The Rust runtime under GaussClaw is **already built**. The port reuses these crates verbatim:

| Concern | Crate | Status |
|---|---|---|
| Capability lattice 𝒦, info-flow lattice ℒ, joint admit | `gauss-kernel`, `gauss-core` | ✅ Phase 1 |
| Differential Turn Engine, WAL-before-effect | `gauss-turn` | ✅ Phase 2 |
| Trinity Memory (SurrealDB: KV + doc + graph + FTS + HNSW + Merkle) | `gauss-memory` | ✅ Phase 1, 6 |
| Composite Sandbox (WASM + Landlock + bwrap + seccomp + Seatbelt + TEE sim) | `gauss-sandbox` | ✅ Phase 3, 10 |
| HWCA worker + schema gate | `gauss-hwca` | ✅ Phase 4 |
| Receipt chain (Ed25519, BLAKE3, TSA anchor) | `gauss-audit`, `gauss-attest` | ✅ Phase 5 |
| Three-plane scheduler (conv / daemon / approval) | `gauss-kernel::sched` | ✅ Phase 1 |
| Provider trait + polyhedral equivalence verifier | `gauss-provider`, `gauss-poly` | ✅ Phase 2, 8 |
| SAG / approval plane | `gauss-sag` | ✅ Phase 7 |
| Gateway wire types, A2UI Canvas, Health (SDHE) | `gauss-gateway`, `gauss-canvas`, `gauss-health` | ✅ Phase 9 |
| Chaos injectors, scale ring, benches | `gauss-chaos`, `gauss-bench`, `gauss-robust` | ✅ Phase 10 |
| zk / DP / learnt-Φ research vehicles | `gauss-zk`, `gauss-dp`, `gauss-learnt` | ✅ Phase 11 |

**GaussClaw introduces a new family of `gaussclaw-*` crates that sit on top of these traits** — it does *not* re-derive runtime primitives.

---

## New Crate Layout

The port adds the following workspace members. Each crate is small, single-responsibility, and binds to existing `gauss-*` traits.

```
crates/
├── gaussclaw-agent          # Hermes AIAgent.run_conversation → DTE turn policy
├── gaussclaw-surfaces       # CLI · TUI · REST · WS · OAI-compat relay (ChannelTrait impls)
├── gaussclaw-channels       # ~20 messaging adapters (Slack, Discord, Matrix, …)
├── gaussclaw-providers      # 20 leaf vendor drivers (Anthropic, OpenAI, Google, …)
├── gaussclaw-providers-meta # OpenRouter (aggregator) + NotDiamond (learned router)
├── gaussclaw-api-modes      # chat-completion · responses · openai-compat shims
├── gaussclaw-tools          # First-party tools: web_search, file_*, shell, …
├── gaussclaw-skill          # Skill Manifest parser + #[tool] proc-macro
├── gaussclaw-store          # Hermes session/lineage schema atop SurrealDB
├── gaussclaw-export         # SFT / DPO writer + Cryptographic Envelope + Taint Filter
├── gaussclaw-fed            # Federated Trajectory Pool client + reference server
├── gaussclaw-config         # Hermes-compatible TOML (figment) loader
├── gaussclaw-migrate        # `gaussclaw import hermes ./hermes-config.toml`
├── gaussclaw-conformance    # Hermes-parity test suite (1,000-turn corpus, OAI SDK)
└── gaussclaw-bin            # The shipping binary; wires all of the above
```

---

## Phase Overview

| Phase | Weeks | Title | Milestone | Headline Deliverable |
|-------|-------|-------|-----------|----------------------|
| **P1** | 1–4   | Surface and Channel Routing | **M1** | All Hermes surfaces re-routed through Gauss Gateway in shim regime; 1,000-turn byte-identical replay |
| **P2** | 4–10  | Memory, Receipts, and Lineage | **M2** | SQLite/FTS5 → Trinity over SurrealDB; Ed25519 chain; 2-week dual-write parity |
| **P3** | 10–16 | Tools and Sandbox | **M3** | Every `@tool` lifted into HWCA + Composite Sandbox; IPI ≤ 2.19 %; spawn p99 ≤ 15 ms |
| **P4** | 16–20 | Provider Plane and Meta-Routers | **M4** | 20 leaf + OpenRouter + NotDiamond under polyhedral / router-transparency contracts |
| **P5** | 20–24 | Trajectory Export and GA | **GA** | Cryptographic Envelope + Taint-Aware Filter + Federated Pool + 15-axis scorecard |

Each milestone produces a shippable binary. Phase N+1 validates against Phase N's artefact.

---

## Phase 1 — Surface and Channel Routing (Weeks 1–4) → M1

**Goal.** Re-route every Hermes entry surface and channel adapter through the Gauss-Aether Gateway so that **Principle 1 holds on Day 1**. The legacy Python `run_conversation` is kept alive as a *privileged subprocess shim*; only the wrapping layer is Rust. This is the **shim regime**.

### Scope

- `surfaces.cli`, `surfaces.tui`, `surfaces.rest`, `surfaces.ws`, `surfaces.oai_compat`
- `channels.*` (Slack, Discord, Telegram, Matrix, Mattermost, IRC, XMPP, Signal, SMS, Email, Webhook, …)
- `gauss-gateway` three-plane router

### Tasks

1. **Audit Hermes adapters.** Walk `hermes/surfaces/*` and `hermes/channels/*` in the upstream repo; produce `docs/HERMES_ADAPTER_MATRIX.md` listing for each: file path, transport, message schema, auth model, expected `surface` field value.
2. **Define `ChannelTrait` impls.** For each surface, implement a thin Rust adapter in `gaussclaw-surfaces` / `gaussclaw-channels` that constructs a `gauss_gateway::Message` and routes it via the three-plane scheduler:
   - CLI / TUI / REST / WS / channels → **Conversation plane**
   - Scheduled / daemon turns → **Daemon plane**
   - Tool-call approvals → **Approval plane**
3. **Shim subprocess.** Implement `PythonShimExecutor` in `gaussclaw-agent` that fork-execs the legacy Hermes Python with stdio JSON-RPC. Every routed message becomes an RPC call; every streamed token is re-emitted on the Gauss-Aether wire.
4. **OAI-compatible relay parity.** Stand up `gaussclaw-surfaces::oai_compat` with full `/v1/chat/completions`, `/v1/completions`, `/v1/models`, and SSE streaming. Parametrise the **OpenAI Python SDK end-to-end test suite** against both Hermes and GaussClaw back-ends.
5. **Audit-trace recording.** Every adapter writes a turn-entry trace (surface, ts, headers, body hash) into `gauss-audit` *before* dispatch, so divergence can be diffed turn-by-turn.

### Crate dependency edges

```
gaussclaw-surfaces  → gauss-gateway, gauss-traits
gaussclaw-channels  → gauss-gateway, gauss-traits
gaussclaw-agent     → gauss-turn, gauss-traits (shim path: tokio::process::Command)
gaussclaw-bin       → all of the above
```

### Exit criteria (M1)

- [ ] All 20+ channels deliver a representative sample of **1,000 production turns** through GaussClaw, with output byte-identical to Hermes modulo timestamp.
- [ ] OAI-compat relay passes **100 %** of the OpenAI Python SDK's official end-to-end suite, parametrised by both backends.
- [ ] Trajectory export under shim regime produces files **byte-identical** to those produced by raw Hermes on the same input traffic (run via `gaussclaw-conformance::replay_corpus`).
- [ ] No regression in `gauss-conformance` (A1–A9, T1–T12).

### Rollback

Adapter-level kill switch in `gaussclaw.toml`:
```toml
[surfaces.rest]
backend = "shim"          # "shim" → legacy Hermes; "native" → Rust executor
```
A single config toggle returns any one surface to the legacy executor.

### Risks (cf. Table III of GaussClaw.pdf)

- *Shim RPC drift* — mitigated by JSON-schema-validated RPC envelope + nightly diff.
- *Channel auth secrets* — handled via `gauss-attest` secret store; never round-tripped to the Python shim.

---

## Phase 2 — Memory, Receipts, and Lineage (Weeks 4–10) → M2

**Goal.** Replace Hermes's `store.session` (SQLite + FTS5) and `store.lineage` (parent-pointer table) with the **Trinity Memory Substrate over SurrealDB** already provided by `gauss-memory`, signed by the Ed25519 receipt chain in `gauss-audit`. The legacy executor still drives turns; every turn is **dual-written** to SQLite and SurrealDB for a 2-week parity window.

### Scope

- `store.session` → SurrealDB `turn` document table (with vector embedding + FTS analyzer)
- `store.lineage` → SurrealDB graph edge `RELATE turn -> turn`
- New tables: `receipt`, `chain_anchor` (time-series), `fts_idx`
- Ed25519 receipt chain inside the same transaction as the turn write
- Hourly TSA anchor (OpenTimestamps by default)

### Trinity schema (SurrealQL, in `gaussclaw-store::schema.surql`)

```surql
DEFINE TABLE turn SCHEMAFULL;
DEFINE FIELD session   ON turn TYPE string;
DEFINE FIELD ts        ON turn TYPE datetime;
DEFINE FIELD surface   ON turn TYPE string;
DEFINE FIELD prompt    ON turn TYPE string;
DEFINE FIELD completion ON turn TYPE string;
DEFINE FIELD tool_calls ON turn TYPE array;
DEFINE FIELD taint     ON turn TYPE string;        -- ⊥ | user | web | adversarial
DEFINE FIELD caps_used ON turn TYPE array;
DEFINE FIELD embedding ON turn TYPE array<float>;
DEFINE FIELD cost      ON turn TYPE object;        -- {tokens, dollars, model_actual}
DEFINE INDEX fts_idx   ON turn FIELDS prompt, completion SEARCH ANALYZER ascii BM25;
DEFINE INDEX hnsw_idx  ON turn FIELDS embedding HNSW DIMENSION 384 DIST COSINE M 16 EFC 200;

DEFINE TABLE receipt SCHEMAFULL;
DEFINE FIELD turn_id   ON receipt TYPE record<turn>;
DEFINE FIELD pk        ON receipt TYPE string;
DEFINE FIELD sig       ON receipt TYPE string;
DEFINE FIELD prev_hash ON receipt TYPE string;
DEFINE FIELD self_hash ON receipt TYPE string;

DEFINE TABLE lineage TYPE RELATION FROM turn TO turn;
DEFINE FIELD signed_edge ON lineage TYPE string;

DEFINE TABLE chain_anchor SCHEMAFULL;
DEFINE FIELD head_at_ts ON chain_anchor TYPE datetime;
DEFINE FIELD head_hash  ON chain_anchor TYPE string;
DEFINE FIELD tsa_proof  ON chain_anchor TYPE bytes;
```

### Tasks

1. **Embedded deployment.** Ship the RocksDB-backed embedded SurrealDB for the `gaussclaw` CLI/TUI; validate the single-node TCP and TiKV-clustered modes against the same SurrealQL.
2. **FTS path.** Wire `store.session` FTS5 reads to SurrealDB's `@@` operator; benchmark on a 10⁵-turn corpus. Recall **must match** Hermes FTS5 on a canned query set (BM25 parity, not arithmetic identity).
3. **Vector path.** Wire HNSW recall via SurrealDB's native HNSW field type; verify the union recall bound (Theorem T5).
4. **Receipt chain integration.** `gaussclaw-store::write_turn(...)` opens a single transaction that:
   1. inserts the `turn` row,
   2. computes `self_hash = BLAKE3(prev_hash || canonical_turn_bytes)`,
   3. signs the receipt with Ed25519,
   4. inserts the `receipt` row,
   5. relates the lineage edge with a signed payload,
   6. commits.
5. **TSA anchor.** Background task in `gaussclaw-store::anchor` writes head every 1,000 receipts (or hourly, whichever first) to OpenTimestamps; result lands in `chain_anchor`. Pluggable: also CTLog/Bitcoin/RFC3161.
6. **Dual-write & diff.** During the parity window, every turn writes to both Hermes SQLite (via shim) and SurrealDB. Nightly job `gaussclaw conformance diff-stores` compares row counts, content hashes, lineage trees.
7. **Approval-plane wakeups.** Subscribe `gauss-sag` to SurrealDB `LIVE SELECT` on pending receipts so deadline elapse and operator action wake the right kernel thread.

### Crate dependency edges

```
gaussclaw-store    → gauss-memory, gauss-audit, gauss-attest, gauss-core
gaussclaw-agent    → gaussclaw-store (replaces direct SQLite calls)
gaussclaw-conformance → gaussclaw-store, gauss-memory
```

### Exit criteria (M2)

- [ ] **2 weeks of production traffic** in dual-write mode without divergence between SQLite and SurrealDB (diff job green nightly).
- [ ] Receipt chain verifies under the public verifier API with **≤ 10 Merkle-proof bytes** per verification (Corollary 4 of `Gauss-Aether.pdf`).
- [ ] **Cold-start ≤ 10 ms** time-from-receive-message to first-token-streamed for a warm session (Theorem T12).
- [ ] **Hybrid recall miss rate ≤ 0.015** on the held-out set (FTS ∪ HNSW).
- [ ] Lineage tree reconstructs identically under both layouts using SurrealDB's `FETCH` traversal vs. SQLite recursive CTE.

### Rollback

SQLite remains authoritative during the parity window. Per-namespace toggle:
```toml
[store]
authoritative = "sqlite"   # promote to "surreal" after M2 + 2 weeks
```

---

## Phase 3 — Tools and Sandbox (Weeks 10–16) → M3

**Goal.** Lift every Hermes `@tool` Python function into a **capability-gated Hierarchical Worker Context** under the Composite Sandbox. Tool raw output stays inside the worker; only a **schema-validated value** crosses back into the parent context. This is the structural cut that closes Deficits 1 and 3 and produces the IPI containment bound of Theorem T9.

### Scope

- Skill Manifest specification (TOML) + parser
- `#[tool]` proc-macro (Rust equivalent of `@tool`)
- Composite Sandbox host wiring (`gauss-sandbox` already implements layers; the new work is the per-tool manifest binding)
- Port of the first-party tool catalogue (~30 tools): `web_search`, `web_fetch`, `file_read`, `file_write`, `shell`, `python_exec`, `calendar_*`, `contacts_*`, `email_*`, `slack_post`, `git_*`, `sql_*`, etc.
- Persistent-worker optimisation for high-frequency tools

### Skill Manifest schema (canonical `skill.toml`)

```toml
name        = "web_search"
description = "Search the web via DuckDuckGo."
usage       = "Use to find recent information not in training data."

caps        = ["network:http_get:duckduckgo.com"]
taint       = "web"                 # ⊥ | user | web | adversarial
reversible  = true
persistent  = false

[cost]
tokens_per_call = 800
wallclock_ms    = 1500
dollars_per_call = 0.0

[schema]                            # JSON Schema for the value v ∈ Σₐ
type = "object"
properties = { results = { type = "array", items = { … } } }
```

### Default taint policy (declass map)

| Tool family | Default taint |
|---|---|
| `web_search`, `web_fetch`, `email_read`, `rss_read` | `web` |
| `shell`, `file_read`, `file_write`, `python_exec` | `user` |
| `calendar_read`, `contacts_read`, `git_status` | `trusted` |
| Untrusted Slack/Discord/IRC ingress | `adversarial` |

The antitone declassification map `declass : ℒ → 𝒦` is loaded from `gaussclaw.toml [taint.declass]` and verified at startup by `gauss-kernel::flow::verify_antitone`.

### Tasks

1. **Skill Manifest spec & parser.** TOML schema in `gaussclaw-skill::manifest`; serde-derived structs; figment loader; manifest-validation pass that rejects under-specified manifests at build time.
2. **`#[tool]` proc-macro.** A Hermes author writes:
   ```rust
   #[tool(
       caps  = ["network:http_get"],
       taint = "web",
       schema = WebSearchOutput,
       reversible = true,
   )]
   async fn web_search(q: String) -> Result<WebSearchOutput> { … }
   ```
   The macro generates: (a) the manifest struct, (b) a `ToolHandler` impl, (c) an `inventory::submit!` registration. Same authoring friction as `@tool`.
3. **Sandbox host wiring.** Each tool invocation:
   1. Kernel admits with `K_t ⊑ K_grant` and `taint ⊑ declass(ℓ)`.
   2. `gauss-hwca` spawns worker context `s_w`.
   3. `gauss-sandbox` enforces WASM (wasmtime, fuel+epoch interrupt) for pure-compute tools, native + Landlock/bwrap/seccomp for filesystem/network tools, namespace+seccomp + Seatbelt + AppContainer per host OS.
   4. Tool raw output stays in `s_w`. Schema validator `X_a` produces value `v ∈ Σ_a`.
   5. Only `v` crosses the boundary; the conversation buffer never sees raw bytes.
4. **First-party tool port.** Port ~30 Hermes tools in order of risk: pure-compute first (`json_*`, `math_*`), then sandboxed I/O (`file_*`, `web_*`), then privileged (`shell`, `python_exec`). Each lands behind a feature flag and a per-tool kill switch.
5. **Persistent workers.** Tools with `persistent = true` retain a worker context across calls within a turn. Spawn cost amortised to first call only.

### Crate dependency edges

```
gaussclaw-skill   → gauss-traits, gauss-core
gaussclaw-tools   → gaussclaw-skill, gauss-hwca, gauss-sandbox
gaussclaw-agent   → gaussclaw-tools (replaces Python tool dispatch)
```

### Exit criteria (M3)

- [ ] Every first-party tool runs under HWCA + Composite Sandbox with **no behavioural regression** on the regression-test corpus.
- [ ] **IPI attack success rate ≤ 2.19 %** on the held-out adversarial corpus (matching AgentSys [8]).
- [ ] **Composite sandbox compromise probability ≤ 1.1 × 10⁻⁷** (product of per-layer measured escape probabilities).
- [ ] **Tool spawn p99 ≤ 15 ms** (cf. ZeroClaw baseline).
- [ ] Persistent-worker optimisation reduces high-frequency-tool spawn cost to first-call-only.
- [ ] All A6, A7, T9, T10 conformance tests green.

### Rollback

Per-tool manifest kill switch:
```toml
[tools.shell]
backend = "shim"     # "native" Rust HWCA path; "shim" reverts to legacy Python
```

---

## Phase 4 — Provider Plane and Meta-Routers (Weeks 16–20) → M4

**Goal.** Re-bind Hermes's ~20 vendor drivers + 3 API modes to the **`ProviderTrait`** of `gauss-provider`, verified at **build time** by the `gauss-poly` polyhedral-equivalence harness. Add first-class **meta-router** adapters for OpenRouter (aggregator) and NotDiamond (learned router), each carrying a **router-transparency** post-condition.

### Scope

- 20 leaf drivers: Anthropic, OpenAI, Google, Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI, Perplexity, Cohere, Replicate, OctoAI, Anyscale, Hugging Face Inference, Ollama (local), llama.cpp (local), vLLM (local), TGI (local).
- 3 API modes: chat-completion, responses, OpenAI-compat.
- 2 meta-routers: OpenRouter, NotDiamond.
- `RouterProviderTrait : ProviderTrait` with the router-transparency contract.

### Contract surface

```rust
#[contract]
trait ProviderTrait {
    /// Postcondition: tokens.len() <= max_tokens
    /// Postcondition: tokens are well-formed UTF-8
    /// Postcondition: tool_calls ⊆ declared tools
    /// Postcondition: finish_reason ∈ {stop, length, tool}
    async fn complete(&self, p: Prompt, max_tokens: usize)
        -> Result<Stream<Token>, ProviderError>;
}

#[contract]
trait RouterProviderTrait: ProviderTrait {
    fn catalogue(&self) -> &[LeafModel];

    /// Postcondition: result.selected ∈ self.catalogue()
    /// Postcondition: result.tokens schema-equiv to
    ///                 ProviderTrait::complete(result.selected, prompt)
    async fn route_complete(&self, p: Prompt, candidates: &[ModelId],
                            max_tokens: usize)
        -> Result<RoutedStream, ProviderError>;
}
```

### Tasks

1. **Specify `ProviderTrait`.** Land the behavioural contract in `gaussclaw-providers::traits` (delegating to `gauss-provider`); attribute macros emit SMT obligations consumed by `gauss-poly`.
2. **Z3 harness.** `gauss-poly` already discharges contracts at build time (Phase 8 of Gauss-Aether). Extend with the **router-transparency** post-condition: for every leaf `m` in `router.catalogue()`, calling `m` directly and calling `m` via the router must produce schema-identical output.
3. **Migrate 20 leaf drivers.** Each driver lives in `gaussclaw-providers::<vendor>`. The build refuses to admit a driver that fails the contract — modulo `best_effort = true` override in `gaussclaw.toml`.
4. **OpenRouter adapter.**
   - OpenAI-Chat-Completions wire schema.
   - `provider = "openrouter"`, `model = "anthropic/claude-3.5-sonnet"` syntax.
   - Per-model price / latency telemetry pulled into Skill Manifest `[cost]` for Daemon-plane scheduling.
   - Automatic failover verified against the router-transparency contract.
5. **NotDiamond adapter.** Both modes:
   - **Advisory:** call `POST /v2/modelRouter/modelSelect`, then dispatch the generation through the chosen leaf adapter directly. Kernel keeps a clean separation between routing and dispatch.
   - **Joint:** call `POST /v2/chat/completions`, read the selected model from the response metadata.
6. **Capability lower-bound resolution.** For a candidate set `M = {m₁, …, m_k}`, kernel computes `K_t = ⋂ K_t(mᵢ)` at admission and **filters `M` before** the router sees it. The router can never dispatch to a model the kernel would have rejected.
7. **Receipt content for meta-routed turns.** Every routed turn's receipt carries three model IDs: the *candidate set*, the *router's recommendation*, the *model actually used*. The receipt chain hashes all three.
8. **Typed fallback chains.** `gaussclaw.toml` syntax:
   ```toml
   [provider.chain]
   primary  = "anthropic/claude-3.5-sonnet"
   fallback = ["openrouter/anthropic/claude-3.5-sonnet",
               "notdiamond/{claude-3.5-sonnet, gpt-4o, gemini-1.5-pro}"]
   ```
   The build refuses to compile a chain whose members are not polyhedrally equivalent on the working subset.

### Crate dependency edges

```
gaussclaw-providers       → gauss-provider, gauss-poly, gauss-traits
gaussclaw-providers-meta  → gaussclaw-providers, gauss-provider
gaussclaw-api-modes       → gaussclaw-providers
gaussclaw-agent           → gaussclaw-api-modes
```

### Exit criteria (M4)

- [ ] **All 20 leaf drivers** pass `ProviderTrait` at build time.
- [ ] **OpenRouter and NotDiamond** pass `RouterProviderTrait` for their entire current catalogue at build time.
- [ ] **Fallback chains** compile only when all members are polyhedrally equivalent on the working subset.
- [ ] Runtime provider switching (Anthropic-direct → Anthropic-via-OpenRouter → NotDiamond{…}) preserves output schema, tool-call lineage, and receipt content on the regression corpus.
- [ ] **Cost field** populated on every turn (tokens, dollars, model_actual).

### Rollback

Per-driver `best_effort = true` flag downgrades the build-time check to a runtime-only equivalence check. Meta-routers may admit catalogue subsets via:
```toml
[providers.openrouter]
catalogue_blacklist = ["some-vendor/broken-model"]
```

---

## Phase 5 — Trajectory Export and GA (Weeks 20–24) → GA

**Goal.** Extend the SFT/DPO export with a **Cryptographic Trajectory Envelope**, ship the **Taint-Aware Filter**, reference-implement the **Federated Trajectory Pool**, run the **15-axis scorecard**, and reach **General Availability**.

### Scope

- `gaussclaw-export::sft` — preserves Hermes JSONL field schema bit-for-bit.
- `gaussclaw-export::dpo` — preserves Hermes preference-pair schema bit-for-bit.
- `gaussclaw-export::envelope` — Cryptographic Trajectory Envelope (Definition 1 of GaussClaw.pdf).
- `gaussclaw-export::filter` — three modes: `permissive`, `strict`, `declassified`.
- `gaussclaw-fed` — S3-backed reference Federated Pool with publish / subscribe / verify API.
- Atropos integration smoke test.
- 15-axis scorecard evaluation.

### Envelope structure

For turn τᵢ producing SFT record `r_i^sft`:
```
Eᵢ = ⟨ r_i^sft, ρᵢ, c_n, πᵢ, TSA(c_n) ⟩
```
- `ρᵢ = ⟨rᵢ, pk, σᵢ, tᵢ⟩` — the turn's signed receipt
- `c_n` — chain head at envelope creation
- `πᵢ` — Merkle inclusion proof for ρᵢ under `c_n`
- `TSA(c_n)` — timestamp authority attestation of `c_n`

The envelope is **optional** for consumers that ignore it; **mandatory** for federated consumption.

### Tasks

1. **Envelope generator.** Emit `Eᵢ` alongside every `r_i^sft`. Envelope verification API in `gaussclaw-export::verify`:
   ```rust
   pub fn verify_envelope(e: &Envelope, pk: &PublicKey,
                          tsa_root: &TsaRoot) -> Result<()>
   ```
2. **Taint-Aware Filter.** Three modes wired through `gaussclaw.toml`:
   ```toml
   [export.filter]
   mode = "declassified"     # permissive | strict | declassified
   ```
   - **Permissive:** emit all records, taint marked in metadata.
   - **Strict:** drop records containing any token with taint ≥ `web`.
   - **Declassified (default):** apply runtime declass map; emit only tokens where `declass(ℓ) ⪰ ⊥`.
3. **Federated Pool reference.** Small S3-backed publish/subscribe service in `gaussclaw-fed`:
   - **Publish:** PUT envelope to `s3://pool/{org}/{chain_head}/{turn_id}.env`.
   - **Subscribe:** poll manifest; for each envelope verify under publisher pk + TSA before admission.
   - **Filter combinator:** combine with taint filter to admit only envelopes whose declared max-taint is acceptable.
4. **Atropos integration smoke test.** End-to-end: GaussClaw instance generates trajectories → envelope-aware Atropos consumer pulls them → fine-tuning proceeds without divergence from an equivalent Hermes run.
5. **15-axis scorecard.** Run `gaussclaw-bench::scorecard` against Hermes, OpenFang, OpenClaw, ZeroClaw; emit Table IV of GaussClaw.pdf.
6. **Six-metric operational profile.** Cold start, tool overhead, audit cost, hybrid recall, crash recovery, multi-tenant safety. Must tie or lead the best baseline on every metric.
7. **`gaussclaw doctor`.** Self-Diagnostic Health Engine (`gauss-health`) command that runs invariants ℐ, prints federated attestations, surfaces any drift.
8. **Migration UX.** `gaussclaw import hermes ./hermes-config.toml` produces a GaussClaw config with the legacy executor enabled and a phase-by-phase opt-in checklist.
9. **Public bug-bounty period.** Two weeks during which Hermes remains co-deployed for any GA-blocking regression.

### Crate dependency edges

```
gaussclaw-export → gaussclaw-store, gauss-audit, gauss-attest
gaussclaw-fed    → gaussclaw-export, gauss-attest, (s3/ipfs feature)
gaussclaw-bin    → gaussclaw-export, gaussclaw-fed
```

### Exit criteria (GA)

- [ ] Trajectory envelopes verify end-to-end on a corpus of **10⁶ records**.
- [ ] The 15-axis scorecard places GaussClaw **strictly above** each of Hermes, OpenFang, OpenClaw, ZeroClaw on **every** axis.
- [ ] Six-metric operational profile ties or leads the best baseline on **every** metric.
- [ ] `gaussclaw doctor` passes on all three deployment modes (embedded, single-node TCP, TiKV-clustered).
- [ ] Public bug-bounty closes without GA-blocking regressions.
- [ ] `gaussclaw import hermes` round-trips a real Hermes deployment in under 60 s.

### Rollback

GA is gated by the bug-bounty period; failure to meet criteria triggers rollback to the most recent M-milestone-passing build. The legacy Hermes deployment is co-deployed throughout the bounty window.

---

## Cross-Phase Concerns

### Configuration compatibility

`gaussclaw-config` is figment-based, accepts the Hermes TOML top-level keys verbatim, and layers GaussClaw-specific keys under namespaced tables:

```toml
# Hermes-compatible (unchanged)
[provider]
name  = "anthropic"
model = "claude-3.5-sonnet"

[surfaces.rest]
host = "127.0.0.1"
port = 8080

# GaussClaw additions (all optional, defaults preserve Hermes behaviour)
[caps]
default_grant = ["fs:read:./data", "network:http_get"]

[taint]
default_declass = "default"     # default | strict

[export.filter]
mode = "declassified"

[provider.chain]
fallback = ["openrouter/anthropic/claude-3.5-sonnet"]
```

### Conformance suite

`gaussclaw-conformance` carries three test classes that run in every CI build from Phase 1 onward:

1. **Hermes-replay.** A frozen 1,000-turn corpus replayed through both Hermes and GaussClaw; byte-equal trajectory output required.
2. **OAI SDK parity.** OpenAI Python SDK's end-to-end test suite, parametrised by both backends.
3. **Axiom regressions.** Every PR runs the `gauss-conformance` suite to guarantee A1–A9 / T1–T12 hold.

### Risk register (operational mitigations)

| Risk | Mitigation |
|---|---|
| Trajectory schema drift | Dual-write through M2; nightly Hermes ↔ GaussClaw export diff; schema versioning |
| Provider contract failure | Per-driver kill switch; runtime chain fallback to legacy executor; `best_effort = true` |
| Sandbox escape on new tool | Per-tool kill switch; one-week shadow run for every new manifest before production routing |
| Receipt-chain corruption | TSA anchor every 1,000 receipts; continuous chain-verifier sidecar with paging on divergence |
| HWCA spawn-cost regression | Persistent-worker optimisation; p99 latency SLO with auto-rollback to legacy Python on breach |
| Trajectory export blocking | Async pipeline with backpressure to the producer; bounded queue with disk spill |
| Federation poisoning | Consumer-side reputation tracking; public exclusion list; (v2) zk-SNARK envelope variant |

### Engineering discipline

- **Privilege tiers (SPECS §2):** PRs touching `gauss-kernel`, `gauss-audit`, `gauss-attest`, or any new privileged GaussClaw surface require dual review.
- **No `unsafe` in `gaussclaw-*` crates without ADR.** All FFI (Python shim, vendor SDKs) is wrapped in safe abstractions.
- **`#![deny(warnings)]`** on every new crate; clippy `pedantic + nursery` is the floor.
- **Per-PR axiom trace.** PR template requires `Hermes module · Gauss-Aether axiom · GaussClaw phase` triple.

---

## Headline Numbers (target at GA)

| Metric | Hermes baseline | GaussClaw target | Mechanism |
|---|---|---|---|
| IPI attack success rate | not measured | **≤ 2.19 %** | T9 + HWCA + ℒ |
| Cold start (warm cache) | 80–150 ms | **≤ 10 ms** | T12 delta-encoded + K-LRU |
| Composite sandbox compromise | ~ 1 | **≤ 1.1 × 10⁻⁷** | T10 + TEE |
| Hybrid recall miss rate | 0.08 (FTS5 only) | **≤ 0.015** | T5: ε_fts · ε_vec |
| Throughput | single Python proc | **Θ(N) nodes** | T6 stateless-turn routing |
| Provider switching cost | manual retest | **build-time verified** | T7 + polyhedral equiv. |
| Receipt forgery probability | no receipts | **negl(λ)** | T11 EUF-CMA + collision |
| Tool spawn latency p99 | in-proc Python | **≤ 15 ms** | WASM + Landlock |
| Trajectory provenance | operator trust | **cryptographic** | §IV-A envelope |
| Cross-org data sharing | not feasible | **federated pool** | §IV-E |

---

## Beyond GA (v2 horizon)

Out of scope for the 24-week port; tracked in `docs/V2_HORIZON.md` after GA:

- **Zero-knowledge trajectory envelopes.** zk-SNARK proof of "this SFT record came from a verifying receipt under our chain head" without revealing turn timing or chain length (cf. `gauss-zk`).
- **Differential-privacy exporter.** Calibrated Laplace noise on token-level lineage statistics (cf. `gauss-dp`).
- **Learnt Φ.** Adaptive autonomy gradient trained on operator approval decisions stored in the receipt chain (cf. `gauss-learnt`).
- **Mechanised proofs of T1–T12.** Coq / Lean kernel core with extraction.
- **AgentDojo-style adversarial benchmark.** Empirical calibration of the T9 worst-case IPI bound against deployment-specific declass maps.

---

## Closing Thesis

GaussClaw is **not a rewrite** of Hermes. It is the same agent dropped into a kernel that was missing. The trajectory flywheel keeps spinning — but every revolution now leaves a signed, taint-labelled, capability-gated record, and accountability under adversarial conditions becomes a structural guarantee rather than an operational hope.
