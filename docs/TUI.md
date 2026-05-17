# `gauss-tui` — Ratatui admin console

The `gauss-tui` binary is the TUI admin interface for Gauss-Aether. It
binds to a live in-process engine (kernel + memory + SAG + signer +
health + cluster ring) and provides ten tabs covering administration,
configuration, and monitoring.

```
┌─ Gauss-Aether · 1.0 ─────────────────────────────────────────────────────────┐
│ 1 Dashboard  2 Turns  3 Memory  4 Sandbox  5 SAG  6 Health  7 Cluster …      │
└──────────────────────────────────────────────────────────────────────────────┘
┌── chain length ──┐┌─ pending approvals ─┐┌─── health ───┐┌──── taint ────┐
│        7         ││          1          ││      OK      ││     User      │
└──────────────────┘└─────────────────────┘└──────────────┘└───────────────┘
┌── Recent turns ────────────────────────────┐┌─ System ──────────────────────┐
│ #1     1 actions  User    a3b…f12           ││ workspace        22 crates    │
│ #2     1 actions  User    8d4…cc1           ││ tests            299 passing  │
│ #3     1 actions  Web     4e2…b9a           ││ kernel grant     0xFFFF…      │
│ ...                                          ││ chain head       a3b…f12      │
└──────────────────────────────────────────────┘│ license          MIT          │
                                                │ cluster          3 nodes      │
                                                │ attestor         SoftwareSim  │
                                                └───────────────────────────────┘
─ [t] run turn  [r] refresh  [a] seed approval  [/] taint  ·  [?] help · [q] quit
```

## Launch

```bash
cargo run -p gauss-tui --release
```

The TUI boots a fresh in-memory engine seeded with three demo cluster
nodes (`gauss-1.eu-west`, `gauss-2.eu-west`, `gauss-3.us-east`). Run a
turn or seed an approval to populate the state from the Dashboard.

## Tabs

| # | Tab        | What it shows / does                                                                       |
|---|------------|--------------------------------------------------------------------------------------------|
| 1 | Dashboard  | KPI cards (chain length, pending approvals, health verdict, current taint). Recent turns list. System info table. `t` runs a demo turn through the live engine, `a` seeds a demo SAG approval, `/` cycles the default taint band. |
| 2 | Turns      | Chronological turn list with the most recent first. Left panel scrolls with `↑↓` / `jk`; right panel shows the highlighted turn's full detail including chain head hex, taint band, and any `SagDecisionRecord` bundles. |
| 3 | Memory     | Chain head + length, FTS analyzer config, HNSW index parameters, BM25 / hybrid recall surface description. Lists optional backends (`kv-mem`, `kv-surrealkv`, `kv-rocksdb`). |
| 4 | Sandbox    | Cap → class mapping table from `gauss_traits::min_sandbox_for`. Layers per cap depth (WASM, Landlock, namespace, seccomp, TEE). |
| 5 | SAG        | Live pending-approval queue (top-left). Default decision table summary (top-right). Selected request detail (bottom-right). `a` approves the highlighted request, `d` denies it, `s` seeds a demo request, `jk` navigates. |
| 6 | Health     | Overall gauge + per-invariant list with verdict symbols (`✓` OK, `!` warning, `✗` failing). `r` re-evaluates against the live subject; `g` forces `grant = 0` so operators can see what a failing invariant looks like. |
| 7 | Cluster    | Consistent-hash ring status + active node list + live "route this session" preview. `n` adds an auto-named node, `x` removes the most-recently-added node, `r` re-routes the test session key. |
| 8 | Audit      | Receipt-chain summary — chain primitive, signing scheme, anchor cadence, anchor kinds, verifier-API entry points, current chain length + head digest. |
| 9 | Scorecard  | 15-axis Pareto-dominance comparison vs the highlighted predecessor (OpenClaw / ZeroClaw / OpenFang / Hermes). Per-axis Δ symbol (`▲` better, `=` equal, `▼` worse). `← →` cycles the predecessor. |
| 0 | Logs       | Rolling buffer of in-process events. `c` clears. |

## Global keybindings

| Key                | Action                                |
|--------------------|---------------------------------------|
| `Tab` / `Shift+Tab`| Cycle tabs.                           |
| `1`–`9`, `0`       | Jump to tab by number.                |
| `?`                | Toggle help overlay.                  |
| `q` / `Ctrl-C`     | Quit cleanly (restores the terminal). |
| `r`                | Refresh polled state on the active tab. |
| `↑ ↓` / `j k`      | Navigate lists on tabs that support it. |

## What's live vs what's mocked

- **Live**: the kernel, memory backend, SAG approval gate, health engine,
  cluster ring, scorecard, canvas, log buffer, and turn engine are
  real workspace crates running in-process. Pressing `t` on the
  Dashboard runs an actual `TurnEngine::run_turn` — the chain head
  advances; the signed receipt is real; pending approvals (when the
  classifier escalates) flow through the live `ChannelSurface` and
  back into the `pending` queue.
- **Mocked**: the toy provider always emits the same text action, so
  the SAG never escalates a real turn to `RequireApproval` on the
  defaults. Use `a` on the Dashboard or `s` on the SAG tab to seed a
  pretend request so you can practise the approval round-trip.
- **Demo-only**: the `g` key on the Health tab writes `grant = 0`
  into the *health subject* (not the kernel) so operators can see
  what a failing-invariant view looks like.

## Architecture

```text
crates/gauss-tui/
├── Cargo.toml                # ratatui 0.28 + crossterm 0.28
└── src/
    ├── main.rs               # tokio entry, terminal setup, event loop
    ├── app.rs                # App state: engine, kernel, memory, SAG,
    │                         # health, ring, scorecards, queues, logs
    └── ui.rs                 # one draw fn per tab + help overlay
```

The app spawns one background task at boot — a drain that copies every
incoming `ApprovalRequest` from the SAG `ChannelSurface` into the
visible `pending` queue. When the operator presses `a` or `d` in the
SAG tab, the `App::decide_pending` method pops the highlighted entry
and pushes the verdict back over the cloned `decision_tx` sender. The
SAG `ApprovalGate` resumes its `request_approval` future with the
decision and the turn either commits or short-circuits.

## Customising / embedding

The boot sequence in `app.rs::App::boot` is the integration point. To
wire `gauss-tui` against your own deployment:

1. Swap `SurrealMemory::open_in_memory` for `SurrealMemory::open` (a
   `kv-surrealkv` / `kv-rocksdb` backend; needs the matching feature
   flag).
2. Swap `ToyProvider` for one of the production provider adapters
   that ship as Phase-12 plugin crates.
3. Swap `Ed25519Signer::from_seed` for a `SigningBackend` that reads
   the operator's KMS / HSM / OS keyring.
4. Swap `AutoApprove` or the `ChannelSurface` shim for a production
   `ApprovalSurface` (Telegram / Slack / Discord / Matrix adapter).

No other code changes; the TUI re-uses your engine via `Arc<TurnEngine
<K, M, P>>` directly.

## Screenshots-as-text

Approval surface in action:

```
┌─ Approval queue (1 pending) ──────────────────┐┌─ Decision table (paper §XI.B) ───┐
│▶ #1000  RequireApproval  send_email           ││ adversarial taint  → Deny         │
│                                               ││ CRYPTO_SIGN        → RequireApproval│
│                                               ││ ¬rev ∧ (NET_POST ∨ SPAWN) → R.A.  │
│                                               ││ ¬rev ∨ Web taint   → Notify       │
│                                               ││ else               → Auto         │
└───────────────────────────────────────────────┘└──────────────────────────────────┘
                                                ┌─ Request detail — [a] approve · [d] deny ─┐
                                                │ turn          1000                         │
                                                │ tool          send_email                   │
                                                │ cap_required  0x0000000000000008           │
                                                │ reversible    no                           │
                                                │ risk          RequireApproval              │
                                                │ reason        non_reversible_high_impact   │
                                                │ args          { "to": "ops@example.com" }  │
                                                └────────────────────────────────────────────┘
```

15-axis Pareto-dominance check:

```
┌─ 15-axis Pareto-dominance (vs `hermes`) — [←→] cycle predecessor ──────────────────┐
│ Axis                       1.0       hermes      Δ                                  │
│ cold_start_ms              9.00       15.00     ▲                                   │
│ warm_hit_ratio             0.95        0.85     ▲                                   │
│ ipi_containment            1.00        0.85     ▲                                   │
│ sandbox_depth              5.00        3.00     ▲                                   │
│ receipt_strength           1.00        0.80     ▲                                   │
│ recall_miss                0.02        0.05     ▲                                   │
│ ...                                                                                  │
│ tee_attestation            1.00        0.00     ▲                                   │
│ license_clarity            1.00        0.50     ▲                                   │
└──────────────────────────────────────────────────────────────────────────────────────┘
```
