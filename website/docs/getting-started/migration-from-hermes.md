---
id: migration-from-hermes
title: Migration from Hermes
sidebar_position: 3
---

# Migration from Hermes

GaussClaw is a **superset** of Hermes. Every Hermes config key, every
`@tool`-decorated function, every CLI subcommand, and every messaging
channel keeps working. GaussClaw-only additions are namespaced and
opt-in.

Migration is one command — and it's reversible.

## Step 1 — import the config

```bash
gaussclaw import hermes ~/.config/hermes/config.toml
```

Writes a `gaussclaw.toml` next to your Hermes config (or to `-o <PATH>`
if specified). Hermes top-level keys (`[provider]`, `[surfaces.*]`,
`[channels.*]`, `[tools.*]`) are copied verbatim. GaussClaw extensions
(`[caps]`, `[taint]`, `[export]`, `[desktop]`) are omitted, so a stock
import is byte-for-byte equivalent to your Hermes deployment.

## Step 2 — start with Hermes-compatible defaults

```bash
gaussclaw                  # TUI — same prompt, same tools, same surface
gaussclaw serve            # web dashboard + OAI-compat relay on :8080
gaussclaw gateway start    # bring the messaging channels online
```

At this point GaussClaw is running everything Hermes ran, the way Hermes
ran it — just on a kernel that admit-gates tool calls, sandboxes
execution, and signs every turn. The replay-corpus conformance gate
checks for byte-identical trajectories against a frozen 1,000-turn
Hermes baseline; if it passes in CI, it'll pass for your workload.

## Step 3 — opt in to GaussClaw extensions

Each section is independent. Turn them on one at a time as you build
confidence.

### Capability gates

```toml
[caps]
default_grant = ["fs:read:./data", "network:http_get"]
```

Restricts the default capability set every tool inherits. Any tool that
asks for a capability outside this grant fails to register.

### Taint declassification policy

```toml
[taint]
default_declass = "default"   # "default" | "strict"
```

`strict` refuses to lower any taint without an explicit declassification
rule. `default` applies the standard antitone map from the SPECS.

### Trajectory envelopes

```toml
[export]
filter_mode = "declassified"  # "permissive" | "strict" | "declassified"
envelopes   = true
```

When `envelopes = true`, every SFT/DPO record carries the original
receipt, a Merkle position witness, and the TSA anchor — so downstream
consumers can verify the trajectory was produced by a real agent run.

### Desktop options

```toml
[desktop]
global_hotkey = true
autostart     = false
```

## Step 4 — verify

```bash
gaussclaw doctor --json
```

The Self-Diagnostic Health Engine checks the seven minimum invariants
plus any custom ones you've registered. Any non-green row points to a
config, install, or kernel-state issue.

```bash
gaussclaw receipt head
```

Prints the current chain head, signature, and most recent TSA anchor.
Save it somewhere safe — you can later prove the chain hasn't been
rewritten by checking the head still extends.

## What stays the same

- Every Hermes subcommand: `model`, `tools`, `config`, `gateway`,
  `setup`, `update`, `doctor`.
- Every `@tool`-decorated function — re-exposed as `#[tool]` in Rust;
  Python tools run through the legacy shim during the transition.
- The SFT / DPO trajectory wire schema (`prompt`, `completion`,
  `surface`, `session_id`, `parent_id`, `ts`, lineage edges, DPO
  pairs) — bit-for-bit. New material lands in an optional envelope
  alongside each record.
- Every messaging channel.

## What is strictly better

- The receipt chain is **tamper-evident** — mutating any past entry
  diverges the chain head and fails Ed25519 verification.
- Tool dispatch is **admit-gated** and **sandboxed** — Pr[compromise]
  bounded at one part in ten million.
- The desktop app runs **without PTY** on Linux / macOS / Windows
  natively, in ~80 MB of RAM instead of ~250 MB.
- Every release is **code-signed and chain-anchored** — the Tauri
  updater verifies both the certificate and the chain inclusion before
  applying any update.

## Rolling back

If GaussClaw isn't right for you, the original Hermes config is
untouched on disk — switch back by running Hermes against it.
Trajectories exported with `envelopes = false` are bit-identical to
Hermes output. Migration is reversible.
