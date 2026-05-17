---
id: first-run
title: First run
sidebar_position: 2
---

# First run

After installing, you have four ways to use GaussClaw — and they all
share the same conversation, the same memory, and the same audit
chain. Open the TUI, message a Telegram bot, hit the OpenAI-compatible
API, and pick up the desktop app — you'll see the same thread.

## TUI (default)

```bash
gaussclaw
```

Launches the Ratatui shell. Press `Enter` to submit, `Shift+Enter` to
insert a newline, `Ctrl+C` to quit. Try `/help` for the full
slash-command surface.

The status bar shows live state the upstream Hermes Ink TUI can't:

| Command | What it shows |
|---|---|
| `/receipt` | Current chain head + Merkle proof for the last turn. |
| `/taint` | Current taint floor + per-token labels. |
| `/caps` | Current capability set granted to the active session. |
| `/sandbox` | Per-tool sandbox layer status (WASM / Landlock / seccomp / bwrap). |

## Web dashboard

```bash
gaussclaw serve --port 8080
```

Starts the Axum backend with the embedded React frontend. Open
[http://127.0.0.1:8080/](http://127.0.0.1:8080/) in any browser. The same port also exposes:

- `/v1/chat/completions` and `/v1/responses` — OpenAI-compatible API.
- `/ws` — WebSocket for streaming.
- `/api/receipts/{head,verify}` — receipt-chain inspection.

The React frontend runs natively on Linux, macOS, and Windows — no
PTY, no WSL2 requirement, no Electron.

## Desktop app

Install the platform-native installer from the
[Releases page](https://github.com/rismanmattotorang/gauss-aether/releases),
then launch **GaussClaw** from your OS launcher. The Tauri 2 binary
shares one `ServerState` with the TUI and the web dashboard — they
all see the same conversation, the same memory, and the same chain
head.

Installer signing:

| OS | Signed by | Mechanism |
|---|---|---|
| macOS | Apple Developer ID | Notarised; runs without Gatekeeper warning. |
| Windows | Authenticode | Runs without SmartScreen prompt. |
| Linux | GPG + Ed25519 | AppImage signature + chain-anchored SHA-256. |

## Connect a messaging channel

```bash
gaussclaw gateway start
```

Walks you through connecting Telegram, Discord, Slack, WhatsApp,
Signal, Matrix, IRC, email, or SMS. You can also drop credentials
into `[channels.*]` in your `gaussclaw.toml` directly. Voice memos
are transcribed automatically.

## Verify health

```bash
gaussclaw doctor
```

Runs the seven Self-Diagnostic Health Engine invariants from
`gauss-health`. A green report means the kernel, the memory store,
the sandbox, the audit chain, and every active surface are coherent.

```bash
gaussclaw doctor --json | jq
```

The `--json` form is what you'd put in a Kubernetes liveness probe or
a Nagios check.
