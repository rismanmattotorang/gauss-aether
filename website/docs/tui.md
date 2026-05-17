---
id: tui
title: TUI reference
sidebar_position: 5
---

# TUI reference

The interactive terminal shell. Built with [Ratatui](https://ratatui.rs)
+ [crossterm](https://github.com/crossterm-rs/crossterm) + tui-textarea
— **no Node runtime**, ~10× smaller binary than the upstream Hermes
React + Ink TUI.

## Layout

```
 ┌────────────────────────── GaussClaw v0.0.1 ─────────────────────────────┐
 │ session=…  model=…  turn=…  chain=…  taint=⊥  caps=…                    │ ← status bar
 ├──────────────────────────────────────────────────────────────────────────┤
 │ history pane (scrollable)                                                │
 ├──────────────────────────────────────────────────────────────────────────┤
 │ > input area (multiline; Shift+Enter newline)                            │
 └──── Enter submit · Ctrl+C quit · Ctrl+L clear · /help help ──────────────┘
```

## Keybindings

| key | action |
|---|---|
| `Enter` | Submit |
| `Shift+Enter` | Newline |
| `Ctrl+C` / `Ctrl+D` | Quit |
| `Ctrl+L` | Clear history |
| `PageUp` / `PageDown` | Scroll history |
| `Tab` | Apply completion (planned) |

## Slash commands

Phase 1 implements `/help`, `/quit`, `/exit`, `/clear`, `/new`. The
following are recognised today and stub-respond with the phase that
fills them:

`/receipt`, `/taint`, `/caps`, `/sandbox`, `/model`, `/tools`,
`/config`, `/logs`, `/statusbar`, `/queue`, `/undo`, `/retry`,
`/copy`, `/paste`, `/details`, `/compact`, `/resume`.

## GaussClaw-only status bar fields

Three fields the upstream Hermes Ink TUI cannot display:

- `chain=<hex>` — first 8 hex chars of the live receipt chain head.
  Advances on every turn (WAL-before-effect).
- `taint=<label>` — current taint floor for the session. `⊥` /
  `user` / `web` / `adversarial`.
- `caps=<n>` — count of granted capabilities.
