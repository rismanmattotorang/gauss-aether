---
id: first-run
title: First run
sidebar_position: 2
---

# First run

After installing, three ways to use GaussClaw:

## TUI (default)

```bash
gaussclaw
```

Launches the Ratatui + crossterm shell. Type a message, press `Enter`
to submit, `Shift+Enter` to insert a newline, `Ctrl+C` to quit. Try
`/help` for the slash-command surface.

The status bar surfaces what the upstream Hermes Ink TUI cannot: the
live audit-chain head, the taint floor, the capability set. Three
GaussClaw-only commands inspect them:

- `/receipt` — current chain head + Merkle proof
- `/taint` — current taint floor + per-token labels
- `/caps` — current capability set
- `/sandbox` — per-tool sandbox layer status

## Web dashboard

```bash
gaussclaw web --port 8642
```

Starts the Axum backend. Open <http://127.0.0.1:8642/> in any browser.
The React frontend runs natively on Linux, macOS, and Windows — no PTY,
no WSL2 requirement.

## Desktop app

Install the platform-native installer from the
[Releases page](https://github.com/rismanmattotorang/gauss-aether/releases),
then launch **GaussClaw** from your OS launcher. The Tauri 2 binary
holds the same agent state as the web dashboard and the TUI — they
all share one `ServerState`.
