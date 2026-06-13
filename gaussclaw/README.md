# GaussClaw

**The verifiable AI agent. One static Rust binary. Drop-in for Hermes.**

GaussClaw is a self-improving, multi-surface AI agent that runs every
turn through the [Gauss-Aether](../gauss-aether/) kernel. It ships as
a **single static binary** — no Python, no Node.js, no Electron — and
matches the Hermes feature surface line-for-line while closing
architectural gaps Hermes has no equivalent for.

[![Tests](https://img.shields.io/badge/Hermes--parity-passing-brightgreen.svg)]()
[![Single binary](https://img.shields.io/badge/runtime-static%20Rust-orange.svg)]()
[![Sandbox](https://img.shields.io/badge/sandbox-4--layer-blue.svg)]()
[![Receipts](https://img.shields.io/badge/audit-Ed25519%20%2B%20Merkle-blueviolet.svg)]()

---

## What you get

The same `gaussclaw` binary hosts every interface:

- 💻 **Full-screen TUI** — Ratatui + crossterm, multiline editing, slash-command autocomplete, streaming tool output, interrupt-and-redirect.
- 🌐 **Web dashboard** — Axum backend, React + Vite + Tailwind frontend, embedded as assets. Same backend serves the OpenAI-compatible API relay.
- 🖥️ **Desktop app** — Tauri 2, signed and notarised on macOS, Windows, and Linux. **~20 MB** installer, **~80 MB** RAM idle, OS WebView — no bundled Chromium.
- 🛠️ **CLI** — clap v4 subcommand parity with the Hermes CLI; `gaussclaw --help` is diffed against a frozen Hermes corpus on every PR.
- 📡 **Gateway** — Telegram, Discord, Slack, WhatsApp, Signal, Matrix, IRC, email, SMS, and ~10 more through a single process. Voice memo transcription included.
- 🔁 **OpenAI-compat relay** — `serve --port 8080` exposes Chat Completions, Responses, and OAI-compat endpoints; drop in any OpenAI SDK.

---

## How GaussClaw is better than Hermes

| Capability | Hermes | **GaussClaw** | Why it matters |
|---|---|---|---|
| Tool capability gate | none | **kernel admit (`CapToken`)** | A misconfigured tool can't grow its blast radius. |
| Prompt injection containment | none | **schema gate at worker boundary, ≤ 2.19 %** | Untrusted tool output can't smuggle instructions to the next turn. |
| Tool sandbox | parent credentials | **WASM + Landlock + seccomp + bwrap** | Compromise probability bounded at ≤ 1.1 × 10⁻⁷. |
| Audit log | mutable SQLite | **Ed25519 + Merkle + TSA anchor** | Tampering leaves a cryptographic trail. |
| Taint tracking | none | **info-flow lattice + declassification** | Web/email observations can't reach high-privilege sinks. |
| Provider swap | manual retest | **polyhedral equivalence in CI** | Vendor migration is build-verified, not hoped-for. |
| Trajectory integrity | raw JSONL | **Cryptographic Envelope** | Downstream training data is provably authentic. |
| Cold start | 80–150 ms (Python import) | **≤ 10 ms** | Background tasks and webhooks feel instant. |
| Hybrid recall miss rate | 0.08 (FTS5) | **≤ 0.015** (BM25 ∪ HNSW) | Long-tail memory recall actually works. |
| Desktop installer | ~150 MB (Electron) | **≤ 20 MB** (Tauri 2) | Ten-times smaller, ten-times leaner. |
| Desktop RAM idle | ~250 MB | **≤ 80 MB** | Runs comfortably alongside your IDE. |
| Desktop signing | unsigned everywhere | **signed + notarised on 3 OSes** | macOS Gatekeeper, Windows SmartScreen, Linux signed AppImage. |
| Runtime dependencies | Python + Node.js | **one static binary** | `cp gaussclaw /usr/local/bin` is the install script. |

Every row is backed by a property test in
[`../gauss-aether/crates/gauss-conformance/`](../gauss-aether/crates/gauss-conformance/),
re-run on every PR.

---

## Install

```bash
# From source — the only option until the crates.io release.
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether
cargo install --path gaussclaw/crates/gaussclaw-bin

# Verify.
gaussclaw doctor
```

Pre-built signed installers for the desktop app live in
[Releases](https://github.com/rismanmattotorang/gauss-aether/releases).

### Migrate from Hermes in one command

```bash
gaussclaw import hermes /path/to/hermes-config.toml > gaussclaw.toml
```

The Hermes TOML keys, the `@tool` decorator ergonomics (preserved
behind a Rust `#[tool]` proc-macro), and the SFT/DPO export schema
are bit-identical. Replay a frozen 1,000-turn Hermes trajectory and
GaussClaw produces byte-equal output.

---

## Command reference

```bash
gaussclaw                              # interactive TUI
gaussclaw serve --port 8080            # web dashboard + OpenAI-compat relay
gaussclaw model                        # configure LLM provider
gaussclaw gateway                      # connect messaging channels
gaussclaw setup                        # first-run wizard
gaussclaw doctor                       # seven-invariant health check
gaussclaw import hermes <path>         # migrate a Hermes config
gaussclaw receipt verify <envelope>    # verify a signed trajectory
gaussclaw export sft  --since <ts>     # SFT trajectory with envelope
gaussclaw export dpo  --since <ts>     # DPO trajectory with envelope
gaussclaw skill list                   # show installed skills
gaussclaw skill add <manifest>         # install a Skill Manifest
```

---

## How it's built

Nineteen single-responsibility crates under [`crates/`](./crates/):

### User-facing surfaces

| Crate | Role |
|---|---|
| [`gaussclaw-bin`](./crates/gaussclaw-bin/) | The shipping static binary; wires every command. |
| [`gaussclaw-cli`](./crates/gaussclaw-cli/) | CLI subcommand surface (clap v4). |
| [`gaussclaw-tui`](./crates/gaussclaw-tui/) | Full-screen TUI (Ratatui + crossterm). |
| [`gaussclaw-web`](./crates/gaussclaw-web/) | Axum dashboard backend + embedded React/Vite/Tailwind frontend. |
| [`gaussclaw-desktop`](./crates/gaussclaw-desktop/) | Tauri 2 desktop shell (~10× smaller than the Hermes Electron app). |
| [`gaussclaw-surfaces`](./crates/gaussclaw-surfaces/) | REST · WebSocket · OpenAI-compat relay. |
| [`gaussclaw-channels`](./crates/gaussclaw-channels/) | ~20 messaging-channel adapters through one gateway. |

### Core loop

| Crate | Role |
|---|---|
| [`gaussclaw-agent`](./crates/gaussclaw-agent/) | Turn policy — kernel admit + audit + SAG approval. |
| [`gaussclaw-store`](./crates/gaussclaw-store/) | Session, turn, and lineage store on the Trinity backend. |
| [`gaussclaw-config`](./crates/gaussclaw-config/) | Hermes-compatible TOML configuration. |
| [`gaussclaw-migrate`](./crates/gaussclaw-migrate/) | `gaussclaw import hermes` migration driver. |

### Self-improvement (Gauss-Agent0)

| Crate | Role |
|---|---|
| [`gaussclaw-rsi`](./crates/gaussclaw-rsi/) | Live-backend wiring for the [`gauss-rsi`](../gauss-aether/crates/gauss-rsi/) engine: a SurrealDB-backed knowledge store, a `ProviderExpert` wrapping any vendor/OpenRouter driver as a frozen frontier expert, and the LinUCB router as a live `SelectionStrategy` for `NotDiamondProvider`. Closes the long-standing "no skill-synthesis loop" gap with a convergence-proven, verifier-gated, rollback-able loop. See [`../AGENT0_INTEGRATION.md`](../AGENT0_INTEGRATION.md). |

### Tools and skills

| Crate | Role |
|---|---|
| [`gaussclaw-skill`](./crates/gaussclaw-skill/) | Skill Manifest parser + `#[tool]` proc-macro. |
| [`gaussclaw-tools`](./crates/gaussclaw-tools/) | First-party tools — `base64`, `echo`, `file_*`, `hash`, `json_get`, `math_eval`, `regex_match`, `shell`, `upper`. Every tool runs under the Composite Sandbox by default. |

### Providers

| Crate | Role |
|---|---|
| [`gaussclaw-providers`](./crates/gaussclaw-providers/) | 20 leaf vendor drivers — Anthropic, OpenAI, Gemini, Cohere, Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI, Perplexity, Anyscale, OctoAI, HuggingFace, Replicate, Ollama, llama.cpp, vLLM, TGI. |
| [`gaussclaw-providers-meta`](./crates/gaussclaw-providers-meta/) | Meta-routers — OpenRouter (aggregator) and NotDiamond (learned router). |
| [`gaussclaw-api-modes`](./crates/gaussclaw-api-modes/) | OpenAI Chat-Completion · Responses · OAI-compat shims. |

### Trajectories and federation

| Crate | Role |
|---|---|
| [`gaussclaw-export`](./crates/gaussclaw-export/) | SFT/DPO writers + Cryptographic Trajectory Envelope + Taint-Aware Filter + `verify_envelope`. |
| [`gaussclaw-fed`](./crates/gaussclaw-fed/) | Federated Trajectory Pool client + reference server. |

### Testing

| Crate | Role |
|---|---|
| [`gaussclaw-conformance`](./crates/gaussclaw-conformance/) | Hermes-parity replay corpus, OpenAI SDK end-to-end, CLI parity diff, TUI snapshot tests, Playwright web e2e, `webdriverio + tauri-driver` desktop e2e. |

---

## Hermes parity, measured on every PR

GaussClaw's CI runs six parity gates alongside the runtime's axiom suite:

| Gate | What it checks |
|---|---|
| **Hermes replay** | A frozen 1,000-turn corpus produces byte-identical trajectory output. |
| **OpenAI SDK** | The official end-to-end suite is parametrised against both backends. |
| **CLI parity** | `gaussclaw --help` is diffed against a frozen Hermes `--help` corpus. |
| **TUI snapshot** | `insta` golden snapshots cover every documented Ratatui screen. |
| **Web e2e** | Playwright drives the React frontend against both backends. |
| **Desktop e2e** | `webdriverio + tauri-driver` drives all twelve Hermes-parity screens on macOS, Windows, and Linux. |

---

## Reference documents

| File | Purpose |
|---|---|
| [`SPEC.pdf`](./SPEC.pdf) | Architecture paper — Cryptographic Trajectory Envelope, polyhedral equivalence contract, 15-axis scorecard, the six Hermes-superiority axes. |
| [`ROADMAP.md`](./ROADMAP.md) | The build plan and historical phase log. |
| [`../gauss-aether/SPECS.md`](../gauss-aether/SPECS.md) | The runtime's normative spec — A1–A9, T1–T12. |
| [`../docs/HERMES_ADAPTER_MATRIX.md`](../docs/HERMES_ADAPTER_MATRIX.md) | Hermes-module → GaussClaw-crate mapping. |
