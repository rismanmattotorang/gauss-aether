---
id: intro
title: Welcome
slug: /intro
sidebar_position: 1
---

# Welcome to GaussClaw

**GaussClaw** is a self-improving, Hermes-compatible AI agent that ships
as a single static Rust binary. It runs on your laptop, your phone, and
your $5 VPS — every surface, every safety guarantee, one executable.

Underneath, GaussClaw runs every turn through the
[Gauss-Aether](https://github.com/rismanmattotorang/gauss-aether/tree/main/gauss-aether)
kernel — a runtime where safety invariants are mechanically checked
rather than asked for in a prompt. The result is an agent that does
everything Hermes does, the way Hermes does it, and can *prove* what
it did when you ask.

## What you can do with it

- **Talk to it** from a terminal, a desktop app, a web dashboard, or
  any messaging platform your team already uses — Telegram, Discord,
  Slack, WhatsApp, Signal, Matrix, IRC, email, SMS.
- **Pick any model** from twenty first-party vendor drivers (Anthropic,
  OpenAI, Gemini, Mistral, Groq, …) plus OpenRouter and NotDiamond as
  meta-routers. Switch with `gaussclaw model`.
- **Drop in your existing OpenAI SDK** code — `gaussclaw serve` exposes
  an OpenAI-compatible relay on localhost.
- **Migrate from Hermes in one command** — `gaussclaw import hermes`
  reads your TOML config and produces a working `gaussclaw.toml`.
- **Build your own agent** on the same kernel by depending on the
  `gauss-traits` crates directly.

## What's different from Hermes

| | Hermes | GaussClaw |
|---|---|---|
| Runtime | Python + Node.js | **Single static Rust binary** |
| Tool sandbox | parent credentials | **WASM + Landlock + seccomp + bwrap** |
| Capability check | none | **Kernel admit gate, monotone shrink** |
| Audit log | mutable SQLite | **Ed25519 + Merkle + TSA anchor** |
| Provider swap | manual retest | **Polyhedral equivalence in CI** |
| Desktop installer | ~150 MB (Electron) | **~20 MB (Tauri 2)** |
| Cold start | 80–150 ms | **≤ 10 ms** |

Every claim above is backed by a property test in the conformance
suite — 299 tests, ~3 seconds, re-run on every PR. See
[architecture](./architecture) for the full mapping.

## Where to go next

- 🚀 [**Install GaussClaw**](./getting-started/installation) — one command.
- 🎬 [**First run**](./getting-started/first-run) — start the TUI, the dashboard, the desktop app.
- 🔁 [**Migrate from Hermes**](./getting-started/migration-from-hermes) — keep your config and your tools.
- 🛠️ [**CLI reference**](./cli) — every subcommand with examples.
- 🏗️ [**Architecture**](./architecture) — how the safety story is built.
