# Gauss-Aether · GaussClaw

**An axiomatic runtime for trustworthy autonomous LLM agents — and the
sample agent that runs on top of it. One Rust workspace, two projects,
one static binary.**

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/Rust-1.83+-orange.svg)](rust-toolchain.toml)
[![Tests](https://img.shields.io/badge/tests-299%20passing-brightgreen.svg)]()
[![Runtime](https://img.shields.io/badge/Gauss--Aether-1.0-blueviolet.svg)]()
[![Agent](https://img.shields.io/badge/GaussClaw-shipping-brightgreen.svg)](gaussclaw/)

| Project | What it is |
|---|---|
| **[Gauss-Aether](gauss-aether/)** | The runtime — a kernel-mediated execution substrate for LLM agents whose safety invariants are mechanically checked rather than asserted in a policy document. 22 `gauss-*` crates. |
| **[GaussClaw](gaussclaw/)** | The sample agent — a Rust port of the [Hermes agent](https://github.com/nousresearch/hermes-agent) that runs every turn through Gauss-Aether's kernel and ships as a single static binary with CLI, TUI, web dashboard, and Tauri 2 desktop shell. 19 `gaussclaw-*` crates. |

Gauss-Aether is a clean-room implementation of an LLM agent runtime
where every privileged operation traces back to a numbered axiom or
theorem, every theorem is backed by a property test in
`gauss-conformance`, and every plugin slots into a typed trait surface
that the polyhedral verifier proves swap-compatible. GaussClaw is the
working agent that demonstrates the runtime end-to-end on a real
Hermes-compatible workload.

If you have built or operated an LLM agent and felt the gap between
"please don't do bad things" and "the type system makes bad things
unreachable," Gauss-Aether is what closing that gap looks like — and
GaussClaw is what an agent looks like once it runs on the other side.

---

## At a glance

| Question | Answer |
|---|---|
| Can a tool execute before the turn is logged? | No — the WAL append is the *only* path to the side-effect commit (Axiom A1). |
| Can a tool escalate its capabilities? | No — the kernel's grant can only shrink (`contract`); growth is a compile-time refusal (A2). |
| Can an instruction-injection prompt cross the worker boundary? | No — ≤ 2.19 % theoretical, 0 / 20 empirical on the IPI corpus, behind a schema gate (A7 / T9). |
| Can the audit log be tampered with? | Not without a Merkle-divergent chain (T3) and an Ed25519 signature failure (T11). |
| Can a provider be swapped without retesting the deployment? | The polyhedral verifier (T7) certifies the swap on a probe set at build time. |
| Is the safety story mechanised? | Lean 4 stubs of all nine axioms and twelve theorems live in `gauss-aether/proofs/lean/`. |

---

## Repository layout

```text
gauss-aether/        ← the runtime (22 crates, the SPECS, the proofs)
  crates/gauss-*/
  SPECS.md           ← normative spec: A1–A9, T1–T12, trait surface
  proofs/lean/       ← mechanised proof skeleton

gaussclaw/           ← the sample agent (19 crates, the architecture paper)
  crates/gaussclaw-*/
  SPEC.pdf

docs/                ← shared docs (quickstart, architecture, ADRs, security)
website/             ← Docusaurus site (en + zh-Hans) + mdBook API reference
```

---

## Gauss-Aether — the runtime

Gauss-Aether is organised as seven layers. Everything above the kernel
line goes through one admit gate; nothing reaches a tool or a provider
without a capability check and a signed audit row.

```text
┌────────────────────────────────────────────────────────────────────┐
│ surface           gauss-canvas · gauss-health · gauss-gateway      │
├────────────────────────────────────────────────────────────────────┤
│ verifier          gauss-poly · gauss-bench                         │
├────────────────────────────────────────────────────────────────────┤
│ autonomy + audit  gauss-sag · gauss-audit                          │
├────────────────────────────────────────────────────────────────────┤
│ memory + work     gauss-memory · gauss-hwca · gauss-sandbox        │
├────────────────────────────────────────────────────────────────────┤
│ turn engine       gauss-turn · gauss-provider                      │
├────────────────────────────────────────────────────────────────────┤
│ kernel + traits   gauss-kernel · gauss-traits · gauss-core         │
├────────────────────────────────────────────────────────────────────┤
│ research          gauss-attest · gauss-chaos · gauss-zk · gauss-dp │
│                   gauss-learnt · gauss-robust                      │
└────────────────────────────────────────────────────────────────────┘
```

### What each crate does

| Crate | Role |
|---|---|
| `gauss-core` | Identifiers, actions, observations, taint labels, the `CapToken` lattice, unified `GaussError`. |
| `gauss-traits` | Public trait surface — `Kernel`, `MemoryBackend`, `Provider`, `SandboxTrait`, `ToolTrait`. |
| `gauss-kernel` | Privileged authority — joint K×L admission, lock-free three-plane scheduler, consistent-hash cluster ring. |
| `gauss-turn` | The Differential Turn Engine (Algorithm 1) with optional sandbox, signed receipts, and SAG approval. |
| `gauss-memory` | Trinity Memory — SurrealDB-backed append log + BM25 + HNSW hybrid recall + K-LRU + Myers diff. |
| `gauss-audit` | SHA-256 chain, Ed25519 signed receipts, RFC 3161 / OpenTimestamps anchors, verifier API. |
| `gauss-provider` | Provider adapter contract; `ToyProvider` ships in-tree, vendor drivers live in `gaussclaw-providers`. |
| `gauss-sandbox` | Composite sandbox: WASM (wasmi) + Landlock + seccomp + bwrap + Seatbelt, with a TEE-attest feature. |
| `gauss-hwca` | Hardware-enforced compute attestation — worker contexts + four-stage schema gate (A7, T9). |
| `gauss-sag` | Supervised Autonomy Gradient — `DecisionTable` + monotonicity verifier + approval surfaces. |
| `gauss-poly` | Polyhedral trait-equivalence verifier (T7). |
| `gauss-canvas` | A2UI Live Canvas Protocol — typed widget tree + update stream (T8). |
| `gauss-health` | Self-Diagnosable Health Engine — seven minimum invariants plus custom registration. |
| `gauss-gateway` | REST / WS / SSE wire types and OpenAI-compatible proxy schema. |
| `gauss-attest` | TEE attestation trait with an Ed25519 software simulator (T10 §L4). |
| `gauss-chaos` | Deterministic kill / partition / clock-skew injectors for chaos tests. |
| `gauss-bench` | The 15-axis Pareto-dominance scorecard. |
| `gauss-zk` | Zero-knowledge proofs over the receipt chain (contract; production prover is additive). |
| `gauss-dp` | Differentially-private trajectory exporter — Laplace + Gaussian mechanisms. |
| `gauss-learnt` | Learnt risk classifier Φ̂ — a logistic scorer that *floors* the SAG rule table. |
| `gauss-robust` | Robust declassifiers — adversarially-adaptive taint downgrades. |
| `gauss-conformance` | The axiom-by-axiom property-test harness (A1–A9, T1–T12). |

### How safety is enforced

The Rust type system is the proof carrier. The conformance suite is
the property-test witness. The polyhedral verifier is the plugin-swap
witness. The Lean 4 stubs are the formal-proof carrier. Where the
source paper writes the runtime as the eleven-tuple
`G = (S, A, O, K, M, F, π, L, Φ, R, V)`, each component maps to a
single crate:

```text
S  state         ← gauss-core            Φ  risk classifier  ← gauss-sag + gauss-learnt
A  actions       ← gauss-core            R  rendering        ← gauss-canvas + gauss-gateway
O  observations  ← gauss-core            V  verification     ← gauss-conformance + gauss-poly
K  capabilities  ← gauss-kernel          L  taint lattice    ← gauss-core + gauss-kernel::flow
M  memory        ← gauss-memory          F  audit chain      ← gauss-audit
π  policy        ← gauss-provider + gauss-poly
```

The axioms and theorems, each with a conformance test:

| ID | Statement | Lives in | Test |
|---|---|---|---|
| **A1** | External effects fire only after the WAL append durably succeeds. | `gauss-turn` | `axiom_a1_wal_before_effect` |
| **A2 / T2** | Capabilities monotonically shrink under `contract`; CAS-protected. | `gauss-kernel::admit` | `axiom_a2_capability_monotonicity` |
| **A3 / T3** | Modifying any payload diverges the Merkle chain head. | `gauss-audit::chain` | `theorem_t3_merkle_tamper_evidence` |
| **A4 / T4** | Three-plane scheduler has a `B/ρ` starvation bound. | `gauss-kernel::sched` | `theorem_t4_starvation_bound` |
| **A5 / T5 / T12** | Memory monoid laws + hybrid recall + delta warm-switch. | `gauss-memory` | `axiom_a5_memory_monoid`, `theorem_t5_*`, `theorem_t12_*` |
| **A6** | Information-flow lattice with antitone declassification. | `gauss-kernel::flow` | `axiom_a6_taint_lattice` |
| **A7 / T9** | Worker-context isolation + IPI containment ≤ 2.19 %. | `gauss-hwca` | `axiom_a7_and_theorem_t9_hwca` |
| **A8** | Supervised-autonomy gradient — monotone risk classifier. | `gauss-sag` | `axiom_a8_sag_approval` |
| **A9 / T11** | Ed25519 EUF-CMA signatures, receipt non-repudiation. | `gauss-audit::sign` | signing crate tests |
| **T6** | Stateless-turn scaling via consistent-hash routing. | `gauss-kernel::cluster` | `theorem_t6_stateless_scaling_and_attest` |
| **T7** | Provider adjunction — polyhedral equivalence on a probe set. | `gauss-poly` | `theorem_t7_provider_adjunction` |
| **T8** | Pareto-dominance scorecard (15-axis comparison). | `gauss-bench` | `theorem_t8_pareto_dominance` |
| **T10** | Composite sandbox bound — `Pr[compromise] ≤ Π pᵢ + p_T`. | `gauss-sandbox` | `theorem_t10_composite_sandbox` |
| **T10 §L4** | TEE attestation simulator + verifier. | `gauss-attest` | `theorem_t6_stateless_scaling_and_attest` (L4 row) |

The full 299-test conformance suite completes in about three seconds
on a modern laptop.

---

## GaussClaw — the sample agent

GaussClaw is what Gauss-Aether looks like when you actually ship it.
It is a Hermes-compatible agent, written entirely in Rust, with no
Python or Node.js runtime, distributed as a single static binary called
`gaussclaw`. The same binary runs the CLI, the full-screen TUI, the
Axum-based web dashboard with an embedded React frontend, the
OpenAI-compatible API relay, and the Tauri 2 desktop shell.

### What ships in the box

| Surface | Crate |
|---|---|
| Shipping static binary | [`gaussclaw-bin`](gaussclaw/crates/gaussclaw-bin/) |
| Turn loop — Kernel admit + audit + SAG + provider call | [`gaussclaw-agent`](gaussclaw/crates/gaussclaw-agent/) |
| CLI subcommands (clap v4, drop-in for `hermes`) | [`gaussclaw-cli`](gaussclaw/crates/gaussclaw-cli/) |
| Full-screen TUI (Ratatui + crossterm) | [`gaussclaw-tui`](gaussclaw/crates/gaussclaw-tui/) |
| Axum dashboard + embedded React/Vite/Tailwind frontend | [`gaussclaw-web`](gaussclaw/crates/gaussclaw-web/) |
| Tauri 2 desktop shell (signed + notarised, ~10× smaller than the Hermes Electron app) | [`gaussclaw-desktop`](gaussclaw/crates/gaussclaw-desktop/) |
| REST · WebSocket · OpenAI-compatible relay | [`gaussclaw-surfaces`](gaussclaw/crates/gaussclaw-surfaces/) |
| ~20 messaging-channel adapters | [`gaussclaw-channels`](gaussclaw/crates/gaussclaw-channels/) |
| Session, turn, and lineage store on the Trinity backend | [`gaussclaw-store`](gaussclaw/crates/gaussclaw-store/) |
| Skill Manifest parser + `#[tool]` proc-macro | [`gaussclaw-skill`](gaussclaw/crates/gaussclaw-skill/) |
| First-party tools: base64, echo, file_*, hash, json_get, math_eval, regex_match, shell, upper | [`gaussclaw-tools`](gaussclaw/crates/gaussclaw-tools/) |
| 20 vendor drivers — Anthropic, OpenAI, Gemini, Cohere, Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI, Perplexity, Anyscale, OctoAI, HuggingFace, Replicate, Ollama, llama.cpp, vLLM, TGI | [`gaussclaw-providers`](gaussclaw/crates/gaussclaw-providers/) |
| Meta-routers — OpenRouter aggregator, NotDiamond learned router | [`gaussclaw-providers-meta`](gaussclaw/crates/gaussclaw-providers-meta/) |
| OpenAI Chat-Completion · Responses · OAI-compat shims | [`gaussclaw-api-modes`](gaussclaw/crates/gaussclaw-api-modes/) |
| Hermes-compatible TOML configuration | [`gaussclaw-config`](gaussclaw/crates/gaussclaw-config/) |
| `gaussclaw import hermes` migration driver | [`gaussclaw-migrate`](gaussclaw/crates/gaussclaw-migrate/) |
| SFT/DPO writers + Cryptographic Trajectory Envelope + Taint-Aware Filter + `verify_envelope` | [`gaussclaw-export`](gaussclaw/crates/gaussclaw-export/) |
| Federated Trajectory Pool client + reference server | [`gaussclaw-fed`](gaussclaw/crates/gaussclaw-fed/) |
| Hermes-parity test suite — CLI parity, OAI SDK parity, replay corpus, polyhedral provider | [`gaussclaw-conformance`](gaussclaw/crates/gaussclaw-conformance/) |

### What GaussClaw adds on top of a Hermes-style agent

Six architectural guarantees that Hermes has no equivalent for:

1. **Capability lattice + admit gate.** Every tool declares a `CapToken` requirement; the kernel checks it before dispatch and can only shrink the grant. Hermes has no capability model.
2. **Taint lattice + declassification map.** Every tool output carries a taint label; the declass map `d: ℒ → 𝒦` is verified antitone at startup.
3. **Composite Sandbox (four layers).** WASM L1 + Landlock L2 + seccomp L3 + bwrap L4 (Seatbelt on macOS). Theorem T10 bounds compromise probability at ≤ 1.1 × 10⁻⁷. Hermes runs subprocesses under parent credentials.
4. **Cryptographic Trajectory Envelope.** Every exported record carries `⟨r, ρ, c_n, π, TSA(c_n)⟩` — Ed25519 receipt + position witness + TSA anchor. Hermes emits raw JSONL with no integrity surface.
5. **Polyhedral equivalence CI gate.** Provider swap-compatibility is verified at build time by `gauss-poly`.
6. **Single-binary shipping.** `gaussclaw` is one static Rust binary; no interpreter at runtime.

### How GaussClaw closes Hermes's architectural deficits

| Hermes behaviour | Gauss-Aether subsystem | Theorem |
|---|---|---|
| Tool fn runs in the host interpreter with full ambient credentials | `gauss-kernel` capability lattice 𝒦 + `gauss-sandbox` Composite Sandbox | T9, T10 |
| SQLite store is mutable; no signed record exists | `gauss-memory` Trinity over SurrealDB + `gauss-audit` Receipt Chain (Ed25519) | T3, T11 |
| Tool text flows into the next prompt verbatim | `gauss-hwca` worker context + schema-validated `ValidatedValue` | T9 |
| Web/email observations carry no taint | `gauss-kernel::flow` info-flow lattice ℒ + declass map | A6 |
| Background and user turns share one event loop | `gauss-kernel::sched` three-plane scheduler (conv / daemon / approval) | T4 |

### Hermes parity, measured

GaussClaw's test surface runs alongside the runtime conformance suite
on every PR:

- **Hermes-replay** — a frozen 1,000-turn corpus produces byte-identical trajectory output.
- **OpenAI SDK parity** — the official end-to-end suite is parametrised against both backends.
- **CLI parity** — `gaussclaw --help` is diffed against a frozen Hermes `--help` corpus.
- **TUI snapshot** — `insta` golden snapshots cover every documented Ratatui screen state.
- **Web e2e** — Playwright drives the React frontend against both backends.
- **Desktop e2e** — `webdriverio + tauri-driver` drives all 12 Hermes-parity screens on macOS, Windows, and Linux.

### Measured vs. Hermes

| Axis | Hermes | GaussClaw | Mechanism |
|---|---|---|---|
| IPI attack success rate | not measured (no defence) | ≤ 2.19 % | T9 + HWCA + ℒ |
| Cold start (warm cache) | 80–150 ms (Python import) | ≤ 10 ms | T12 delta-encoded + K-LRU |
| Composite sandbox compromise | ~ 1 (no sandbox) | ≤ 1.1 × 10⁻⁷ | T10 + TEE |
| Hybrid recall miss rate | 0.08 (FTS5 only) | ≤ 0.015 | T5: ε_fts · ε_vec |
| Receipt forgery probability | no receipts | negl(λ) | T11 EUF-CMA + collision |
| Provider switching | manual retest | build-time verified | T7 + polyhedral equivalence |
| Desktop installer size | ~150 MB (Electron 39) | ≤ 20 MB (Tauri 2) | OS WebView, no Chromium |
| Desktop RAM idle | ~250 MB | ≤ 80 MB | OS WebView |
| Desktop code-signing | unsigned everywhere | signed + notarised on all 3 OSes | CI signing + `gauss-attest` |

---

## Quick start

Requires **Rust 1.83+** (workspace MSRV). Builds on Linux, macOS, and
Windows; Linux gets the full four-layer sandbox stack, macOS substitutes
Seatbelt at L2.

```bash
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether

# Build the workspace and run the conformance suite (~3 s on a modern laptop).
cargo test --workspace

# Run the agent.
cargo run --bin gaussclaw                              # full-screen TUI
cargo run --bin gaussclaw -- doctor                    # health check
cargo run --bin gaussclaw -- serve --port 8080         # web dashboard + OAI relay
cargo run --bin gaussclaw -- import ./hermes.toml      # migrate a Hermes config
cargo run --bin gaussclaw -- receipt verify ./env.json # verify a trajectory envelope
```

The same gates CI enforces:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
```

### Embedding the runtime directly

```rust
use std::sync::Arc;
use gauss_core::{CapToken, TurnId};
use gauss_kernel::PrivilegedKernel;
use gauss_memory::SurrealMemory;
use gauss_provider::ToyProvider;
use gauss_sag::{ApprovalGate, default_decision_table, AutoApprove};
use gauss_turn::{TurnEngine, TurnInput};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let memory   = Arc::new(SurrealMemory::open_in_memory().await?);
    let kernel   = Arc::new(PrivilegedKernel::new(CapToken::TOP));
    let provider = Arc::new(ToyProvider::always_text("hi from gauss-aether!"));
    let sag      = Arc::new(ApprovalGate::new(
        default_decision_table(),
        AutoApprove::new("operator"),
    ));

    let engine = TurnEngine::new(kernel, memory.clone(), provider).with_sag(sag);

    let summary = engine.run_turn(TurnInput {
        id:  TurnId::new(1),
        obs: /* your observation */ todo!(),
    }).await?;

    println!("chain head = {}", hex::encode(summary.chain_head.digest));
    Ok(())
}
```

See [`docs/QUICKSTART.md`](docs/QUICKSTART.md) for the full embed
walkthrough — SAG approval round-trip, signed receipts, gateway wire
types.

---

## Documentation

**Runtime — Gauss-Aether.**

- [`gauss-aether/SPECS.md`](gauss-aether/SPECS.md) — the normative engineering specification.
- [`gauss-aether/Gauss-Aether.pdf`](gauss-aether/Gauss-Aether.pdf) — the architecture paper.
- [`gauss-aether/proofs/lean/`](gauss-aether/proofs/lean/) — Lean 4 stubs for all nine axioms and twelve theorems.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — crate-by-crate architecture tour.
- [`docs/SECURITY.md`](docs/SECURITY.md) — threat model and disclosure policy.
- [`docs/adr/`](docs/adr/) — sixteen architecture decision records (ADR-0001 to ADR-0016).

**Agent — GaussClaw.**

- [`gaussclaw/README.md`](gaussclaw/README.md) — the agent's own guide.
- [`gaussclaw/SPEC.pdf`](gaussclaw/SPEC.pdf) — the architecture paper: Cryptographic Trajectory Envelope, polyhedral equivalence contract, 15-axis scorecard.
- [`docs/HERMES_ADAPTER_MATRIX.md`](docs/HERMES_ADAPTER_MATRIX.md) — Hermes-module → GaussClaw-crate mapping.
- [`website/`](website/) — Docusaurus content tree (English + Simplified Chinese) and mdBook API reference.

---

## Design tenets

1. **Axioms before features.** Every subsystem traces back to an axiom or theorem.
2. **No `unsafe` in privileged crates.** Workspace lint: `unsafe_code = "forbid"`.
3. **Lock-free where the CAS pattern is clean.** The three-plane scheduler packs `(tokens, epoch_ms)` into a single `AtomicU64`.
4. **Property tests + type-state.** Lattice laws and chain integrity are property-tested; the Differential Turn Engine encodes phase ordering in the type system.
5. **`#[non_exhaustive]` everywhere.** Field and variant additions stay semver-minor.
6. **The WAL barrier is structural.** Tool execution is *unreachable* until `memory.append(...)` returns — A1 by construction, not by convention.
7. **The worker boundary is structural.** Only a `ValidatedValue` crosses back to the parent — A7 by construction.
8. **Receipts cover the SAG verdict.** Approval decisions are part of the canonical signed payload — A8 ∧ T11 by construction.
9. **Recall is monoidal and hybrid.** Memory composition is associative; BM25 ∪ HNSW are fused with weighted union — A5 ∧ T5.
10. **Autonomy is gated by an auditable table.** The monotone `DecisionTable` is the policy floor; the learnt scorer can only tighten it.

---

## Licence

MIT — see [`LICENSE-MIT`](LICENSE-MIT). The project was originally
dual-licensed (Apache-2.0 OR MIT); the 1.0 release simplified to
MIT-only for plugin-ecosystem clarity (ADR-0017).

---

## Contributing

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md). In short:

- Every PR cites the axiom, theorem, or SPECS section it advances.
- Every new plugin trait follows the four-rule `specT` style guide — serialisable outputs, `#[non_exhaustive]`, unified `GaussError`, probe-set-checkable invariants (ADR-0014).
- Tier-0 changes (`gauss-kernel`, `gauss-audit`, `gauss-attest`) require dual review.

---

## Citing

```bibtex
@software{gauss_aether_2026,
  author = {Gauss-Aether Contributors},
  title  = {Gauss-Aether: An Axiomatic Runtime for Trustworthy LLM Agents},
  year   = 2026,
  url    = {https://github.com/rismanmattotorang/gauss-aether},
  license = {MIT}
}
```

---

## Acknowledgements

Gauss-Aether builds on lessons from OpenClaw, ZeroClaw, OpenFang, and
Hermes; its 15-axis scorecard exists precisely to make
"successor-of" a falsifiable claim rather than a marketing one.
GaussClaw is the working demonstration that a Hermes-style agent and a
Gauss-Aether-style kernel can occupy the same process without either
side losing what makes it valuable. The source paper's axiom and
theorem numbering is preserved verbatim across the SPECS, the
conformance suite, the ADRs, and the Lean stubs.
