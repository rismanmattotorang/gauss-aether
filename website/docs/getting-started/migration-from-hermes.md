---
id: migration-from-hermes
title: Migration from Hermes
sidebar_position: 3
---

# Migration from Hermes

GaussClaw is a **superset** of Hermes — every Hermes config key, tool,
and surface continues to work, and new GaussClaw-only keys are optional
and namespaced.

## Step 1 — import the config

```bash
gaussclaw import ~/.config/hermes/config.toml
```

Writes a `gaussclaw.toml` next to your Hermes config (or to `-o <PATH>`
if specified). Hermes top-level keys (`[provider]`, `[surfaces.*]`,
`[channels.*]`, `[tools.*]`) are copied verbatim; GaussClaw extensions
(`[caps]`, `[taint]`, `[export]`, `[desktop]`) are omitted so the
import is reversible.

## Step 2 — opt in to extensions, one section at a time

```toml
# Phase 3 deliverable: capability gates.
[caps]
default_grant = ["fs:read:./data", "network:http_get"]

# Phase 3 deliverable: declassification policy.
[taint]
default_declass = "default"   # "default" | "strict"

# Phase 5 deliverable: trajectory export with envelopes.
[export]
filter_mode = "declassified"  # "permissive" | "strict" | "declassified"
envelopes   = true

# Phase 1 deliverable: desktop runtime options.
[desktop]
global_hotkey = true
autostart     = false
```

Each section is opt-in; omitting a section keeps the corresponding
GaussClaw subsystem in Hermes-compatible mode.

## Step 3 — verify with `gaussclaw doctor`

```bash
gaussclaw doctor --json
```

Returns the Self-Diagnostic Health Engine report. Any `red` invariant
points to a config / install / kernel-state mismatch.

## What stays the same

- Every Hermes subcommand: `model`, `tools`, `config`, `gateway`,
  `setup`, `update`, `doctor`.
- Every `@tool`-decorated function (re-exposed as `#[tool]` in Rust;
  Python tools run through the legacy shim during the transition).
- The SFT / DPO trajectory wire schema (`prompt`, `completion`,
  `surface`, `session_id`, `parent_id`, `ts`, lineage edges, DPO
  pairs) — bit-for-bit. New material lands in an optional envelope
  alongside each record.
- Every messaging channel.

## What is strictly better

- The receipt chain is **tamper-evident** (theorem T3 of
  GaussClaw.pdf).
- Tool dispatch is **admit-gated** and **sandboxed** (T9, T10).
- The desktop app runs **without PTY** on Linux / macOS / Windows
  natively.
- Every release is **code-signed and chain-anchored**.

See the [roadmap](https://github.com/rismanmattotorang/gauss-aether/blob/main/GAUSSCLAW_ROADMAP.md)
§ "Target numbers (at GA)" for the full Hermes-baseline-vs-GA table.
