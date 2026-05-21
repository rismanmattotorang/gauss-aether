# GaussClaw — Hermes-to-Rust Porting Roadmap

**Companion to:** `GaussClaw.pdf`, `Gauss-Aether.pdf`, `SPECS.md`, `ROADMAP.md`
**Mission:** Port every Python module of [Hermes](https://github.com/nousresearch/hermes-agent) into a single Rust binary `gaussclaw` that runs on the Gauss-Aether kernel, **preserving Hermes surface convergence and SFT/DPO export semantics bit-for-bit** while closing the five Hermes architectural deficits with kernel-grade discipline (capability, accountability, information-flow, isolation, fairness).
**Cadence target:** 5 phases / 24 weeks / 4 milestones + GA — mirroring `GaussClaw.pdf` §V and operationalised against the existing Gauss-Aether 1.0 crate surface.

---

## Binding Constraints

These are **non-negotiable** for the duration of the port:

1. **Surface-Convergence Preservation (Principle 1).** For every Hermes surface σᵢ and every well-formed message *m*, the resulting GaussClaw trajectory record must contain — modulo cryptographic envelope — the identical conversation state, lineage edge, and provider response as the corresponding Hermes record.
2. **Trajectory schema bit-equality.** The SFT/DPO JSONL wire shape (`prompt`, `completion`, `surface`, `session_id`, `parent_id`, `ts`, lineage edges, DPO pairs) is preserved field-for-field. New material is *appended* in an optional envelope, never inlined into existing fields.
3. **Decorator ergonomics.** The Hermes `@tool` authoring surface survives literally. The Skill Manifest is a non-breaking *addition*.
4. **TOML config compatibility.** A Hermes deployment's top-level config keys continue to work. New keys (caps, taint, fallback chains, meta-routers) are *optional*.
5. **No axiom regression.** No PR ships that breaks any A1–A9 / T1–T12 conformance test already green in `gauss-conformance`.

Every PR must reference (a) the Hermes module being ported, (b) the Gauss-Aether axiom/theorem it lands under, and (c) the GaussClaw phase milestone (M1–M4/GA).

---

## Existing Substrate (already shipped in Gauss-Aether 1.0)

The Rust runtime under GaussClaw is **already built**. The port reuses these crates verbatim:

| Concern | Crate | Status |
|---|---|---|
| Capability lattice 𝒦, info-flow lattice ℒ, joint admit | `gauss-kernel`, `gauss-core` | ✅ Phase 1 |
| Differential Turn Engine, WAL-before-effect | `gauss-turn` | ✅ Phase 2 |
| Trinity Memory (SurrealDB: KV + doc + graph + FTS + HNSW + Merkle) | `gauss-memory` | ✅ Phase 1, 6 |
| Composite Sandbox (WASM + Landlock + bwrap + seccomp + Seatbelt + TEE sim) | `gauss-sandbox` | ✅ Phase 3, 10 |
| HWCA worker + schema gate | `gauss-hwca` | ✅ Phase 4 |
| Receipt chain (Ed25519, BLAKE3, TSA anchor) | `gauss-audit`, `gauss-attest` | ✅ Phase 5 |
| Three-plane scheduler (conv / daemon / approval) | `gauss-kernel::sched` | ✅ Phase 1 |
| Provider trait + polyhedral equivalence verifier | `gauss-provider`, `gauss-poly` | ✅ Phase 2, 8 |
| SAG / approval plane | `gauss-sag` | ✅ Phase 7 |
| Gateway wire types, A2UI Canvas, Health (SDHE) | `gauss-gateway`, `gauss-canvas`, `gauss-health` | ✅ Phase 9 |
| Chaos injectors, scale ring, benches | `gauss-chaos`, `gauss-bench`, `gauss-robust` | ✅ Phase 10 |
| zk / DP / learnt-Φ research vehicles | `gauss-zk`, `gauss-dp`, `gauss-learnt` | ✅ Phase 11 |

**GaussClaw introduces a new family of `gaussclaw-*` crates that sit on top of these traits** — it does *not* re-derive runtime primitives.

---

## New Crate Layout

The port adds the following workspace members. Each crate is small, single-responsibility, and binds to existing `gauss-*` traits.

```
crates/
├── gaussclaw-agent          # Hermes AIAgent.run_conversation → DTE turn policy
├── gaussclaw-cli            # clap subcommands: model · tools · config · gateway · setup · update · doctor
├── gaussclaw-tui            # Ratatui + crossterm; full-screen interactive shell (replaces React+Ink)
├── gaussclaw-web            # Axum dashboard backend + embedded React 19 / Vite / Tailwind v4 frontend
├── gaussclaw-desktop        # Tauri 2 desktop app; reuses gaussclaw-web frontend; replaces Hermes Electron
├── gaussclaw-website        # Docusaurus content tree + i18n (en, zh-Hans); CI-built static site
├── gaussclaw-surfaces       # Thin adapters: REST · WS · OAI-compat relay (ChannelTrait impls)
├── gaussclaw-channels       # ~20 messaging adapters (Slack, Discord, Telegram, Matrix, …)
├── gaussclaw-providers      # 20 leaf vendor drivers (Anthropic, OpenAI, Google, …)
├── gaussclaw-providers-meta # OpenRouter (aggregator) + NotDiamond (learned router)
├── gaussclaw-api-modes      # chat-completion · responses · openai-compat shims
├── gaussclaw-tools          # First-party tools: web_search, file_*, shell, …
├── gaussclaw-skill          # Skill Manifest parser + #[tool] proc-macro
├── gaussclaw-store          # Hermes session/lineage schema atop SurrealDB
├── gaussclaw-export         # SFT / DPO writer + Cryptographic Envelope + Taint Filter
├── gaussclaw-fed            # Federated Trajectory Pool client + reference server
├── gaussclaw-config         # Hermes-compatible TOML (figment) loader
├── gaussclaw-migrate        # `gaussclaw import hermes ./hermes-config.toml`
├── gaussclaw-conformance    # Hermes-parity test suite (1,000-turn corpus, OAI SDK, TUI snapshot, web e2e)
└── gaussclaw-bin            # The shipping binary; wires all of the above
```

### UI surface parity matrix (vs. Hermes upstream)

| Hermes path | Stack | GaussClaw crate | Stack | Improvement |
|---|---|---|---|---|
| `hermes/ui-tui/` | TypeScript · React · forked **Ink** | `gaussclaw-tui` | Rust · **Ratatui + crossterm** | No Node runtime; cold-start sub-10 ms; receipt-aware status bar |
| `hermes/web/` (frontend) | React 19 · Vite · Tailwind v4 · shadcn-style | `gaussclaw-web` (`/frontend`) | **Retained** verbatim, served via `rust-embed` | Zero behavioural drift; same `StatusPage` / `ConfigPage` / `EnvPage` |
| `hermes/web/` (backend) | FastAPI · **POSIX PTY** (WSL2-only) | `gaussclaw-web` (`/backend`) | Rust · **Axum** · WebSocket streaming | No PTY; runs on Linux / macOS / Windows native; capability-gated config writes |
| Hermes Desktop ([hermes-ai.net/desktop](https://hermes-ai.net/desktop/)) | **Electron 39** · Chromium-bundled · HTTP-over-localhost:8642 · unsigned | `gaussclaw-desktop` | **Tauri 2** · system WebView · typed IPC sidecar · Ed25519-signed | ~10× smaller installer; ~3× lower RAM; sub-500 ms cold start; code-signed on all 3 OSes; mobile (iOS/Android) path via Tauri 2 mobile |
| `hermes/tui_gateway/` | Python TUI ↔ gateway IPC | folded into `gaussclaw-tui` + `gaussclaw-gateway` plane | Rust · in-process channel | One process; no IPC ceremony |
| `hermes/website/` | **Docusaurus** + i18n (zh-Hans) | `gaussclaw-website` | **Docusaurus** retained + new `mdBook` API reference | Preserves URL structure + i18n; adds rustdoc-aware crate API site |
| `hermes` CLI | Python `click` | `gaussclaw-cli` | Rust · `clap` v4 | Static binary; identical subcommands and flags |

---

## Phase Overview

| Phase | Weeks | Title | Milestone | Headline Deliverable | Status |
|-------|-------|-------|-----------|----------------------|--------|
| **P1** | 1–4   | Surface and Channel Routing | **M1** | All Hermes surfaces re-routed through Gauss Gateway in shim regime; 1,000-turn byte-identical replay | ✅ Shipped |
| **P2** | 4–10  | Memory, Receipts, and Lineage | **M2** | SQLite/FTS5 → Trinity over SurrealDB; Ed25519 chain; 2-week dual-write parity | ✅ Shipped |
| **P3** | 10–16 | Tools and Sandbox | **M3** | Every `@tool` lifted into HWCA + Composite Sandbox; IPI ≤ 2.19 %; spawn p99 ≤ 15 ms | ✅ Shipped |
| **P4** | 16–20 | Provider Plane and Meta-Routers | **M4** | 20 leaf + OpenRouter + NotDiamond under polyhedral / router-transparency contracts | ✅ Shipped (vendor codec selection config-driven; HTTP backend wiring is the P6 §1 follow-on) |
| **P5** | 20–24 | Trajectory Export | **M5** | Cryptographic Envelope + Taint-Aware Filter + Federated Pool + 15-axis scorecard | ✅ Shipped |
| **P6** | 24–32 | Production wiring + GA | **GA** | Provider HTTP backend wired live; multi-agent coordinator; signed/notarised desktop artefacts; public bug bounty | 🟡 In flight (see §"Phase 6 — Production Wiring + GA" below) |

> **Implementation status as of 2026-05.** The workspace ships **26
> `gaussclaw-*` crates** (~46.6K LOC, 883 tests) on top of **28
> `gauss-*` crates** at the Gauss-Aether 1.0 line. Phases 1 → 5 are
> closed; Phase 6 is the production-GA push tracked in the parent
> `/ROADMAP.md` as Sprint 14 → 17 and recapped in this file's
> Phase 6 section below. The OpenHarness parity matrix in
> `/docs/OPENHARNESS_PARITY.md` is the authoritative subsystem map.

Each milestone produces a shippable binary. Phase N+1 validates against Phase N's artefact.

---

## Phase 1 — Surface and Channel Routing (Weeks 1–4) → M1

**Goal.** Re-route every Hermes entry surface and channel adapter through the Gauss-Aether Gateway so that **Principle 1 holds on Day 1**. The legacy Python `run_conversation` is kept alive as a *privileged subprocess shim*; only the wrapping layer is Rust. This is the **shim regime**.

### Scope

- **CLI** (`gaussclaw-cli`) — clap-based subcommand surface matching Hermes 1:1
- **TUI** (`gaussclaw-tui`) — Ratatui-based full-screen shell replacing Hermes's React+Ink
- **Web dashboard** (`gaussclaw-web`) — Axum backend + retained React 19 / Vite / Tailwind frontend
- **Website** (`gaussclaw-website`) — Docusaurus content + i18n; mdBook API reference
- **Thin surfaces** (`gaussclaw-surfaces`) — REST, WS, OAI-compat relay
- **Messaging channels** (`gaussclaw-channels`) — Slack, Discord, Telegram, Matrix, Mattermost, IRC, XMPP, Signal, SMS, Email, Webhook, …
- **Three-plane routing** via `gauss-gateway`

### Tasks

1. **Audit Hermes adapters.** Walk `hermes/surfaces/*`, `hermes/channels/*`, `hermes/ui-tui/src/**`, `hermes/web/**`, `hermes/website/**`; produce `docs/HERMES_ADAPTER_MATRIX.md` listing per surface: file path, transport, message schema, auth model, `surface` field value.

2. **CLI subcommand parity (`gaussclaw-cli`).** clap v4 derive API. The shipping binary must accept every Hermes top-level command:

   | Hermes command | GaussClaw equivalent | Notes |
   |---|---|---|
   | `hermes` (no args) | `gaussclaw` | Launches TUI by default |
   | `hermes model` | `gaussclaw model` | Interactive model picker (talks to provider plane) |
   | `hermes tools` | `gaussclaw tools` | Enable/disable tools; lists Skill Manifests |
   | `hermes config set <k> <v>` | `gaussclaw config set <k> <v>` | figment-backed; capability-gated writes |
   | `hermes gateway` | `gaussclaw gateway` | Starts messaging gateway (Daemon plane) |
   | `hermes setup` | `gaussclaw setup` | Interactive wizard; writes `gaussclaw.toml` |
   | `hermes update` | `gaussclaw update` | Self-update via signed release manifests |
   | `hermes doctor` | `gaussclaw doctor` | Runs `gauss-health` SDHE invariants |
   | — | `gaussclaw import hermes <path>` | NEW: migrate Hermes config |
   | — | `gaussclaw chat` | NEW: one-shot REPL without TUI |

   Flag-level parity tested by `gaussclaw-conformance::cli_parity` against a frozen `--help` corpus.

3. **TUI (`gaussclaw-tui`).** Replace the React + Ink stack with a native Rust TUI:
   - **Stack:** [Ratatui](https://ratatui.rs) + [crossterm](https://github.com/crossterm-rs/crossterm) for rendering, [tui-textarea](https://github.com/rhysd/tui-textarea) for multiline editing, [tui-tree-widget](https://github.com/EdJoPaTo/tui-rs-tree-widget) for the queue panel, `tokio` for async streaming.
   - **Widget parity** with Hermes Ink components:
     - Chat input with multiline (`Shift+Enter` / `Alt+Enter` to insert newline)
     - Live assistant streaming pane (tokens arrive over WS from the Conversation plane)
     - Prompt overlays: **approval**, **clarify**, **sudo**, **secret-input** (all routed through `gauss-sag`)
     - Queue preview panel (messages queued while agent busy; drain on completion)
     - Status bar (session, model, turn count, **receipt chain head hash**, taint floor — *new vs. Hermes*)
     - Completion list (slash commands, file paths)
     - Session resume picker (`SELECT FROM turn WHERE …` via `gaussclaw-store`)
     - Model picker (catalogue from `gaussclaw-providers` + meta-routers)
     - Tool activity lane (per-tool sandbox status, fuel/epoch counters)
   - **Keybindings** match Hermes verbatim: `Enter` submit, `Shift+Enter`/`Alt+Enter` newline, `Ctrl+C` interrupt/clear/exit, `Ctrl+L` new session, `Cmd/Ctrl+G` open `$EDITOR`, `Tab` apply completion, `Up/Down` cycle completions / edit history.
   - **Slash commands** match Hermes verbatim plus GaussClaw additions:
     `/help`, `/quit`, `/clear`, `/new`, `/compact`, `/resume`, `/copy`, `/paste`, `/details`, `/logs`, `/statusbar`, `/queue`, `/undo`, `/retry`,
     **new:** `/receipt` (show current chain head + Merkle proof), `/taint` (show floor + per-token labels), `/caps` (show current capability set), `/sandbox` (per-tool layer status).
   - **Theme:** TOML-driven, parses Hermes's existing theme files unchanged.
   - **Conformance:** `gaussclaw-conformance::tui_snapshot` records terminal output via [insta](https://insta.rs/) golden snapshots for every screen state.

4. **Web dashboard (`gaussclaw-web`).** Two halves:

   **Backend (`/backend`, Rust · Axum).**
   - Replaces Hermes's FastAPI server. No Python in the dashboard path.
   - REST endpoints mirror Hermes: `GET /api/status`, `GET /api/sessions`, `GET /api/config/schema`, `POST /api/config/set`, `GET /api/env`, `POST /api/env/save`, `GET /api/tools`, `POST /api/chat` (SSE), `WS /api/chat/ws`.
   - **Eliminates the POSIX PTY dependency.** The chat pane streams over WebSocket through `gauss-gateway::Conversation`, so the dashboard runs natively on Linux, macOS, and Windows — closing the Hermes WSL2-only restriction.
   - Config writes are capability-gated: the dashboard backend declares `caps = ["config:write"]` in its Skill Manifest; an operator without that capability sees a read-only UI.
   - Telemetry endpoints expose `gauss-health` SDHE invariants, receipt-chain head, per-plane budget pools, IPI-defence counters.

   **Frontend (`/frontend`, retained verbatim from Hermes upstream).**
   - **No rewrite.** Kept on React 19 + Vite + Tailwind CSS v4 + shadcn-style primitives.
   - Pages preserved: `StatusPage`, `ConfigPage`, `EnvPage`.
   - Page additions (additive only): `ReceiptPage` (chain head, verify upload, TSA proofs), `LineagePage` (graph viewer), `SandboxPage` (per-tool layer status), `ProvidersPage` (catalogue, polyhedral-equivalence badges, OpenRouter/NotDiamond catalogues), `ExportPage` (envelope viewer, taint filter mode toggle).
   - Build pipeline: `cd frontend && pnpm install && pnpm build` → `dist/` is embedded into the Rust binary via [`rust-embed`](https://github.com/pyrossh/rust-embed) so the shipping `gaussclaw` is a single static binary.
   - The Vite dev server proxies `/api/*` to the Axum backend during development (mirrors Hermes's FastAPI proxy).
   - **Optional Leptos variant** tracked in v2 (`/frontend-leptos`) for a fully-Rust WASM dashboard. Not GA-blocking.

5. **Desktop app (`gaussclaw-desktop`).** A native cross-platform desktop application built on **Tauri 2 + Rust**, designed as a strict superset of the upstream **Hermes Desktop** (Electron 39 + Python backend over HTTP at `127.0.0.1:8642`). It reuses the same React 19 / Vite / Tailwind v4 frontend from `gaussclaw-web` — no second codebase — and wires it to the `gaussclaw` daemon as a Tauri **sidecar** over typed IPC instead of HTTP.

   **Stack.**
   - [Tauri 2](https://tauri.app) Rust shell rendering through the OS-native WebView (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux). No Chromium bundled.
   - Shared frontend: the exact `gaussclaw-web/frontend` React 19 + Vite + Tailwind v4 + shadcn-style codebase, compiled once and consumed by both the web dashboard and the desktop shell. A `BUILD_TARGET=desktop` env switch enables Tauri-only features (tray menu, hotkey overlay, native file dialogs).
   - Sidecar: the shipping `gaussclaw` binary registered via `tauri.conf.json > tauri.bundle.externalBin`; lifetime managed by Tauri. IPC over typed `#[tauri::command]` and `tauri::Window::emit` events — no localhost HTTP socket, no PTY.
   - Tauri 2 plugins: `tauri-plugin-window-state`, `tauri-plugin-global-shortcut`, `tauri-plugin-notification`, `tauri-plugin-autostart`, `tauri-plugin-clipboard-manager`, `tauri-plugin-fs` (capability-scoped), `tauri-plugin-shell` (capability-scoped), `tauri-plugin-updater` (Ed25519-signed manifests), `tauri-plugin-deep-link`, `tauri-plugin-single-instance`, `tauri-plugin-os`, `tauri-plugin-store`, `tauri-plugin-dialog`, `tauri-plugin-process`.

   **Screen parity** with Hermes Desktop's 12 windows — preserved by name, navigation, and search semantics (SQLite FTS5 there → SurrealDB FTS here):
   `Chat`, `Sessions`, `Agents`, `Skills`, `Models`, `Memory`, `Soul`, `Tools`, `Schedules`, `Gateway`, `Office`, `Settings`.

   **Slash-command parity.** All 22 Hermes Desktop slash commands accepted in the Chat window, plus the GaussClaw-specific `/receipt`, `/taint`, `/caps`, `/sandbox` overlays introduced in the TUI.

   **Toolset parity.** All 14 Hermes Desktop toolsets (web, browser, terminal, file, code, vision, …) ship as Skill Manifests in `gaussclaw-tools`. Each toolset is toggleable from the `Tools` window, with the toggle producing a capability-set delta the kernel honours immediately (no daemon restart).

   **Gateway parity.** All 16 messaging gateways from Hermes Desktop wired through `gaussclaw-channels` and visible in the `Gateway` window.

   **Additive screens** (already in `gaussclaw-web` frontend; ride the same Tauri shell):
   `Receipt` (chain head, Merkle proof verifier, TSA attestation viewer), `Lineage` (interactive conversation graph), `Sandbox` (per-tool layer status with fuel/epoch counters), `Providers` (catalogue, polyhedral-equivalence badges, OpenRouter / NotDiamond aggregator views, cost telemetry), `Export` (trajectory envelope inspector, taint-filter mode toggle, federated-pool publisher).

   **Office / 3D view.** Hermes's `Office` Claw3d window is preserved through three.js inside the WebView (the React frontend already drives it). An optional **Bevy** companion window via Tauri multi-window is tracked as v2 for GPU-accelerated scenes.

   **Desktop-native features that close Hermes Desktop's gaps.**
   - **System tray** with quick-toggle for the gateway, recent sessions, capability hold/release, and a one-click `Pause all turns` kill switch routed through the Approval plane.
   - **Global hotkey** (default `Cmd/Ctrl+Shift+H`, configurable) summons a Spotlight-style quick-prompt overlay that dispatches a one-shot turn without focusing the main window.
   - **Native OS notifications** for tool-approval prompts, deadline elapses, gateway-arriving messages, receipt-chain anchor events.
   - **Single-instance lock** (Hermes Electron does not guarantee this; double-clicks on the launcher can produce duplicated tray icons).
   - **Deep links**: `gaussclaw://session/{id}`, `gaussclaw://skill/install/{url}` for one-click skill installs from skill-marketplace sources.
   - **Autostart on login** opt-in (`tauri-plugin-autostart`).
   - **Window-state restoration** per monitor, per workspace.
   - **Clipboard monitor** surfaced as a `clipboard:read` Skill Manifest — capability-gated; off by default.
   - **Drag-and-drop files** into Chat routed through Tauri's scoped `fs` plugin, dropped contents default to taint `user`.
   - **Multi-window** for `Skills` inspector, `Office` 3D view, `Memory` editor, `Sandbox` monitor (each a separate Tauri window).
   - **Accessibility**: WebView reads the same ARIA tree as `gaussclaw-web`; keyboard navigation parity with the TUI keybindings where applicable.

   **Capability-model alignment (the structural win).**
   Tauri 2 ships a permission system — `tauri.conf.json > app.security.capabilities[]` — that scopes every plugin call (fs read, shell exec, network fetch, clipboard, …) to a declared allowlist. **This permission system maps directly onto Gauss-Aether's capability lattice 𝒦.** `gaussclaw-desktop`'s build pipeline emits the Tauri capability JSON *from the same Skill Manifest declarations the kernel reads at admission time*. A tool's `caps = ["fs:read:./data"]` produces the matching Tauri scoped FS permission as a build-time artefact. The desktop shell's front-door capability discipline therefore is the same artefact as the agent's tool-execution capability discipline — there is no second policy to drift.

   **Sidecar IPC over typed commands.**
   The shipping `gaussclaw` daemon runs as a Tauri sidecar; the frontend talks to it through `invoke('gc_command', payload)` which serialises over the platform IPC channel (Unix domain sockets / Windows named pipes). This **eliminates** the Hermes Desktop `localhost:8642` HTTP attack surface, removes HTTP framing overhead, and lets the IPC payload be signed by `gauss-attest` for inter-process authenticity. Streaming responses arrive through Tauri `event` channels, not SSE.

   **Distribution & signing.**

   | Platform | Bundles | Signing |
   |---|---|---|
   | macOS | Universal `.dmg` (aarch64 + x86_64), `.app` | Apple Developer ID + notarization + stapling in CI |
   | Windows | `.msi` (WiX), `.exe` (NSIS), winget manifest | Authenticode signing in CI; EV cert preferred |
   | Linux | `.AppImage`, `.deb`, `.rpm`, Flatpak | GPG-signed; Flathub manifest |
   | iOS (v2) | TestFlight `.ipa` via Tauri 2 mobile | App Store signing |
   | Android (v2) | `.apk`, `.aab` via Tauri 2 mobile | Play Console / F-Droid |

   **Auto-update.**
   `tauri-plugin-updater` consumes Ed25519-signed update manifests; the signing key is held by `gauss-attest`. Each released installer's hash is also chained into the public receipt log, giving every shipped binary a verifiable provenance edge — a property Hermes Desktop's electron-updater does not provide.

   **Footprint targets** (vs. Hermes Desktop Electron 39 baseline):

   |  | Hermes Desktop | GaussClaw Desktop target |
   |---|---|---|
   | Installer size | ~150 MB | **≤ 20 MB** |
   | On-disk size | ~300 MB | **≤ 50 MB** |
   | RAM idle | ~250 MB | **≤ 80 MB** |
   | Cold start (warm cache) | ~3 s | **≤ 500 ms** |
   | Code-signed | No | **Yes (3 OSes)** |
   | Notarized (macOS) | No | **Yes** |
   | Auto-update integrity | TLS-only | **Ed25519-signed + chained receipt** |
   | Mobile path | None | **Tauri 2 mobile (v2)** |
   | IPC channel | HTTP on localhost:8642 | **OS-native IPC (no socket)** |

   **Phase placement.** Frontend reuse and Tauri shell scaffolding land in Phase 1 alongside `gaussclaw-web` (the shared frontend is built once). Code-signing, notarization, signed auto-update manifests, and Flathub/winget submissions finalise in Phase 5 as part of the GA release artefact set.

6. **Website (`gaussclaw-website`).** Two surfaces:

   **User-facing site (`/site`).**
   - **Docusaurus retained** — same generator Hermes uses, same theme structure, same i18n folder layout (English + `zh-Hans`).
   - Content imported from Hermes `/website/` and rewritten for GaussClaw subsystem names; cross-links to the published GaussClaw / Gauss-Aether PDFs.
   - Sections: Getting Started · CLI Reference · TUI Reference · Web Dashboard · Tools (Skill Manifests) · Providers & Meta-Routers · Trajectory Export · Capability & Taint · Receipt Chain · Migration from Hermes · Architecture (links to ARCHITECTURE.md).
   - i18n: English + Simplified Chinese at launch (matching Hermes); German, Japanese tracked as v2.
   - Build: `pnpm build` → `build/` deployable to GitHub Pages, Cloudflare Pages, or any static host.

   **API reference (`/rustdoc`).**
   - [mdBook](https://rust-lang.github.io/mdBook/) site generated by `cargo doc --workspace --no-deps` and stitched into a unified crate-graph index.
   - Auto-deploys with the user-facing site under `/api/`.
   - Covers all `gauss-*` and `gaussclaw-*` crates with their axiom/theorem annotations.

   **CI:** `ascii-guard`-equivalent lint preserved; new lint checks that every page references at least one canonical anchor (axiom, theorem, or Hermes-module path).

7. **Thin surface adapters (`gaussclaw-surfaces`).** REST (Axum routes serving `/v1/...`), WS (axum WebSocket upgrades, same wire shape as Hermes), OAI-compat relay (`/v1/chat/completions`, `/v1/completions`, `/v1/models` with SSE streaming).

8. **Channel adapters (`gaussclaw-channels`).** One module per messaging platform, each a `ChannelTrait` impl. Auth secrets handled by `gauss-attest` secret store; never round-tripped to the Python shim.

9. **Shim subprocess.** `PythonShimExecutor` in `gaussclaw-agent` fork-execs legacy Hermes with stdio JSON-RPC. Every routed message is an RPC call; every streamed token re-emitted on the Gauss-Aether wire. Removed phase-by-phase as native executors land.

10. **Three-plane routing.** All surfaces route through `gauss-gateway`:
   - **Conversation plane:** CLI, TUI, REST, WS, OAI-compat relay, web chat, messaging channels.
   - **Daemon plane:** Scheduled / background turns, gateway long-poll workers.
   - **Approval plane:** Tool-call approval prompts surfaced through TUI overlays, web `ApprovalPage`, and Slack/Discord interactive messages.

11. **OAI-compat relay parity.** Parametrise the **OpenAI Python SDK end-to-end test suite** against both Hermes and GaussClaw back-ends.

12. **Audit-trace recording.** Every adapter writes a turn-entry trace (surface, ts, headers, body hash) into `gauss-audit` *before* dispatch.

### Crate dependency edges

```
gaussclaw-cli       → gaussclaw-agent, gaussclaw-tui, gaussclaw-web, gaussclaw-config
gaussclaw-tui       → gauss-gateway, gauss-traits, ratatui, crossterm, tui-textarea
gaussclaw-web       → gauss-gateway, gauss-health, gauss-sag, axum, rust-embed
gaussclaw-desktop   → gaussclaw-web (frontend reuse), gauss-attest (updater key),
                       tauri v2 + plugins (window-state, global-shortcut,
                       notification, autostart, clipboard-manager, fs, shell,
                       updater, deep-link, single-instance, os, store, dialog)
gaussclaw-website   → (build-only: docusaurus, mdbook; no runtime deps)
gaussclaw-surfaces  → gauss-gateway, gauss-traits, axum
gaussclaw-channels  → gauss-gateway, gauss-traits, gauss-attest
gaussclaw-agent     → gauss-turn, gauss-traits (shim path: tokio::process::Command)
gaussclaw-bin       → all of the above
```

### Exit criteria (M1)

- [ ] All 20+ channels deliver a representative sample of **1,000 production turns** through GaussClaw, with output byte-identical to Hermes modulo timestamp.
- [ ] **CLI parity:** every Hermes subcommand and flag accepted by `gaussclaw-cli`; `gaussclaw --help` and per-subcommand help diff clean against a frozen Hermes corpus.
- [ ] **TUI parity:** every Hermes Ink screen, keybinding, and slash command available in `gaussclaw-tui`; insta snapshot tests green for every documented screen state; latency p99 for first-token render ≤ 20 ms above provider latency.
- [ ] **Web dashboard parity:** `StatusPage`, `ConfigPage`, `EnvPage` work against the Axum backend with no client-side changes from the Hermes upstream frontend; Playwright e2e suite green on Linux, macOS, Windows native (no WSL2 required).
- [ ] **Desktop app shell:** `gaussclaw-desktop` builds a Tauri 2 development bundle on macOS, Windows, Linux that loads the shared frontend, attaches the `gaussclaw` sidecar via typed IPC, and renders all 12 Hermes Desktop screens. Tray, global hotkey, native notifications, and single-instance lock pass functional tests. Code-signing / notarization deferred to GA.
- [ ] **Website parity:** Docusaurus site builds, deploys to a static host, serves both English and `zh-Hans`; redirect map from the Hermes URL structure tested.
- [ ] OAI-compat relay passes **100 %** of the OpenAI Python SDK's official end-to-end suite, parametrised by both backends.
- [ ] Trajectory export under shim regime produces files **byte-identical** to those produced by raw Hermes on the same input traffic (run via `gaussclaw-conformance::replay_corpus`).
- [ ] No regression in `gauss-conformance` (A1–A9, T1–T12).

### Rollback

Adapter-level kill switch in `gaussclaw.toml`:
```toml
[surfaces.rest]
backend = "shim"          # "shim" → legacy Hermes; "native" → Rust executor
```
A single config toggle returns any one surface to the legacy executor.

### Risks (cf. Table III of GaussClaw.pdf)

- *Shim RPC drift* — mitigated by JSON-schema-validated RPC envelope + nightly diff.
- *Channel auth secrets* — handled via `gauss-attest` secret store; never round-tripped to the Python shim.

---

## Phase 2 — Memory, Receipts, and Lineage (Weeks 4–10) → M2

**Goal.** Replace Hermes's `store.session` (SQLite + FTS5) and `store.lineage` (parent-pointer table) with the **Trinity Memory Substrate over SurrealDB** already provided by `gauss-memory`, signed by the Ed25519 receipt chain in `gauss-audit`. The legacy executor still drives turns; every turn is **dual-written** to SQLite and SurrealDB for a 2-week parity window.

### Scope

- `store.session` → SurrealDB `turn` document table (with vector embedding + FTS analyzer)
- `store.lineage` → SurrealDB graph edge `RELATE turn -> turn`
- New tables: `receipt`, `chain_anchor` (time-series), `fts_idx`
- Ed25519 receipt chain inside the same transaction as the turn write
- Hourly TSA anchor (OpenTimestamps by default)

### Trinity schema (SurrealQL, in `gaussclaw-store::schema.surql`)

```surql
DEFINE TABLE turn SCHEMAFULL;
DEFINE FIELD session   ON turn TYPE string;
DEFINE FIELD ts        ON turn TYPE datetime;
DEFINE FIELD surface   ON turn TYPE string;
DEFINE FIELD prompt    ON turn TYPE string;
DEFINE FIELD completion ON turn TYPE string;
DEFINE FIELD tool_calls ON turn TYPE array;
DEFINE FIELD taint     ON turn TYPE string;        -- ⊥ | user | web | adversarial
DEFINE FIELD caps_used ON turn TYPE array;
DEFINE FIELD embedding ON turn TYPE array<float>;
DEFINE FIELD cost      ON turn TYPE object;        -- {tokens, dollars, model_actual}
DEFINE INDEX fts_idx   ON turn FIELDS prompt, completion SEARCH ANALYZER ascii BM25;
DEFINE INDEX hnsw_idx  ON turn FIELDS embedding HNSW DIMENSION 384 DIST COSINE M 16 EFC 200;

DEFINE TABLE receipt SCHEMAFULL;
DEFINE FIELD turn_id   ON receipt TYPE record<turn>;
DEFINE FIELD pk        ON receipt TYPE string;
DEFINE FIELD sig       ON receipt TYPE string;
DEFINE FIELD prev_hash ON receipt TYPE string;
DEFINE FIELD self_hash ON receipt TYPE string;

DEFINE TABLE lineage TYPE RELATION FROM turn TO turn;
DEFINE FIELD signed_edge ON lineage TYPE string;

DEFINE TABLE chain_anchor SCHEMAFULL;
DEFINE FIELD head_at_ts ON chain_anchor TYPE datetime;
DEFINE FIELD head_hash  ON chain_anchor TYPE string;
DEFINE FIELD tsa_proof  ON chain_anchor TYPE bytes;
```

### Tasks

1. **Embedded deployment.** Ship the RocksDB-backed embedded SurrealDB for the `gaussclaw` CLI/TUI; validate the single-node TCP and TiKV-clustered modes against the same SurrealQL.
2. **FTS path.** Wire `store.session` FTS5 reads to SurrealDB's `@@` operator; benchmark on a 10⁵-turn corpus. Recall **must match** Hermes FTS5 on a canned query set (BM25 parity, not arithmetic identity).
3. **Vector path.** Wire HNSW recall via SurrealDB's native HNSW field type; verify the union recall bound (Theorem T5).
4. **Receipt chain integration.** `gaussclaw-store::write_turn(...)` opens a single transaction that:
   1. inserts the `turn` row,
   2. computes `self_hash = BLAKE3(prev_hash || canonical_turn_bytes)`,
   3. signs the receipt with Ed25519,
   4. inserts the `receipt` row,
   5. relates the lineage edge with a signed payload,
   6. commits.
5. **TSA anchor.** Background task in `gaussclaw-store::anchor` writes head every 1,000 receipts (or hourly, whichever first) to OpenTimestamps; result lands in `chain_anchor`. Pluggable: also CTLog/Bitcoin/RFC3161.
6. **Dual-write & diff.** During the parity window, every turn writes to both Hermes SQLite (via shim) and SurrealDB. Nightly job `gaussclaw conformance diff-stores` compares row counts, content hashes, lineage trees.
7. **Approval-plane wakeups.** Subscribe `gauss-sag` to SurrealDB `LIVE SELECT` on pending receipts so deadline elapse and operator action wake the right kernel thread.

### Crate dependency edges

```
gaussclaw-store    → gauss-memory, gauss-audit, gauss-attest, gauss-core
gaussclaw-agent    → gaussclaw-store (replaces direct SQLite calls)
gaussclaw-conformance → gaussclaw-store, gauss-memory
```

### Exit criteria (M2)

- [ ] **2 weeks of production traffic** in dual-write mode without divergence between SQLite and SurrealDB (diff job green nightly).
- [ ] Receipt chain verifies under the public verifier API with **≤ 10 Merkle-proof bytes** per verification (Corollary 4 of `Gauss-Aether.pdf`).
- [ ] **Cold-start ≤ 10 ms** time-from-receive-message to first-token-streamed for a warm session (Theorem T12).
- [ ] **Hybrid recall miss rate ≤ 0.015** on the held-out set (FTS ∪ HNSW).
- [ ] Lineage tree reconstructs identically under both layouts using SurrealDB's `FETCH` traversal vs. SQLite recursive CTE.

### Rollback

SQLite remains authoritative during the parity window. Per-namespace toggle:
```toml
[store]
authoritative = "sqlite"   # promote to "surreal" after M2 + 2 weeks
```

---

## Phase 3 — Tools and Sandbox (Weeks 10–16) → M3

**Goal.** Lift every Hermes `@tool` Python function into a **capability-gated Hierarchical Worker Context** under the Composite Sandbox. Tool raw output stays inside the worker; only a **schema-validated value** crosses back into the parent context. This is the structural cut that closes Deficits 1 and 3 and produces the IPI containment bound of Theorem T9.

### Scope

- Skill Manifest specification (TOML) + parser
- `#[tool]` proc-macro (Rust equivalent of `@tool`)
- Composite Sandbox host wiring (`gauss-sandbox` already implements layers; the new work is the per-tool manifest binding)
- Port of the first-party tool catalogue (~30 tools): `web_search`, `web_fetch`, `file_read`, `file_write`, `shell`, `python_exec`, `calendar_*`, `contacts_*`, `email_*`, `slack_post`, `git_*`, `sql_*`, etc.
- Persistent-worker optimisation for high-frequency tools

### Skill Manifest schema (canonical `skill.toml`)

```toml
name        = "web_search"
description = "Search the web via DuckDuckGo."
usage       = "Use to find recent information not in training data."

caps        = ["network:http_get:duckduckgo.com"]
taint       = "web"                 # ⊥ | user | web | adversarial
reversible  = true
persistent  = false

[cost]
tokens_per_call = 800
wallclock_ms    = 1500
dollars_per_call = 0.0

[schema]                            # JSON Schema for the value v ∈ Σₐ
type = "object"
properties = { results = { type = "array", items = { … } } }
```

### Default taint policy (declass map)

| Tool family | Default taint |
|---|---|
| `web_search`, `web_fetch`, `email_read`, `rss_read` | `web` |
| `shell`, `file_read`, `file_write`, `python_exec` | `user` |
| `calendar_read`, `contacts_read`, `git_status` | `trusted` |
| Untrusted Slack/Discord/IRC ingress | `adversarial` |

The antitone declassification map `declass : ℒ → 𝒦` is loaded from `gaussclaw.toml [taint.declass]` and verified at startup by `gauss-kernel::flow::verify_antitone`.

### Tasks

1. **Skill Manifest spec & parser.** TOML schema in `gaussclaw-skill::manifest`; serde-derived structs; figment loader; manifest-validation pass that rejects under-specified manifests at build time.
2. **`#[tool]` proc-macro.** A Hermes author writes:
   ```rust
   #[tool(
       caps  = ["network:http_get"],
       taint = "web",
       schema = WebSearchOutput,
       reversible = true,
   )]
   async fn web_search(q: String) -> Result<WebSearchOutput> { … }
   ```
   The macro generates: (a) the manifest struct, (b) a `ToolHandler` impl, (c) an `inventory::submit!` registration. Same authoring friction as `@tool`.
3. **Sandbox host wiring.** Each tool invocation:
   1. Kernel admits with `K_t ⊑ K_grant` and `taint ⊑ declass(ℓ)`.
   2. `gauss-hwca` spawns worker context `s_w`.
   3. `gauss-sandbox` enforces WASM (wasmtime, fuel+epoch interrupt) for pure-compute tools, native + Landlock/bwrap/seccomp for filesystem/network tools, namespace+seccomp + Seatbelt + AppContainer per host OS.
   4. Tool raw output stays in `s_w`. Schema validator `X_a` produces value `v ∈ Σ_a`.
   5. Only `v` crosses the boundary; the conversation buffer never sees raw bytes.
4. **First-party tool port.** Port ~30 Hermes tools in order of risk: pure-compute first (`json_*`, `math_*`), then sandboxed I/O (`file_*`, `web_*`), then privileged (`shell`, `python_exec`). Each lands behind a feature flag and a per-tool kill switch.
5. **Persistent workers.** Tools with `persistent = true` retain a worker context across calls within a turn. Spawn cost amortised to first call only.

### Crate dependency edges

```
gaussclaw-skill   → gauss-traits, gauss-core
gaussclaw-tools   → gaussclaw-skill, gauss-hwca, gauss-sandbox
gaussclaw-agent   → gaussclaw-tools (replaces Python tool dispatch)
```

### Exit criteria (M3)

- [ ] Every first-party tool runs under HWCA + Composite Sandbox with **no behavioural regression** on the regression-test corpus.
- [ ] **IPI attack success rate ≤ 2.19 %** on the held-out adversarial corpus (matching AgentSys [8]).
- [ ] **Composite sandbox compromise probability ≤ 1.1 × 10⁻⁷** (product of per-layer measured escape probabilities).
- [ ] **Tool spawn p99 ≤ 15 ms** (cf. ZeroClaw baseline).
- [ ] Persistent-worker optimisation reduces high-frequency-tool spawn cost to first-call-only.
- [ ] All A6, A7, T9, T10 conformance tests green.

### Rollback

Per-tool manifest kill switch:
```toml
[tools.shell]
backend = "shim"     # "native" Rust HWCA path; "shim" reverts to legacy Python
```

---

## Phase 4 — Provider Plane and Meta-Routers (Weeks 16–20) → M4

**Goal.** Re-bind Hermes's ~20 vendor drivers + 3 API modes to the **`ProviderTrait`** of `gauss-provider`, verified at **build time** by the `gauss-poly` polyhedral-equivalence harness. Add first-class **meta-router** adapters for OpenRouter (aggregator) and NotDiamond (learned router), each carrying a **router-transparency** post-condition.

### Scope

- 20 leaf drivers: Anthropic, OpenAI, Google, Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI, Perplexity, Cohere, Replicate, OctoAI, Anyscale, Hugging Face Inference, Ollama (local), llama.cpp (local), vLLM (local), TGI (local).
- 3 API modes: chat-completion, responses, OpenAI-compat.
- 2 meta-routers: OpenRouter, NotDiamond.
- `RouterProviderTrait : ProviderTrait` with the router-transparency contract.

### Contract surface

```rust
#[contract]
trait ProviderTrait {
    /// Postcondition: tokens.len() <= max_tokens
    /// Postcondition: tokens are well-formed UTF-8
    /// Postcondition: tool_calls ⊆ declared tools
    /// Postcondition: finish_reason ∈ {stop, length, tool}
    async fn complete(&self, p: Prompt, max_tokens: usize)
        -> Result<Stream<Token>, ProviderError>;
}

#[contract]
trait RouterProviderTrait: ProviderTrait {
    fn catalogue(&self) -> &[LeafModel];

    /// Postcondition: result.selected ∈ self.catalogue()
    /// Postcondition: result.tokens schema-equiv to
    ///                 ProviderTrait::complete(result.selected, prompt)
    async fn route_complete(&self, p: Prompt, candidates: &[ModelId],
                            max_tokens: usize)
        -> Result<RoutedStream, ProviderError>;
}
```

### Tasks

1. **Specify `ProviderTrait`.** Land the behavioural contract in `gaussclaw-providers::traits` (delegating to `gauss-provider`); attribute macros emit SMT obligations consumed by `gauss-poly`.
2. **Z3 harness.** `gauss-poly` already discharges contracts at build time (Phase 8 of Gauss-Aether). Extend with the **router-transparency** post-condition: for every leaf `m` in `router.catalogue()`, calling `m` directly and calling `m` via the router must produce schema-identical output.
3. **Migrate 20 leaf drivers.** Each driver lives in `gaussclaw-providers::<vendor>`. The build refuses to admit a driver that fails the contract — modulo `best_effort = true` override in `gaussclaw.toml`.
4. **OpenRouter adapter.**
   - OpenAI-Chat-Completions wire schema.
   - `provider = "openrouter"`, `model = "anthropic/claude-3.5-sonnet"` syntax.
   - Per-model price / latency telemetry pulled into Skill Manifest `[cost]` for Daemon-plane scheduling.
   - Automatic failover verified against the router-transparency contract.
5. **NotDiamond adapter.** Both modes:
   - **Advisory:** call `POST /v2/modelRouter/modelSelect`, then dispatch the generation through the chosen leaf adapter directly. Kernel keeps a clean separation between routing and dispatch.
   - **Joint:** call `POST /v2/chat/completions`, read the selected model from the response metadata.
6. **Capability lower-bound resolution.** For a candidate set `M = {m₁, …, m_k}`, kernel computes `K_t = ⋂ K_t(mᵢ)` at admission and **filters `M` before** the router sees it. The router can never dispatch to a model the kernel would have rejected.
7. **Receipt content for meta-routed turns.** Every routed turn's receipt carries three model IDs: the *candidate set*, the *router's recommendation*, the *model actually used*. The receipt chain hashes all three.
8. **Typed fallback chains.** `gaussclaw.toml` syntax:
   ```toml
   [provider.chain]
   primary  = "anthropic/claude-3.5-sonnet"
   fallback = ["openrouter/anthropic/claude-3.5-sonnet",
               "notdiamond/{claude-3.5-sonnet, gpt-4o, gemini-1.5-pro}"]
   ```
   The build refuses to compile a chain whose members are not polyhedrally equivalent on the working subset.

### Crate dependency edges

```
gaussclaw-providers       → gauss-provider, gauss-poly, gauss-traits
gaussclaw-providers-meta  → gaussclaw-providers, gauss-provider
gaussclaw-api-modes       → gaussclaw-providers
gaussclaw-agent           → gaussclaw-api-modes
```

### Exit criteria (M4)

- [ ] **All 20 leaf drivers** pass `ProviderTrait` at build time.
- [ ] **OpenRouter and NotDiamond** pass `RouterProviderTrait` for their entire current catalogue at build time.
- [ ] **Fallback chains** compile only when all members are polyhedrally equivalent on the working subset.
- [ ] Runtime provider switching (Anthropic-direct → Anthropic-via-OpenRouter → NotDiamond{…}) preserves output schema, tool-call lineage, and receipt content on the regression corpus.
- [ ] **Cost field** populated on every turn (tokens, dollars, model_actual).

### Rollback

Per-driver `best_effort = true` flag downgrades the build-time check to a runtime-only equivalence check. Meta-routers may admit catalogue subsets via:
```toml
[providers.openrouter]
catalogue_blacklist = ["some-vendor/broken-model"]
```

---

## Phase 5 — Trajectory Export and GA (Weeks 20–24) → GA

**Goal.** Extend the SFT/DPO export with a **Cryptographic Trajectory Envelope**, ship the **Taint-Aware Filter**, reference-implement the **Federated Trajectory Pool**, run the **15-axis scorecard**, and reach **General Availability**.

### Scope

- `gaussclaw-export::sft` — preserves Hermes JSONL field schema bit-for-bit.
- `gaussclaw-export::dpo` — preserves Hermes preference-pair schema bit-for-bit.
- `gaussclaw-export::envelope` — Cryptographic Trajectory Envelope (Definition 1 of GaussClaw.pdf).
- `gaussclaw-export::filter` — three modes: `permissive`, `strict`, `declassified`.
- `gaussclaw-fed` — S3-backed reference Federated Pool with publish / subscribe / verify API.
- Atropos integration smoke test.
- 15-axis scorecard evaluation.

### Envelope structure

For turn τᵢ producing SFT record `r_i^sft`:
```
Eᵢ = ⟨ r_i^sft, ρᵢ, c_n, πᵢ, TSA(c_n) ⟩
```
- `ρᵢ = ⟨rᵢ, pk, σᵢ, tᵢ⟩` — the turn's signed receipt
- `c_n` — chain head at envelope creation
- `πᵢ` — Merkle inclusion proof for ρᵢ under `c_n`
- `TSA(c_n)` — timestamp authority attestation of `c_n`

The envelope is **optional** for consumers that ignore it; **mandatory** for federated consumption.

### Tasks

1. **Envelope generator.** Emit `Eᵢ` alongside every `r_i^sft`. Envelope verification API in `gaussclaw-export::verify`:
   ```rust
   pub fn verify_envelope(e: &Envelope, pk: &PublicKey,
                          tsa_root: &TsaRoot) -> Result<()>
   ```
2. **Taint-Aware Filter.** Three modes wired through `gaussclaw.toml`:
   ```toml
   [export.filter]
   mode = "declassified"     # permissive | strict | declassified
   ```
   - **Permissive:** emit all records, taint marked in metadata.
   - **Strict:** drop records containing any token with taint ≥ `web`.
   - **Declassified (default):** apply runtime declass map; emit only tokens where `declass(ℓ) ⪰ ⊥`.
3. **Federated Pool reference.** Small S3-backed publish/subscribe service in `gaussclaw-fed`:
   - **Publish:** PUT envelope to `s3://pool/{org}/{chain_head}/{turn_id}.env`.
   - **Subscribe:** poll manifest; for each envelope verify under publisher pk + TSA before admission.
   - **Filter combinator:** combine with taint filter to admit only envelopes whose declared max-taint is acceptable.
4. **Atropos integration smoke test.** End-to-end: GaussClaw instance generates trajectories → envelope-aware Atropos consumer pulls them → fine-tuning proceeds without divergence from an equivalent Hermes run.
5. **15-axis scorecard.** Run `gaussclaw-bench::scorecard` against Hermes, OpenFang, OpenClaw, ZeroClaw; emit Table IV of GaussClaw.pdf.
6. **Six-metric operational profile.** Cold start, tool overhead, audit cost, hybrid recall, crash recovery, multi-tenant safety. Must tie or lead the best baseline on every metric.
7. **`gaussclaw doctor`.** Self-Diagnostic Health Engine (`gauss-health`) command that runs invariants ℐ, prints federated attestations, surfaces any drift.
8. **Migration UX.** `gaussclaw import hermes ./hermes-config.toml` produces a GaussClaw config with the legacy executor enabled and a phase-by-phase opt-in checklist.
9. **Desktop GA artefacts (`gaussclaw-desktop`).** Finalise the Tauri 2 desktop release:
   - Set up CI signing jobs: Apple Developer ID + notarization (macOS), Authenticode (Windows), GPG (Linux), each gated on `gauss-attest` key release.
   - Build the universal macOS `.dmg`, Windows `.msi`/`.exe`, Linux `.AppImage`/`.deb`/`.rpm`; submit winget and Flathub manifests.
   - Wire `tauri-plugin-updater` to a signed manifest endpoint; every release binary's hash is anchored in the public receipt chain.
   - Measure and gate footprint targets (installer ≤ 20 MB, RAM ≤ 80 MB, cold start ≤ 500 ms) per OS.
   - Smoke-test deep links (`gaussclaw://session/{id}`, `gaussclaw://skill/install/{url}`), tray, global hotkey, single-instance lock, autostart, drag-and-drop, native notifications, multi-window flows.
   - Confirm the Tauri capability JSON is emitted from the same Skill Manifests the kernel reads, with a CI lint that fails on drift.
10. **Website + dashboard GA.** Finalise `gaussclaw-website`:
   - All Docusaurus sections complete and proofread in English + `zh-Hans`.
   - `ReceiptPage`, `LineagePage`, `SandboxPage`, `ProvidersPage`, `ExportPage` ship in `gaussclaw-web`'s frontend with documented walkthroughs in the site.
   - mdBook API reference auto-builds from `cargo doc --workspace`.
   - Deploy target set up (GitHub Pages or Cloudflare Pages); HTTPS, redirect map from Hermes URL space, and 301 from `hermes-agent.nousresearch.com`-style paths configured for the official deployment.
11. **Public bug-bounty period.** Two weeks during which Hermes remains co-deployed for any GA-blocking regression. Includes the desktop app: bounty pays for any IPC-bypass, capability-escalation, or update-manifest-forgery finding.

### Crate dependency edges

```
gaussclaw-export → gaussclaw-store, gauss-audit, gauss-attest
gaussclaw-fed    → gaussclaw-export, gauss-attest, (s3/ipfs feature)
gaussclaw-bin    → gaussclaw-export, gaussclaw-fed
```

### Exit criteria (GA)

- [ ] Trajectory envelopes verify end-to-end on a corpus of **10⁶ records**.
- [ ] The 15-axis scorecard places GaussClaw **strictly above** each of Hermes, OpenFang, OpenClaw, ZeroClaw on **every** axis.
- [ ] Six-metric operational profile ties or leads the best baseline on **every** metric.
- [ ] `gaussclaw doctor` passes on all three deployment modes (embedded, single-node TCP, TiKV-clustered).
- [ ] Public bug-bounty closes without GA-blocking regressions.
- [ ] `gaussclaw import hermes` round-trips a real Hermes deployment in under 60 s.
- [ ] **Website live** with English + `zh-Hans` content, mdBook API reference, working migration guide; Lighthouse score ≥ 95 on every section.
- [ ] **Single-binary shipping.** `gaussclaw` is one static binary that includes the TUI, the embedded web dashboard frontend, and all subcommands; no Node/Python runtime required at runtime.
- [ ] **Desktop GA release artefacts.**
  - macOS universal `.dmg` signed with Apple Developer ID, notarized, stapled.
  - Windows `.msi` and `.exe` signed with Authenticode; winget manifest accepted.
  - Linux `.AppImage`, `.deb`, `.rpm` GPG-signed; Flathub manifest accepted.
  - Installer size **≤ 20 MB**, on-disk **≤ 50 MB**, RAM idle **≤ 80 MB**, cold start **≤ 500 ms** measured on each OS.
  - `tauri-plugin-updater` consumes Ed25519-signed manifests; every release binary's SHA-256 is anchored in the public receipt chain via `gauss-attest`.
  - Tauri capability JSON emitted from the same Skill Manifests the kernel reads; no policy drift between front-door and tool-dispatch capabilities.

### Rollback

GA is gated by the bug-bounty period; failure to meet criteria triggers rollback to the most recent M-milestone-passing build. The legacy Hermes deployment is co-deployed throughout the bounty window.

---

## Cross-Phase Concerns

### Configuration compatibility

`gaussclaw-config` is figment-based, accepts the Hermes TOML top-level keys verbatim, and layers GaussClaw-specific keys under namespaced tables:

```toml
# Hermes-compatible (unchanged)
[provider]
name  = "anthropic"
model = "claude-3.5-sonnet"

[surfaces.rest]
host = "127.0.0.1"
port = 8080

# GaussClaw additions (all optional, defaults preserve Hermes behaviour)
[caps]
default_grant = ["fs:read:./data", "network:http_get"]

[taint]
default_declass = "default"     # default | strict

[export.filter]
mode = "declassified"

[provider.chain]
fallback = ["openrouter/anthropic/claude-3.5-sonnet"]
```

### Conformance suite

`gaussclaw-conformance` carries six test classes that run in every CI build from Phase 1 onward:

1. **Hermes-replay.** A frozen 1,000-turn corpus replayed through both Hermes and GaussClaw; byte-equal trajectory output required.
2. **OAI SDK parity.** OpenAI Python SDK's end-to-end test suite, parametrised by both backends.
3. **CLI parity.** Hermes `--help` corpus diffed against `gaussclaw --help`; every subcommand exit-code and stderr shape locked.
4. **TUI snapshot.** `insta` golden snapshots of every documented Ink screen state, re-rendered through Ratatui and diffed.
5. **Web e2e.** Playwright suite driving the React frontend against both Hermes FastAPI and GaussClaw Axum backends; identical user-visible behaviour on Linux / macOS / Windows native.
6. **Desktop e2e.** [WebdriverIO + tauri-driver](https://v2.tauri.app/develop/tests/webdriver/) suite that boots the Tauri shell on macOS, Windows, Linux, drives all 12 Hermes-parity screens, exercises tray / hotkey / notifications / deep links, and asserts the IPC payload schema against the same OpenAPI-style contract the Axum backend serves.
7. **Axiom regressions.** Every PR runs the `gauss-conformance` suite to guarantee A1–A9 / T1–T12 hold.

### Risk register (operational mitigations)

| Risk | Mitigation |
|---|---|
| Trajectory schema drift | Dual-write through M2; nightly Hermes ↔ GaussClaw export diff; schema versioning |
| Provider contract failure | Per-driver kill switch; runtime chain fallback to legacy executor; `best_effort = true` |
| Sandbox escape on new tool | Per-tool kill switch; one-week shadow run for every new manifest before production routing |
| Receipt-chain corruption | TSA anchor every 1,000 receipts; continuous chain-verifier sidecar with paging on divergence |
| HWCA spawn-cost regression | Persistent-worker optimisation; p99 latency SLO with auto-rollback to legacy Python on breach |
| Trajectory export blocking | Async pipeline with backpressure to the producer; bounded queue with disk spill |
| Federation poisoning | Consumer-side reputation tracking; public exclusion list; (v2) zk-SNARK envelope variant |

### Engineering discipline

- **Privilege tiers (SPECS §2):** PRs touching `gauss-kernel`, `gauss-audit`, `gauss-attest`, or any new privileged GaussClaw surface require dual review.
- **No `unsafe` in `gaussclaw-*` crates without ADR.** All FFI (Python shim, vendor SDKs) is wrapped in safe abstractions.
- **`#![deny(warnings)]`** on every new crate; clippy `pedantic + nursery` is the floor.
- **Per-PR axiom trace.** PR template requires `Hermes module · Gauss-Aether axiom · GaussClaw phase` triple.

---

## Headline Numbers (target at GA)

| Metric | Hermes baseline | GaussClaw target | Mechanism |
|---|---|---|---|
| IPI attack success rate | not measured | **≤ 2.19 %** | T9 + HWCA + ℒ |
| Cold start (warm cache) | 80–150 ms | **≤ 10 ms** | T12 delta-encoded + K-LRU |
| Composite sandbox compromise | ~ 1 | **≤ 1.1 × 10⁻⁷** | T10 + TEE |
| Hybrid recall miss rate | 0.08 (FTS5 only) | **≤ 0.015** | T5: ε_fts · ε_vec |
| Throughput | single Python proc | **Θ(N) nodes** | T6 stateless-turn routing |
| Provider switching cost | manual retest | **build-time verified** | T7 + polyhedral equiv. |
| Receipt forgery probability | no receipts | **negl(λ)** | T11 EUF-CMA + collision |
| Tool spawn latency p99 | in-proc Python | **≤ 15 ms** | WASM + Landlock |
| Trajectory provenance | operator trust | **cryptographic** | §IV-A envelope |
| Cross-org data sharing | not feasible | **federated pool** | §IV-E |

---

## Beyond GA (v2 horizon)

Out of scope for the 24-week port; tracked in `docs/V2_HORIZON.md` after GA:

- **Zero-knowledge trajectory envelopes.** zk-SNARK proof of "this SFT record came from a verifying receipt under our chain head" without revealing turn timing or chain length (cf. `gauss-zk`).
- **Differential-privacy exporter.** Calibrated Laplace noise on token-level lineage statistics (cf. `gauss-dp`).
- **Learnt Φ.** Adaptive autonomy gradient trained on operator approval decisions stored in the receipt chain (cf. `gauss-learnt`).
- **Mechanised proofs of T1–T12.** Coq / Lean kernel core with extraction.
- **AgentDojo-style adversarial benchmark.** Empirical calibration of the T9 worst-case IPI bound against deployment-specific declass maps.

---

## Phase 6 — Production Wiring + GA (Weeks 24–32) → GA

> **Mirror of the parent `/ROADMAP.md` Sprint 14 → 17.** Phase 6 is
> the production push that turns the agent — already complete in
> code — into a release operators can deploy and depend on. It
> closes the four remaining "Known gaps" entries in
> `docs/OPENHARNESS_PARITY.md` and ships the GA artefacts the
> earlier phases were structurally ready for but hadn't yet cut.

### Status snapshot (entering Phase 6)

| Surface | State on entry to Phase 6 |
|---|---|
| Agent loop | ✅ Production-wired through `gaussclaw serve`; streams via WebSocket; Ctrl+C / WS-close mid-turn cancel |
| Vendor codec selection | ✅ Config-driven (`anthropic` / `openai` / `echo`); env-sourced API keys |
| Provider HTTP backend | 🟡 `UnconfiguredBackend` fail-loud default — real `reqwest`-backed adapter is Phase 6 §1 |
| MCP transports | ✅ HTTP + stdio; real-server interop test pending (Phase 6 §4) |
| Plugin slash dispatch | 🟡 Discovery + `/commands` works; typed dispatch is Phase 6 §3 |
| Multi-agent coordinator | 🟡 One-shot `DelegateTool` + `MixtureOfAgentsTool` ship; team registry + persistent identities are Phase 6 §9 |
| Desktop installers | 🟡 Tauri 2 shell + IPC ship; code-signing pipelines are Phase 6 §10 |

### Tasks (P6-A — operator-visible production wiring, weeks 24–27)

1. **Provider HTTP backend.** New `gaussclaw-providers-http`
   crate (`reqwest` + `rustls-native-certs`) implementing the
   `gaussclaw_providers::HttpBackend` trait. Plumbed through
   `ProviderChoice::with_backend` in the bin so `gaussclaw serve`
   reaches `api.anthropic.com` / `api.openai.com` with no extra
   config. The existing `gaussclaw-http` crate covers the tools
   side; this crate is the providers side. They share a private
   `reqwest::Client` helper so the TLS / DNS / proxy stories are
   single-sourced.
2. **Live-network smoke test.** `#[ignore]`-gated +
   `live-network` cargo-feature-gated test runs one
   `AgentLoop::run` against the real Anthropic Messages API.
   Stays off by default; the `release` workflow runs it on a
   protected runner with an org-scoped key. Asserts one signed
   receipt + one chain head verify via the public verifier.
3. **Plugin-registered slash dispatch.** `PluginRegistry::
   slash_handlers()` returns `&[(name, fn(&mut Repl, &str) ->
   SlashOutcome)]`. The TUI's `dispatch_slash` consults the
   registry before falling back to the hand-written match;
   "did you mean?" already works through the existing slash
   registry.
4. **MCP HTTP transport — real server interop.** The
   reference MCP echo server runs in CI under docker-in-docker;
   the `live-network` lane runs an end-to-end test that bridges
   through `McpHttpClient` and asserts tool dispatch round-trips
   bit-for-bit against the canonical schema gate output.
5. **Native streaming overrides.** Three `complete_streaming`
   overrides in `gaussclaw-providers` (Anthropic SSE, OpenAI
   `chat/completions/stream`, Ollama line-delimited JSON). A new
   polyhedral test asserts streaming and non-streaming paths
   produce byte-equal canonical completions on a shared probe
   set so the receipt chain stays invariant under streaming
   swap.
6. **`gaussclaw-pty`** (`portable-pty`-backed `PtyBackend`).
   Cap-gated to `EXECUTOR_LOCAL`; wall-clock timeout kills the
   child; partial output surfaces through the existing
   `PtyResult` shape.
7. **`gaussclaw-modal-http`** (`reqwest`-backed
   `ModalHttpClient`). Bearer-token auth, retry with jitter,
   per-call cost cap pre-checked from `ModalConfig::
   max_cost_dollars`.
8. **Search adapter crates** — `gaussclaw-search-tavily`,
   `gaussclaw-search-serpapi`, `gaussclaw-search-brave`. Each
   sits behind the existing `SearchProvider` trait; one
   canonical `SearchResult` shape.

### Tasks (P6-B — multi-agent + observability, weeks 27–29)

9. **Team registry + persistent agent identities.** New
   `gaussclaw-coordinator` crate. `Team = bundle of AgentIdentity {
   id, capabilities, persona, default_model } + TeamPolicy
   (parallel / sequential / consensus)`. Identities persist in
   the Trinity store; coordinator restart resumes mid-conversation.
   New `cap:coordinator:dispatch` bit.
10. **Headless worker subprocesses.** Each non-trivial team
    member runs as a `gaussclaw worker` subprocess speaking
    JSON-RPC over UDS / named pipes. Receipts chain per-worker;
    the coordinator's chain anchors each worker's head digest
    so the parent chain stays forgery-resistant.
11. **`gaussclaw teams {list, run, attach, kill, logs}`** CLI
    surface + a `TeamsPage` (the 10th dashboard view).
12. **OpenTelemetry exporter.** New `gaussclaw-otel` crate
    exporting `LoopEvent`, `AuditEntry`, kernel admit decisions,
    and per-tool span metrics over OTLP/gRPC. Operators get
    Grafana / Datadog with zero in-house instrumentation work.
13. **Prometheus metrics endpoint.** `/metrics` on the
    dashboard exposes turn rate, fallback rate, IPI defence
    hits, sandbox layer counts, audit-chain depth, plugin load
    count. Locked names + labels in `docs/METRICS.md`.
14. **Structured logging policy.** `tracing` subscriber emits
    JSON in production by default; the existing
    `gaussclaw-redact` policy applies to every span so secrets
    cannot leak.

### Tasks (P6-C — desktop GA + release engineering, weeks 29–31)

15. **macOS:** universal `.dmg` signed with Apple Developer ID,
    notarized, stapled. CI signing workflow uses OIDC-scoped
    secrets; build reproducible from the tagged commit.
16. **Windows:** `.msi` + `.exe` Authenticode-signed (EV cert
    in HSM). `winget` manifest auto-PR'd to `microsoft/winget-
    pkgs` on tag.
17. **Linux:** `.AppImage`, `.deb`, `.rpm` GPG-signed; Flathub
    manifest PR'd on tag.
18. **Tauri-plugin-updater integration.** Every release manifest
    Ed25519-signed; every binary's SHA-256 anchored in the
    public receipt chain via `gauss-attest`. The four-axis
    verifier from `docs/UPDATE_INTEGRITY.md` is the canonical
    wire format.
19. **Footprint CI gates.** Per-OS asserts: installer ≤ 20 MB,
    on-disk ≤ 50 MB, RAM idle ≤ 80 MB, cold start ≤ 500 ms. A
    release blocks if any axis regresses.
20. **WebdriverIO + tauri-driver smoke matrix.** macOS /
    Windows / Linux runners drive all 12 Hermes-parity screens
    + the 5 additive screens, exercising tray / hotkey /
    notifications / deep links; the IPC payload schema is
    asserted against the same OpenAPI-style contract the Axum
    backend serves.
21. **Package channels.** Cargo crate publishing to crates.io;
    docs.rs builds clean. Homebrew tap; apt/yum repos.

### Tasks (P6-D — bug bounty + GA launch, weeks 31–32)

22. **2-week public bug bounty.** Scope as published in
    `docs/BUG_BOUNTY.md` (15 in-scope crates, four-tier payout
    schedule). An external security firm runs an independent
    audit of the cap-lattice + audit-chain design; their
    report ships as a public PDF.
23. **Co-deployment.** Hermes co-deployed throughout the
    bounty window so any GA-blocking regression has a clear
    fallback (the shim regime).
24. **Migration runbook.** `gaussclaw import hermes
    ~/.hermes/config.toml` validated end-to-end on three real
    operator deployments; `docs/MIGRATION.md` reflects what we
    actually saw, not what we projected.
25. **GA scorecard.** Re-run the 15-axis Pareto-dominance
    scorecard from `gauss-bench` against Hermes, OpenFang,
    OpenClaw, ZeroClaw. Ship as `docs/GA_SCORECARD.md`. The
    release blocks if the scorecard regresses on any axis from
    the 1.0 baseline.
26. **`v1.0.0` tag.** Cut from `main`. Crates published. Desktop
    artefacts published. Website live in English + Simplified
    Chinese with the migration guide and the verifier walk-
    through.

### Exit criteria (GA)

- [ ] **All four remaining `docs/OPENHARNESS_PARITY.md` Known-gaps
      entries closed.**
  - [ ] Real HTTP backend wired into providers (§1).
  - [ ] Live-network smoke test green on protected CI (§2).
  - [ ] Plugin-registered slash commands dispatch through their
        handlers (§3).
  - [ ] MCP HTTP transport interop test green against the
        reference server (§4).
  - [ ] Multi-agent coordinator ships team registry + persistent
        identities + headless worker subprocesses (§9–§11).
- [ ] Code-signed, notarised desktop installers on all three major
      OSes; footprint CI gates green.
- [ ] Cargo crates published; docs.rs green; Homebrew / apt / yum
      / winget / Flathub accept the tagged release.
- [ ] Public bug-bounty window closes without GA-blocking findings;
      external security firm signs off.
- [ ] 15-axis scorecard re-affirms Pareto dominance over Hermes /
      OpenFang / OpenClaw / ZeroClaw.
- [ ] `v1.0.0` tag cut; announcement post anchors every shipped
      binary's SHA-256 in the public receipt chain.

### Rollback

GA is gated by the bounty window; failure to meet criteria
triggers rollback to the most recent passing milestone. The
Hermes deployment is co-deployed throughout the window. Any
in-flight subsystem can be reverted to its Phase-5 state without
losing the engine's correctness invariants — the cap-lattice
and audit-chain don't depend on the production wiring.

---

## Closing Thesis

GaussClaw is **not a rewrite** of Hermes. It is the same agent dropped into a kernel that was missing. The trajectory flywheel keeps spinning — but every revolution now leaves a signed, taint-labelled, capability-gated record, and accountability under adversarial conditions becomes a structural guarantee rather than an operational hope.
