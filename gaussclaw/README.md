# GaussClaw

**GaussClaw** is the Hermes-to-Rust agent built on top of the [Gauss-Aether](../gauss-aether/) runtime framework. It re-implements every Hermes capability in safety-critical Rust and adds five architectural guarantees Hermes has no equivalent for.

This directory holds GaussClaw's source code, specification, and roadmap. The companion directory [`../gauss-aether/`](../gauss-aether/) holds the runtime framework GaussClaw binds to.

## Reference documents

| File | Purpose |
|---|---|
| [`ROADMAP.md`](./ROADMAP.md) | Five-phase build plan with exit criteria. Phases 1–5 ship on `main`; the operational GA gates (signing CI, scorecard run, website Lighthouse, bug-bounty) are tracked here. |
| [`SPEC.pdf`](./SPEC.pdf) | Architecture paper. Definitions of the Cryptographic Trajectory Envelope, the polyhedral equivalence contract, the 15-axis scorecard, and the six Hermes-superiority axes. |

## What ships

GaussClaw is a single static binary, `gaussclaw`, that runs the TUI, the embedded React/Vite/Tailwind web dashboard, the OpenAI-compatible API relay, every Hermes CLI subcommand, and a Tauri 2 desktop shell — no Python or Node runtime required at runtime.

Built from 19 single-responsibility crates under [`crates/`](./crates/):

| Surface | Crate |
|---|---|
| Shipping binary, wires every command | [`gaussclaw-bin`](./crates/gaussclaw-bin/) |
| CLI subcommand surface (clap v4) | [`gaussclaw-cli`](./crates/gaussclaw-cli/) |
| Full-screen TUI (ratatui + crossterm) | [`gaussclaw-tui`](./crates/gaussclaw-tui/) |
| Axum dashboard backend + embedded React frontend | [`gaussclaw-web`](./crates/gaussclaw-web/) |
| Tauri 2 desktop shell (~10× smaller than Hermes Electron) | [`gaussclaw-desktop`](./crates/gaussclaw-desktop/) |
| REST/WS/OAI-compat thin surfaces | [`gaussclaw-surfaces`](./crates/gaussclaw-surfaces/) |
| ~20 messaging-channel adapters | [`gaussclaw-channels`](./crates/gaussclaw-channels/) |
| Turn policy + Kernel admit + audit | [`gaussclaw-agent`](./crates/gaussclaw-agent/) |
| Session/turn/lineage store on Gauss-Aether's Trinity backend | [`gaussclaw-store`](./crates/gaussclaw-store/) |
| Skill Manifest parser + `#[tool]` proc-macro | [`gaussclaw-skill`](./crates/gaussclaw-skill/) |
| First-party tools (base64, echo, file_*, hash, json_get, math_eval, regex_match, shell, upper) | [`gaussclaw-tools`](./crates/gaussclaw-tools/) |
| Vendor drivers (Anthropic, OpenAI, Google Gemini, Cohere, Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI, Perplexity, Anyscale, OctoAI, HuggingFace, Replicate, Ollama, llama.cpp, vLLM, TGI) | [`gaussclaw-providers`](./crates/gaussclaw-providers/) |
| Meta-routers (OpenRouter aggregator, NotDiamond learned router) | [`gaussclaw-providers-meta`](./crates/gaussclaw-providers-meta/) |
| OpenAI Chat-Completion / Responses / OAI-compat shims | [`gaussclaw-api-modes`](./crates/gaussclaw-api-modes/) |
| Hermes-compatible TOML configuration | [`gaussclaw-config`](./crates/gaussclaw-config/) |
| `gaussclaw import hermes` migration driver | [`gaussclaw-migrate`](./crates/gaussclaw-migrate/) |
| SFT/DPO writers + Cryptographic Trajectory Envelope + Taint-Aware Filter + `verify_envelope` | [`gaussclaw-export`](./crates/gaussclaw-export/) |
| Federated Trajectory Pool client + reference server | [`gaussclaw-fed`](./crates/gaussclaw-fed/) |
| Hermes-parity test suite (CLI parity, OAI SDK parity, replay corpus, polyhedral provider) | [`gaussclaw-conformance`](./crates/gaussclaw-conformance/) |

## Six Hermes-superiorities

1. **Capability lattice + admit gate** — every tool declares a `CapToken` requirement; the kernel checks it before dispatch. Hermes has no capability model.
2. **Taint lattice + declassification map** — every tool's output carries a taint label; the declass map (`d: ℒ → 𝒦`) is verified antitone at startup. Hermes has no taint tracking.
3. **Composite Sandbox (4 layers)** — WASM L1, Landlock L2, seccomp L3, bwrap L4. Theorem T10 bounds compromise probability at ≤ 1.1 × 10⁻⁷. Hermes runs subprocesses under parent credentials.
4. **Cryptographic Trajectory Envelope** — every export record carries `⟨r, ρ, c_n, π, TSA(c_n)⟩`: Ed25519 receipt + position witness + TSA anchor. Hermes emits raw JSONL with no integrity surface.
5. **Polyhedral equivalence CI gate** — provider swap-compatibility verified at build time by `gauss-poly`. Hermes has no equivalence check.
6. **Single-binary shipping** — `gaussclaw` is one static Rust binary. Hermes requires Python + Node.js at runtime.

## Getting started

```bash
# Build and run the TUI
cargo run --bin gaussclaw

# Inspect health
cargo run --bin gaussclaw -- doctor

# Migrate a Hermes config
cargo run --bin gaussclaw -- import ./hermes-config.toml > gaussclaw.toml

# Verify an export envelope
cargo run --bin gaussclaw -- receipt verify ./envelope.json
```
