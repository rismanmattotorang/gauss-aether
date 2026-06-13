# GaussClaw

**The self-improving AI agent that ships as one file — and won't run code
you didn't authorise.**

GaussClaw is a Rust-native AI agent that lives on your laptop, your
phone, and your $5 VPS at the same time. It learns from every
conversation, builds skills from experience, plugs into 200+ language
models, and connects to Telegram, Discord, Slack, WhatsApp, and a dozen
other places your team already works.

Unlike every other agent in its class, GaussClaw can prove what it did.
Every tool call passes a capability check before it runs. Every turn is
signed and chained. Every export of training data carries a tamper-proof
receipt. Migration from Hermes is one command.

And it gets better over time *without touching a single model weight*.
The new **Gauss-Agent0** engine accumulates capability in an external,
verifiable knowledge-and-skill store composed from a pool of frozen
frontier models — a self-improvement loop with a convergence proof, a
verification gate on every fact it learns, and one-operation rollback.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![One static binary](https://img.shields.io/badge/runtime-static%20Rust-orange.svg)]()
[![Hermes parity](https://img.shields.io/badge/Hermes-byte--equal%20replay-brightgreen.svg)]()
[![Sandbox](https://img.shields.io/badge/sandbox-4--layer-blue.svg)]()
[![Receipts](https://img.shields.io/badge/audit-Ed25519%20%2B%20Merkle-blueviolet.svg)]()
[![Agent0 RSI](https://img.shields.io/badge/Agent0-weight--frozen%20RSI-ff69b4.svg)]()
[![Docs](https://img.shields.io/badge/docs-website-blue.svg)](website/)

---

## Lives where you do

One binary. Every surface.

- 💻 **Terminal** — full-screen TUI with multiline editing, slash-command autocomplete, conversation history, and streaming tool output.
- 🖥️ **Desktop** — signed, notarised installers for macOS, Windows, and Linux. ~20 MB on disk, ~80 MB RAM idle.
- 🌐 **Web dashboard** — same `gaussclaw serve` command spins up a React frontend and an OpenAI-compatible API relay.
- 📱 **Messaging** — Telegram, Discord, Slack, WhatsApp, Signal, Matrix, IRC, email, SMS through one gateway. Voice memos transcribed and replied to.
- 🔁 **OpenAI SDK** — point any existing OpenAI client at `http://localhost:8080/v1` and keep your code.

---

## Install

```bash
# Linux / macOS / WSL2 — from source
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether
cargo install --path gaussclaw/crates/gaussclaw-bin

# Verify
gaussclaw doctor
```

Desktop app installers live in
[Releases](https://github.com/rismanmattotorang/gauss-aether/releases) —
signed for Gatekeeper, SmartScreen, and Linux AppImage.

### Coming from Hermes? One command.

```bash
gaussclaw import hermes ~/.hermes/config.toml > gaussclaw.toml
```

Your Hermes TOML keys keep working. Your `@tool` decorators keep working
(behind a Rust `#[tool]` proc-macro). Your SFT/DPO export schema is
bit-identical. A frozen 1,000-turn Hermes corpus replays byte-for-byte.

---

## Get started

```bash
gaussclaw                          # talk to it (TUI)
gaussclaw model                    # pick a model
gaussclaw gateway                  # connect Telegram, Slack, Discord…
gaussclaw serve --port 8080        # web dashboard + API relay
gaussclaw setup                    # first-run wizard
gaussclaw doctor                   # health check
gaussclaw receipt verify <file>    # prove a trajectory is genuine
```

---

## What makes GaussClaw different

### 🧠 It remembers, and it learns

GaussClaw curates its own memory. After every complex task it can write
itself a skill — a piece of reusable know-how — and pull it back the next
time the situation comes up. It nudges itself to persist important
context, searches its past conversations with full-text and vector
recall together, and builds a model of who you are across sessions.

Hybrid recall miss rate: **≤ 1.5 %**. Hermes baseline: **8 %**.

### 🔁 It improves itself — and proves every step (Gauss-Agent0)

Most "self-improving" agents retrain model weights: expensive,
unauditable, easy to contaminate, hard to undo. GaussClaw's
**Gauss-Agent0** engine does the opposite. It keeps the models *frozen*
and accumulates everything it learns in an external, inspectable
knowledge-and-skill store — so improvement is a database you can read,
diff, and roll back, not a black box of shifted parameters.

Each improvement cycle:

- **routes** the task to the best subset of a heterogeneous model pool
  with a cost-aware contextual-bandit router (it beats its own best
  single model when the pool disagrees);
- **retrieves** supporting facts along two paths at once — a vector
  (semantic) path and a graph (premise-chain) path — and fuses them, so
  it can *compose* knowledge no single model held alone;
- **verifies** every candidate before admitting it: executable claims
  are run, factual claims must win a cross-provider quorum, and skills
  are admitted only with a statistical (PAC) competence guarantee;
- **gates on drift**: a goal-drift index is checked every cycle, and any
  regression rolls the store back to the last checkpoint in one
  operation.

The loop has a **convergence proof** (the knowledge gap contracts
geometrically), a **synergy guarantee** (under cross-model
complementarity the composed store strictly dominates every individual
model), and a **no-regret routing bound** — and every one of those
guarantees is a live, monitored number on the dashboard. Every admitted
fact carries provenance (which models, which premises, which verifier
tier, which cycle), so a self-improving agent finally has an audit trail.

This is the loop GaussClaw's memory system was always missing. See
[`AGENT0_INTEGRATION.md`](AGENT0_INTEGRATION.md) for the architecture and
the paper [`Gauss-Agent0-PaperV1.0.pdf`](Gauss-Agent0-PaperV1.0.pdf) for
the theory.

### 🛡️ Tools that can't go rogue

Every tool in GaussClaw declares a capability before it runs. The kernel
checks the capability against the active grant — and the grant can only
*shrink* over the lifetime of a turn, never grow. The type system
refuses to compile code that tries to widen a capability.

Tools then run inside a four-layer sandbox: WebAssembly, Linux
Landlock, seccomp filters, and bwrap (Seatbelt on macOS). Compromise
probability is bounded mathematically at one part in ten million.

By contrast, a Hermes tool runs in the host Python interpreter with
your full ambient credentials.

### 🚫 Resistant to prompt injection by design

When a tool returns text — from a web page, a PDF, an email — GaussClaw
runs the output through a four-stage schema gate before any of it
touches the next prompt. Untrusted instructions can't smuggle themselves
back into the conversation.

Measured prompt-injection success rate on a standard corpus:
**≤ 2.19 % theoretical, 0 / 20 empirical.** Hermes has no defence
mechanism at this layer.

### 🧾 An audit log that holds up in court

Every turn — input, model output, tool calls, approvals — is hashed
into a Merkle chain, signed with Ed25519, and anchored to an RFC 3161
timestamp authority every thousand entries. Mutating any past record
breaks the chain head; the signature catches the substitution.

GaussClaw ships a `receipt verify` command. Hermes ships a mutable
SQLite file.

### 🔌 Any model. No lock-in.

Twenty first-party vendor drivers: Anthropic, OpenAI, Google Gemini,
Cohere, Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI,
Perplexity, Anyscale, OctoAI, HuggingFace, Replicate, Ollama,
llama.cpp, vLLM, TGI. Plus OpenRouter as an aggregator and NotDiamond
as a learned router.

Switch with `gaussclaw model`. Before the swap commits, the polyhedral
verifier checks the new provider is behaviourally equivalent to the old
one on a probe set — so a model change can't quietly regress your
deployment.

### 📤 Train on your own conversations, provably

GaussClaw exports SFT and DPO trajectories with a **Cryptographic
Trajectory Envelope** — every record carries the original receipt, a
position witness in the Merkle chain, and the TSA anchor. Anyone
downstream can verify the trajectory was produced by a real conversation
on a real GaussClaw instance, not synthesised after the fact.

Optional differential-privacy noise. Federated trajectory pool included.

### ⏱️ Bounded everything

Runaway work is a failure mode, not a surprise. Every executor backend
(local, Docker, SSH) honours a per-request wall-clock timeout — a hung
tool is killed and reaped, never left pinning a slot. The agent loop
caps both its iterations *and* its cumulative tool calls per turn, so a
misbehaving provider can't drive unbounded execution. Webhook signature
parsing is constant-time and panic-free on adversarial input, and the
TUI transcript buffer prunes itself instead of growing without bound.

### 🦀 One binary. No interpreter. No surprises.

`gaussclaw` is one static Rust binary. No Python at runtime. No
Node.js at runtime. No Chromium bundled with the desktop app. About
**20 MB** to install, **80 MB** of RAM at idle, and **10 ms** to
cold-start a turn.

The desktop app is about a tenth the size and a third the memory of
the Hermes Electron build.

### 🏗️ Build your own agent on the same engine

If GaussClaw isn't the agent you want, embed the engine directly.
**Gauss-Aether** — the runtime underneath — is a Rust SDK with a clean
plugin trait surface (`Kernel`, `MemoryBackend`, `Provider`,
`SandboxTrait`, `ToolTrait`). Build a code reviewer, a research
notebook driver, an infra-automation worker — whatever you need — and
inherit every safety property GaussClaw has.

See [`gauss-aether/README.md`](gauss-aether/README.md) and
[`docs/QUICKSTART.md`](docs/QUICKSTART.md).

---

## GaussClaw vs. Hermes at a glance

| | Hermes | **GaussClaw** |
|---|---|---|
| Runtime | Python + Node.js | **Single static Rust binary** |
| Desktop installer | ~150 MB (Electron) | **~20 MB (Tauri 2)** |
| Desktop RAM idle | ~250 MB | **~80 MB** |
| Cold start | 80–150 ms | **≤ 10 ms** |
| Tool sandbox | parent credentials | **WASM + Landlock + seccomp + bwrap** |
| Capability check | none | **Kernel admit gate, monotone shrink** |
| Prompt injection containment | none | **≤ 2.19 % (0/20 empirical)** |
| Audit log | mutable SQLite | **Ed25519 + Merkle + TSA anchor** |
| Provider swap | manual retest | **Polyhedral equivalence verified in CI** |
| Trajectory exports | raw JSONL | **Cryptographic envelope, downstream-verifiable** |
| Hybrid recall miss rate | ~8 % | **≤ 1.5 %** |
| Self-improvement | weight retraining (or none) | **Weight-frozen RSI: verified knowledge/skill store, convergence-proven, rollback-able** |
| Learned facts auditability | n/a | **Per-item provenance: models, premises, verifier tier, cycle** |
| Code signing on desktop | unsigned | **Signed + notarised on 3 OSes** |
| Migration from Hermes | n/a | **One command** |

Every claim above is backed by property tests in the conformance
crates; the full workspace suite — ~1,400 tests — re-runs on every PR.

---

## Under the hood

GaussClaw is the agent. **Gauss-Aether** is the engine.

The repository ships both: 26 `gaussclaw-*` crates (the agent surfaces,
channels, tools, providers, exporters) on top of 29 `gauss-*` crates
(the kernel, the turn engine, the memory store, the audit chain, the
sandbox, the verifier, and `gauss-rsi` — the Gauss-Agent0
self-improvement engine).

If you want to know the agent's user-facing capabilities:
**[`gaussclaw/README.md`](gaussclaw/README.md)**.

If you want to embed the engine in your own product:
**[`gauss-aether/README.md`](gauss-aether/README.md)**.

If you want to read about the safety story — the nine axioms, twelve
theorems, Lean 4 proof skeleton, polyhedral verifier, the
property-test harness — start with
[`gauss-aether/SPECS.md`](gauss-aether/SPECS.md) and the
[architecture paper](gauss-aether/Gauss-Aether.pdf).

---

## Documentation

- [`AGENT0_INTEGRATION.md`](AGENT0_INTEGRATION.md) — the Gauss-Agent0 self-improvement engine: assessment, phased strategy, and `gauss-rsi` module map.
- [`docs/QUICKSTART.md`](docs/QUICKSTART.md) — embed walkthrough.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — crate-by-crate tour.
- [`docs/SECURITY.md`](docs/SECURITY.md) — threat model and disclosure.
- [`docs/HERMES_ADAPTER_MATRIX.md`](docs/HERMES_ADAPTER_MATRIX.md) — Hermes-module → GaussClaw-crate mapping.
- [`docs/adr/`](docs/adr/) — sixteen architecture decision records.
- [`website/`](website/) — full documentation site (English + Simplified Chinese).

---

## Contributing

See [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md). Pull requests are
welcome on every crate; Tier-0 changes (`gauss-kernel`, `gauss-audit`,
`gauss-attest`) require dual review.

---

## Licence

MIT — see [`LICENSE-MIT`](LICENSE-MIT).

---

## Citing

```bibtex
@software{gauss_aether_2026,
  author  = {Gauss-Aether Contributors},
  title   = {GaussClaw: A Verifiable Self-Improving AI Agent},
  year    = 2026,
  url     = {https://github.com/rismanmattotorang/gauss-aether},
  license = {MIT}
}
```
