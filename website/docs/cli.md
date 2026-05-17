---
id: cli
title: CLI reference
sidebar_position: 4
---

# CLI reference

```bash
gaussclaw [OPTIONS] [COMMAND]
```

Global options apply to every subcommand:

| flag | description |
|---|---|
| `-c, --config <PATH>` | Alternate `gaussclaw.toml` |
| `-v, --verbose...` | Repeatable verbosity bump |
| `-q, --quiet` | Suppress non-error output |
| `--help` | Per-subcommand help text |

## Hermes-parity subcommands

| GaussClaw | Hermes upstream | Status |
|---|---|---|
| `gaussclaw` (no args) | `hermes` | Launches the TUI |
| `gaussclaw model {list,show,set}` | `hermes model` | Provider plane (Phase 4) |
| `gaussclaw tools {list,show,enable,disable}` | `hermes tools` | Skill Manifest (Phase 3) |
| `gaussclaw config {list,get,set,path}` | `hermes config` | Phase 1 ✓ |
| `gaussclaw gateway {start,stop,status}` | `hermes gateway` | Channels foundation ✓ |
| `gaussclaw setup` | `hermes setup` | Phase 1 |
| `gaussclaw update` | `hermes update` | Tauri updater (Phase 5) |
| `gaussclaw doctor` | `hermes doctor` | SDHE (Phase 1) |

## GaussClaw extensions

| Subcommand | Purpose |
|---|---|
| `gaussclaw chat [-m TEXT] [-s ID]` | One-shot chat REPL without the full TUI |
| `gaussclaw import <hermes-config>` | Migrate a Hermes deployment |
| `gaussclaw receipt {head,verify}` | Inspect the receipt chain or verify an envelope |
| `gaussclaw web [--host HOST] [--port PORT] [--open]` | Launch the Axum dashboard backend |

## Parity gate

The conformance suite (`gaussclaw-conformance`) carries a frozen `--help`
corpus that locks the surface against accidental drift. Every PR runs:

1. Every subcommand parses.
2. The `SUBCOMMANDS` table matches the clap-derived surface.
3. Every Hermes subcommand is covered (no over-claim, no missing entries).
4. `insta` snapshots of every `--help` page match the locked baseline.

See [`crates/gaussclaw-conformance/src/cli_parity.rs`](https://github.com/rismanmattotorang/gauss-aether/blob/main/crates/gaussclaw-conformance/src/cli_parity.rs).
