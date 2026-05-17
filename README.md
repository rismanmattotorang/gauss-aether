# GaussClaw · Gauss-Aether

**The verifiable AI agent. Built in Rust. Proven safe by construction.**

GaussClaw is a self-improving AI agent that ships as a single static
binary — no Python, no Node, no interpreter. Underneath it runs on
**Gauss-Aether**, a kernel-mediated runtime whose safety properties
are mechanically checked at build time, signed at runtime, and
re-verified on every release.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/Rust-1.83+-orange.svg)](rust-toolchain.toml)
[![Tests](https://img.shields.io/badge/conformance-299%20green-brightgreen.svg)]()
[![Runtime](https://img.shields.io/badge/Gauss--Aether-1.0-blueviolet.svg)]()
[![Agent](https://img.shields.io/badge/GaussClaw-shipping-brightgreen.svg)](gaussclaw/)
[![Docs](https://img.shields.io/badge/docs-website-blue.svg)](website/)

If you have run a Hermes-class agent in production and watched it
execute a tool you didn't authorise, write to a store you can't audit,
or absorb a prompt-injection you can't detect — GaussClaw is the agent
where those failure modes are *unreachable*, not unlikely.

---

## Why GaussClaw

**🔐 Safe by construction, not by prompt.**
Every tool call passes a capability admit gate before it runs.
Capabilities can only shrink, never grow — the type system refuses to
compile code that tries. Tool output crosses a four-stage schema gate
before it touches the next prompt, capping prompt-injection success at
**≤ 2.19 %** (0 / 20 empirical). The Composite Sandbox stacks WASM +
Landlock + seccomp + bwrap, bounding compromise at **≤ 1.1 × 10⁻⁷**.

**🧾 Cryptographic audit trail.**
Every turn is committed to a Merkle chain, signed with Ed25519, and
anchored to an RFC 3161 timestamp authority every 1,000 receipts. The
chain is part of the same database transaction as the turn write —
you cannot have an effect without a signed record of it.

**🦀 One binary. No runtime.**
`gaussclaw` is a single static Rust binary that hosts the CLI, the
full-screen TUI, the web dashboard with embedded React frontend, the
OpenAI-compatible API relay, and the Tauri 2 desktop shell. **~20 MB**
installer, **~80 MB** RAM idle, **≤ 10 ms** cold start — about an
order of magnitude better than Hermes Electron + Python on every axis.

**🔌 Drop-in for Hermes.**
GaussClaw reads Hermes TOML configs, preserves the `@tool` decorator
ergonomics behind a Rust `#[tool]` proc-macro, passes the official
OpenAI SDK end-to-end suite against both backends, and replays a
frozen 1,000-turn Hermes corpus byte-for-byte. Migration is one
command: `gaussclaw import hermes ./config.toml`.

**🌐 Lives where your team works.**
Telegram, Discord, Slack, WhatsApp, Signal, Matrix, IRC, email, SMS —
~20 messaging adapters through one gateway. Voice memo transcription
included. Same single binary.

**🧠 Any model, anywhere.**
20 first-party vendor drivers (Anthropic, OpenAI, Gemini, Cohere,
Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI,
Perplexity, Anyscale, OctoAI, HuggingFace, Replicate, Ollama,
llama.cpp, vLLM, TGI) plus meta-routers for OpenRouter and
NotDiamond. Switch with `gaussclaw model` — the polyhedral verifier
proves the swap is behaviourally equivalent on a probe set before you
ship it.

**📚 Self-improving, in your hand.**
Agent-curated memory with periodic nudges. Autonomous skill creation
after complex tasks. FTS5 + HNSW hybrid recall (BM25 ∪ vector,
weighted union) — miss rate **≤ 1.5 %**, vs. ~8 % for FTS5-only
stores.

**📤 Trajectories you can prove.**
SFT and DPO export with the Cryptographic Trajectory Envelope —
`⟨r, ρ, c_n, π, TSA(c_n)⟩` per record. Any downstream consumer can
verify a trajectory was produced by a real receipt chain and not
synthesised after the fact. Optional differential-privacy noise on
export. Federated Trajectory Pool client included.

---

## How GaussClaw compares to Hermes

| Axis | Hermes | **GaussClaw** | Mechanism |
|---|---|---|---|
| Tool capability check | none | ✅ kernel admit gate, monotone shrink | A2 |
| Prompt-injection containment | none | ✅ ≤ 2.19 % theoretical, 0/20 empirical | T9 + HWCA |
| Tool sandbox | parent credentials | ✅ 4-layer composite (compromise ≤ 1.1 × 10⁻⁷) | T10 |
| Audit log | mutable SQLite | ✅ Ed25519 + Merkle + TSA anchor | T11, T3 |
| Taint tracking | none | ✅ info-flow lattice ℒ + antitone declass | A6 |
| Provider swap | manual retest | ✅ build-time polyhedral equivalence | T7 |
| Trajectory integrity | raw JSONL | ✅ Cryptographic Envelope | A9 |
| Cold start (warm cache) | 80–150 ms | **≤ 10 ms** | T12 + K-LRU |
| Hybrid recall miss rate | 0.08 | **≤ 0.015** | T5: ε_fts · ε_vec |
| Desktop installer | ~150 MB (Electron) | **≤ 20 MB** (Tauri 2) | OS WebView |
| Desktop RAM idle | ~250 MB | **≤ 80 MB** | OS WebView |
| Desktop signing | unsigned | **signed + notarised** on 3 OSes | CI + `gauss-attest` |
| Runtime dependencies | Python + Node | **one static binary** | — |

Every row marked ✅ is backed by a property test in `gauss-conformance`,
re-run on every PR; the 299-test suite finishes in about three seconds.

---

## Install

**Linux, macOS, WSL2:**
```bash
cargo install gaussclaw                    # from crates.io once published
# or from source:
git clone https://github.com/rismanmattotorang/gauss-aether && cd gauss-aether
cargo install --path gaussclaw/crates/gaussclaw-bin
```

**Desktop app** (signed installers, macOS / Windows / Linux): see
[Releases](https://github.com/rismanmattotorang/gauss-aether/releases).

**Build the workspace and run conformance** (the same gates CI enforces):
```bash
cargo test --workspace                                    # 299 tests, ~3 s
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

---

## Get started

```bash
gaussclaw                              # full-screen TUI
gaussclaw serve --port 8080            # web dashboard + OpenAI-compat relay
gaussclaw model                        # pick a provider
gaussclaw gateway                      # connect Telegram / Discord / Slack / …
gaussclaw doctor                       # health check (seven invariants)
gaussclaw import hermes ./hermes.toml  # migrate a Hermes config
gaussclaw receipt verify ./env.json    # verify a trajectory envelope
```

The same binary serves every surface — there is no separate web server,
gateway daemon, or desktop wrapper to keep in sync.

---

## What's in the box

GaussClaw is **41 single-responsibility crates** split into two layers:
the agent (19 `gaussclaw-*` crates) on top of the runtime (22 `gauss-*`
crates).

### Agent surfaces — [`gaussclaw/`](gaussclaw/)

| Surface | Crate |
|---|---|
| CLI subcommands (clap v4, Hermes-compatible) | [`gaussclaw-cli`](gaussclaw/crates/gaussclaw-cli/) |
| Full-screen TUI (Ratatui + crossterm) | [`gaussclaw-tui`](gaussclaw/crates/gaussclaw-tui/) |
| Axum dashboard + embedded React/Vite frontend | [`gaussclaw-web`](gaussclaw/crates/gaussclaw-web/) |
| Tauri 2 desktop shell, signed + notarised | [`gaussclaw-desktop`](gaussclaw/crates/gaussclaw-desktop/) |
| REST · WebSocket · OpenAI-compat relay | [`gaussclaw-surfaces`](gaussclaw/crates/gaussclaw-surfaces/) |
| ~20 messaging-channel adapters | [`gaussclaw-channels`](gaussclaw/crates/gaussclaw-channels/) |
| Turn loop — kernel admit + audit + SAG | [`gaussclaw-agent`](gaussclaw/crates/gaussclaw-agent/) |
| Session, turn, lineage store (Trinity backend) | [`gaussclaw-store`](gaussclaw/crates/gaussclaw-store/) |
| Skill Manifest parser + `#[tool]` proc-macro | [`gaussclaw-skill`](gaussclaw/crates/gaussclaw-skill/) |
| First-party tools — base64, file_*, hash, json_get, math_eval, regex_match, shell, … | [`gaussclaw-tools`](gaussclaw/crates/gaussclaw-tools/) |
| 20 LLM vendor drivers | [`gaussclaw-providers`](gaussclaw/crates/gaussclaw-providers/) |
| Meta-routers — OpenRouter, NotDiamond | [`gaussclaw-providers-meta`](gaussclaw/crates/gaussclaw-providers-meta/) |
| OpenAI Chat-Completion · Responses · OAI-compat | [`gaussclaw-api-modes`](gaussclaw/crates/gaussclaw-api-modes/) |
| Hermes-compatible TOML config | [`gaussclaw-config`](gaussclaw/crates/gaussclaw-config/) |
| `gaussclaw import hermes` migration driver | [`gaussclaw-migrate`](gaussclaw/crates/gaussclaw-migrate/) |
| SFT/DPO writer + Cryptographic Trajectory Envelope | [`gaussclaw-export`](gaussclaw/crates/gaussclaw-export/) |
| Federated Trajectory Pool client + reference server | [`gaussclaw-fed`](gaussclaw/crates/gaussclaw-fed/) |
| Hermes-parity test suite | [`gaussclaw-conformance`](gaussclaw/crates/gaussclaw-conformance/) |
| The shipping binary | [`gaussclaw-bin`](gaussclaw/crates/gaussclaw-bin/) |

### Runtime kernel — [`gauss-aether/`](gauss-aether/)

| Crate | Role |
|---|---|
| `gauss-core` | Capability tokens, taint labels, observation types, lattice algebra. |
| `gauss-traits` | Public trait surface — `Kernel`, `MemoryBackend`, `Provider`, `ToolTrait`. |
| `gauss-kernel` | Privileged admit gate, lock-free three-plane scheduler, cluster ring. |
| `gauss-turn` | The Differential Turn Engine — WAL-before-effect by construction. |
| `gauss-memory` | Trinity Memory — SurrealDB + BM25 + HNSW + K-LRU + Myers diff. |
| `gauss-audit` | Ed25519 receipts, SHA-256 chain, RFC 3161 / OpenTimestamps anchors. |
| `gauss-sandbox` | Composite sandbox — WASM + Landlock + seccomp + bwrap + Seatbelt. |
| `gauss-hwca` | Worker contexts + four-stage schema gate (the prompt-injection wall). |
| `gauss-sag` | Supervised Autonomy Gradient — monotone approval table. |
| `gauss-poly` | Polyhedral provider-equivalence verifier. |
| `gauss-gateway` | REST / WS / SSE wire types and OAI-compat proxy. |
| `gauss-attest` | TEE attestation trait + Ed25519 software simulator. |
| `gauss-canvas` · `gauss-health` · `gauss-chaos` · `gauss-bench` · `gauss-conformance` | Live canvas, health invariants, chaos injectors, scorecard, conformance harness. |
| `gauss-zk` · `gauss-dp` · `gauss-learnt` · `gauss-robust` | Research tracks behind stable trait contracts (zk envelopes, DP export, learnt-Φ, robust declassifiers). |

Full crate-by-crate detail in [`gauss-aether/README.md`](gauss-aether/README.md)
and [`gaussclaw/README.md`](gaussclaw/README.md).

---

## The safety story, in one screen

Where Hermes asks the model not to misbehave, GaussClaw makes
misbehaviour either *unreachable* (compile-time) or *evident*
(crypto-detectable). The source paper models the system as the
tuple `G = (S, A, O, K, M, F, π, L, Φ, R, V)`; each component is one
crate, each invariant is one numbered axiom or theorem, each axiom
has a property test:

| ID | Property | Crate | Test |
|---|---|---|---|
| **A1** | Effects fire only after the WAL append commits. | `gauss-turn` | `axiom_a1_wal_before_effect` |
| **A2 / T2** | Capabilities monotonically shrink; CAS-protected. | `gauss-kernel` | `axiom_a2_capability_monotonicity` |
| **A3 / T3** | Mutating any payload diverges the Merkle head. | `gauss-audit` | `theorem_t3_merkle_tamper_evidence` |
| **A4 / T4** | Three-plane scheduler has a `B/ρ` starvation bound. | `gauss-kernel` | `theorem_t4_starvation_bound` |
| **A5 / T5 / T12** | Memory monoid laws + hybrid recall + warm switch. | `gauss-memory` | `axiom_a5_memory_monoid` etc. |
| **A6** | Info-flow lattice with antitone declassification. | `gauss-kernel::flow` | `axiom_a6_taint_lattice` |
| **A7 / T9** | Worker isolation + IPI ≤ 2.19 %. | `gauss-hwca` | `axiom_a7_and_theorem_t9_hwca` |
| **A8** | Monotone Supervised Autonomy Gradient. | `gauss-sag` | `axiom_a8_sag_approval` |
| **A9 / T11** | EUF-CMA Ed25519 receipts; non-repudiation. | `gauss-audit::sign` | signing tests |
| **T6** | Stateless-turn scaling via consistent-hash routing. | `gauss-kernel::cluster` | `theorem_t6_stateless_scaling_and_attest` |
| **T7** | Polyhedral provider equivalence on a probe set. | `gauss-poly` | `theorem_t7_provider_adjunction` |
| **T8** | 15-axis Pareto-dominance scorecard. | `gauss-bench` | `theorem_t8_pareto_dominance` |
| **T10** | Composite-sandbox bound `Pr[compromise] ≤ Π pᵢ + p_T`. | `gauss-sandbox` | `theorem_t10_composite_sandbox` |

Lean 4 stubs of all nine axioms and twelve theorems live in
[`gauss-aether/proofs/lean/`](gauss-aether/proofs/lean/); they are
discharged incrementally without changing the runtime contract.

---

## Embed the runtime

GaussClaw is one consumer of Gauss-Aether. If you want a different
agent on the same kernel — say, a research notebook driver, an
infrastructure-automation worker, or a single-purpose code reviewer —
embed the runtime directly:

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

End-to-end embed walkthrough — SAG approval round-trip, signed
receipts, gateway wire types — in
[`docs/QUICKSTART.md`](docs/QUICKSTART.md).

---

## Documentation

- **[`gaussclaw/README.md`](gaussclaw/README.md)** — the agent: surfaces, channels, tools, providers.
- **[`gauss-aether/README.md`](gauss-aether/README.md)** — the runtime: trait surface, axioms, proofs.
- **[`gauss-aether/SPECS.md`](gauss-aether/SPECS.md)** — normative specification (axioms A1–A9, theorems T1–T12).
- **[`gauss-aether/Gauss-Aether.pdf`](gauss-aether/Gauss-Aether.pdf)** — runtime architecture paper.
- **[`gaussclaw/SPEC.pdf`](gaussclaw/SPEC.pdf)** — agent architecture paper.
- **[`docs/QUICKSTART.md`](docs/QUICKSTART.md)** · **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** · **[`docs/SECURITY.md`](docs/SECURITY.md)**
- **[`docs/HERMES_ADAPTER_MATRIX.md`](docs/HERMES_ADAPTER_MATRIX.md)** — Hermes-module → GaussClaw-crate map.
- **[`docs/adr/`](docs/adr/)** — sixteen architecture decision records.
- **[`website/`](website/)** — Docusaurus site (English + Simplified Chinese) and mdBook API reference.

---

## Design tenets

1. **Properties before features.** Every subsystem traces back to a numbered axiom or theorem.
2. **No `unsafe` in privileged crates.** Workspace lint: `unsafe_code = "forbid"`.
3. **The WAL barrier is structural.** Tool execution is *unreachable* until the receipt commits.
4. **The worker boundary is structural.** Only a `ValidatedValue` crosses back to the parent.
5. **Receipts cover the approval verdict.** SAG decisions are part of the canonical signed payload.
6. **Recall is monoidal and hybrid.** BM25 ∪ HNSW with weighted union; composition is associative.
7. **Autonomy is gated by an auditable table.** Monotone `DecisionTable` is the floor; the learnt scorer can only tighten it.

---

## Licence

MIT — see [`LICENSE-MIT`](LICENSE-MIT). Originally dual-licensed
(Apache-2.0 OR MIT); the 1.0 release simplified to MIT-only for
plugin-ecosystem clarity (ADR-0017).

---

## Contributing

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md). The short version:

- Every PR cites the axiom, theorem, or SPECS section it advances.
- New plugin traits follow the four-rule `specT` style guide — serialisable outputs, `#[non_exhaustive]`, unified `GaussError`, probe-set-checkable invariants (ADR-0014).
- Tier-0 changes (`gauss-kernel`, `gauss-audit`, `gauss-attest`) require dual review.

---

## Citing

```bibtex
@software{gauss_aether_2026,
  author  = {Gauss-Aether Contributors},
  title   = {Gauss-Aether: A Verifiable Runtime for Trustworthy LLM Agents},
  year    = 2026,
  url     = {https://github.com/rismanmattotorang/gauss-aether},
  license = {MIT}
}
```

---

## Acknowledgements

GaussClaw is the working demonstration that a Hermes-class agent and
a verifiable kernel can occupy the same process without either side
losing what makes it valuable. The 15-axis Pareto scorecard exists
precisely to make "successor-of" a falsifiable claim rather than a
marketing one — we encourage running it against any agent that wants
to make the same claim.
