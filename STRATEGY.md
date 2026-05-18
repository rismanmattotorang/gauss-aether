# Strategic Plan — GaussClaw vs. Hermes

> *Last updated: 2026-05*

This document captures GaussClaw's competitive strategy against the
upstream [Hermes agent](https://github.com/NousResearch/hermes-agent).
It is the source-of-truth for prioritisation across the TUI, the web
dashboard, the desktop shell, the channels, and the agent loop.

---

## 1. Honest audit of the current codebase

A LOC + structure scan of `gaussclaw/crates/` reports the following
state — distinct from what the README marketing claims:

| Crate | Files | Lines | Reality |
|---|---|---|---|
| `gaussclaw-agent` | 2 | 1,413 | **Shipping** — turn loop, kernel admit, audit. |
| `gaussclaw-store` | 4 | 1,556 | **Shipping** — Trinity store, session, lineage. |
| `gaussclaw-export` | 6 | 1,595 | **Shipping** — SFT / DPO + Cryptographic Envelope. |
| `gaussclaw-surfaces` | 1 | 1,291 | **Shipping** — REST / WS / OAI-compat. |
| `gaussclaw-tools` | 19 | 3,150 | **18 tools** real. Sprint 1 added 5; Sprint 2 added 3 HTTP tools (http_get/post/head) over an injectable `HttpClient` trait. README claimed 30+. |
| `gaussclaw-providers` | 15 | 3,575 | **9 leaf providers** + 12 OpenAI-compat shims (groq, cerebras, fireworks, deepseek, mistral, together, xai, perplexity, anyscale, octoai, vllm, tgi). README claimed 20; effective count is now **21**. |
| `gaussclaw-providers-meta` | 3 | 567 | **Shipping** — OpenRouter + NotDiamond + router glue. |
| `gaussclaw-fed` | 4 | 820 | **Shipping** — federated pool client + backend. |
| `gaussclaw-channels` | 5 | 1,580 | **6 channels** real (Webhook, InMemory, Slack, Telegram, Discord, Email). README claimed ~20. **Sprint 1 closed the gap by 4.** |
| `gaussclaw-skill` | 1 | 432 | **Manifest parser only.** No synthesise / promote loop. |
| `gaussclaw-config` | 1 | 467 | **Shipping** — Hermes-compatible TOML. |
| `gaussclaw-migrate` | 1 | 488 | **Shipping** — `import hermes` driver. |
| `gaussclaw-cli` | 1 | 357 | **Shipping** — clap v4 subcommand surface. |
| `gaussclaw-bin` | 1 | 503 | **Shipping** — the single binary. |
| `gaussclaw-conformance` | 5 | 1,044 | **Shipping** — Hermes-parity tests. |
| `gaussclaw-tui` | 1 | 616 | **Bare minimum.** /help, /quit, /clear. No streaming, no slash-command autocomplete, no overlays, no $EDITOR, no history file. |
| `gaussclaw-web` | 1 | 763 | **Backend shipping.** Frontend is a placeholder `index.html`. |
| `gaussclaw-desktop` | 7 | 1,820 | **Shipping** — 18 IPC commands (12 dashboard mirrors, 5 desktop-only, 1 chain-verified updater), Tauri 2 builder wired behind `tauri-runtime` feature, README with build/sign recipe. Sprint 3. |
| `gaussclaw-api-modes` | 1 | 6 | **Empty.** Placeholder lib.rs. |

### Headline gaps vs. the README (Sprint 0 + Sprint 1 status)

- ~~Web frontend doesn't exist~~ → **Sprint 0**: six-view dashboard ships.
- ~~TUI is rudimentary~~ → **Sprint 0**: persistent history, OSC 52 copy, 9 working slash commands. **Sprint 1**: three-overlay modal system (approval / clarify / password).
- ~~Channels: 2 / 20~~ → **Sprint 1**: 6 / 20 (Webhook, InMemory, Slack, Telegram, Discord, Email).
- ~~Tools: 11 / 30+~~ → **Sprint 1**: 15 / 30+. **Sprint 2**: 18 / 30+ (added http_get, http_post, http_head with injectable `HttpClient` + header allowlist + body cap).
- ~~Providers: 9 / 20~~ → **Sprint 1**: 21 effective via the OpenAI-compat shim catalogue.
- ~~Desktop: scaffold with 3 IPC commands~~ → **Sprint 3**: 18 IPC commands (12 dashboard mirrors + 5 desktop-only + 1 chain-verified updater); Tauri 2 runtime wired behind the `tauri-runtime` feature; signing recipe documented; chain-anchored updater verification implemented.

---

## 2. What Hermes actually ships

| Surface | Hermes | Maturity |
|---|---|---|
| **TUI** | Ink/React + nanostores; transcript pane, streaming row, activity lane, queue preview, slash-command popup, modal overlays for approval / clarify / password / resume; OSC 52 copy; `!cmd` + `{!cmd}` shell escape; `$EDITOR` integration; persistent history; double-Enter interrupt | **Rich**, production-grade |
| **Web** | Vite + React 19 + Tailwind 4 + xterm.js + Three.js. Surface: config editor, env/API-key page, agent status, recent sessions. **No chat, no tool inspector, no memory viewer, no replay.** | **Thin** by their own description |
| **Channels** | 16+ adapters (Telegram, Discord, Slack, WhatsApp, Signal, Matrix, Mattermost, WeChat, WeCom, DingTalk, Feishu, BlueBubbles, Email, SMS, Yuanbao, QQBot, +webhook, +HA) | Mature |
| **Skills** | 27 prebuilt skills; FTS5 session search + LLM summarisation + agent-nudged persistence. Marketed as "self-improving" but operationally a memory loop, not a closed synthesis loop. | Real but oversold |
| **Security** | `SECURITY.md` is explicit: *"The only security boundary against an adversarial LLM is the operating system. Nothing inside the agent process constitutes containment."* Approval gate is denylist; admitted bypassable. | **Weak by admission** |

---

## 3. Strategic priorities

### Tier 1 — leapfrog opportunities (where Hermes is weak)

1. **Real web dashboard.** Hermes web is config + status. Ship chat + tool inspector + memory viewer + receipt browser + session timeline. Immediate, defensible win.
2. **Capability-first approval UX.** Hermes shows a denylist prompt with "o/s/a/d" quickkeys. GaussClaw can show a typed capability budget, a taint trace, and a sandbox layer report — the visible difference between "would this be okay?" and "here are the exact resources this tool is about to touch."
3. **Receipt-chain explorer.** Hermes has no audit surface. GaussClaw can show every signed turn, the Merkle position witness, and the TSA anchor in a clickable timeline.
4. **Trajectory envelope viewer.** Drop in an envelope file → see verification status, position witness, declassification trail.
5. **In-binary skill marketplace.** Browse, install, sign, and verify Skill Manifests from the dashboard — Hermes ships `--skills name1,name2` strings.

### Tier 2 — match parity (where Hermes is strong)

6. **Polish the TUI** to Ink-parity: streaming, overlay system (approval / clarify / password), slash-command autocomplete popup, persistent history at `~/.gaussclaw/history`, `$EDITOR` integration, `!cmd` shell escape, OSC 52 copy. Add the GaussClaw-only `/receipt`, `/taint`, `/caps`, `/sandbox` overlays.
7. **Channel coverage.** Implement Telegram, Discord, Slack, Email at minimum — the 4 cover ≥ 95 % of demand. WeChat/Feishu/DingTalk/Yuanbao are China-market plays we can deprioritise.
8. **More providers.** Add Mistral, Together, Groq, Cerebras, Fireworks, DeepSeek, xAI, Perplexity, Anyscale, OctoAI, vLLM, TGI — 11 more provider impls. The trait surface is ready; this is mostly HTTP plumbing.
9. **More tools.** Add http_get, http_post (rate-limited, taint-marked), datetime, uuid, env_get (cap-gated), http_head, json_set, csv_parse, yaml_parse, sql_query (sandboxed).

### Tier 3 — don't bother

10. WeChat/Feishu/DingTalk/Yuanbao channels (CN-market, low ROI globally).
11. 7-backend terminal isolation (Singularity, Modal, Daytona, Vercel Sandbox). Our 4-layer Composite Sandbox supersedes the goal of these backends.

---

## 4. Execution plan

### Sprint 0 (this commit) — *the visible leapfrog*

**Deliverable: a real web dashboard.**
- A polished single-page app embedded in `gaussclaw-web/frontend/dist/`.
- Built in **vanilla HTML + CSS + JS** — no npm, no Vite, no Tailwind compile step.
- Aligns with the "single static binary, no Node.js at runtime *or build time*" pitch.
- Five views: **Chat** · **Sessions** · **Tools** · **Receipts** · **Settings**.
- Dark mode by default with cyan accent (matches the docs site).
- Connects to the existing `/api/status`, `/api/sessions`, `/api/tools`, `/api/providers`, `/api/receipt/head`, `/api/chat/ws` endpoints.
- Aspirational features documented inline where the backend isn't ready (graceful degradation).

**Deliverable: TUI polish round 1.**
- Persistent history at `~/.gaussclaw/history` (Hermes parity).
- New slash commands: `/sessions`, `/copy`, `/queue`, `/model`, `/retry`, `/undo` (parity surface; some stubbed pending backend wiring).
- $EDITOR integration via `Ctrl+E` to compose multiline messages.
- `!cmd` shell escape that runs a command and inserts output (HWCA-gated).
- Better status bar: shows live receipt head, taint floor, capability count.

**Deliverable: this STRATEGY.md.** Captures the audit + priorities.

### Sprint 1 — *match parity* ✅ **shipped**

What landed:

- **Channel adapters**: `SlackChannel` (v0= HMAC + 5-minute replay window),
  `TelegramChannel` (optional `X-Telegram-Bot-Api-Secret-Token`
  header + typed `Update` parsing), `DiscordChannel` (Ed25519
  interaction-signature verification), `EmailChannel` (sender allowlist
  + operator-controlled taint downgrade). All four share the typed
  `ChannelTrait` + admit-gate + outbox contract.
- **Five new tools**: `datetime` (now / parse), `uuid` (v4 + v7), `json_set`
  (mirror of `json_get`), `csv_parse` (RFC 4180 with quoted-field /
  escaped-quote / CRLF support), `env_get` (cap-gated by `env:read`,
  caller-supplied allowlist). New `CapToken::ENV_READ` (bit 9) added to
  `gauss-core`.
- **Providers**: the 12 OpenAI-compat shim constructors in
  `gaussclaw-providers::openai_compat` (groq, cerebras, fireworks,
  deepseek, mistral, together, xai, perplexity, anyscale, octoai, vllm,
  tgi) are surfaced through the catalogue. Effective provider count
  is **21** (9 leaf + 12 OAI-compat), exceeding the 20-claim.
- **TUI overlay system**: a typed `Overlay` enum with three variants
  (`approval`, `clarify`, `password`), Hermes-parity quick-keys
  (o/s/d for approve/refuse/details; 1-9 quick-pick for clarify),
  full Esc-cancel + Ctrl-C interception, masked input for passwords,
  and a centred render-with-Clear panel.

Deferred to a follow-on:
- The three HTTP tools (`http_get`, `http_post`, `http_head`) — these
  need an `HttpBackend` trait equivalent to the providers' one, plus a
  workspace HTTP-client dep. Tracked in Sprint 2.

### Sprint 2 — *deepen the leapfrog* ✅ **shipped**

What landed:

- **HTTP tool family** (`gaussclaw-tools::http`): three tools
  (`http_get`, `http_post`, `http_head`) sharing an injectable
  `HttpClient` trait + `MockHttpClient` for tests + `UnconfiguredHttpClient`
  default. Operator-controlled `HttpToolPolicy`: HTTPS-only scheme by
  default, header allowlist, 64-KiB body cap (truncates with a flag,
  never drops silently). Output taint defaults to `Web`. Hermes ships
  `http_get` as a thin `requests.get(url)` wrapper with no allowlist,
  no body cap, no taint.
- **Dashboard backend endpoints** (`gaussclaw-web`):
  - `GET  /api/receipts/recent?limit=N` — recent-receipts list with
    per-row verification status.
  - `POST /api/envelope/verify` — verify a Cryptographic Trajectory
    Envelope; response names the failing axis (`signature`,
    `payload_digest`, `chain_link`, `witness_head`, `witness_index`,
    `public_key`).
  - `POST /api/skills/preview` — parse a Skill Manifest TOML and
    return its typed summary (caps, taint, cost, IPI guard, max
    string length) without installing it.
- **Dashboard frontend** (`gaussclaw-web/frontend/dist/`):
  - **Receipts view** split into two panes: recent-receipts list +
    envelope-verify drop zone with `dragenter`/`drop` handling and
    a typed verify report (✓ verified or ✕ failed-axis).
  - **Tools view** gains a collapsible "Preview a Skill Manifest"
    panel: paste TOML → see the typed summary with capability badges,
    taint, cost, guards, before any install side-effect.
  - Updated `builtInTools` fallback list to 18 entries including the
    HTTP family with correct cap + taint + layer markers.
- **Web crate gains a real test suite for the new endpoints**:
  4 new tests cover the empty-store receipt path, a valid skill
  preview, an invalid skill preview, and the envelope-verify failure
  axis surface.

Deferred to a follow-on:
- A `reqwest`-backed default `HttpClient` impl behind an optional
  feature flag (the trait + mock are landed; the runtime can inject
  any conforming impl).
- Skill Manifest **install** UI (preview ships now; install requires
  a registry write path under `cap:config:write`).
- Replay corpus diff visualiser — defers to when we have an actual
  shipped corpus to demo against.

### Sprint 3 — *the desktop story* ✅ **shipped**

What landed:

- **`gaussclaw-desktop` IPC surface grows from 6 to 18 commands** in
  three modules:
  - **Dashboard mirrors** (`commands.rs`, +6): `gc_health`,
    `gc_sessions_recent`, `gc_receipts_recent`, `gc_envelope_verify`,
    `gc_skill_preview`, `gc_tools_list`. Every panel the web dashboard
    surfaces is also reachable from the desktop without an HTTP
    round-trip.
  - **Desktop-only commands** (`system.rs`, +5): `gc_clipboard_copy`,
    `gc_global_hotkey_register` (with chord-grammar validation),
    `gc_tray_menu` (operator-configurable tray model),
    `gc_notify` (native notifications), and
    `gc_updater_verify_artifact`.
  - **Chain-verified updater** (`updater.rs`, new): `ReleaseManifest`
    + `verify_release_artifact` — checks SHA-256 binding, publisher
    Ed25519 signature over `version:target:sha256_hex`, target-triple
    match, and strict-greater SemVer (refuses downgrade attacks).
    Hermes ships unsigned binaries with no chain anchor — its
    updater verifies nothing.
- **`runtime.rs` is no longer a stub.** Behind the existing
  `tauri-runtime` feature gate, it now boots a real `tauri::Builder`
  with seven official plugins (`single-instance`, `window-state`,
  `global-shortcut`, `clipboard-manager`, `notification`, `deep-link`,
  `updater`), registers all 18 IPC commands via
  `tauri::generate_handler!`, and runs the event loop. The default
  (no-feature) build remains runtime-free so CI on plain runners
  still compiles + tests the library half.
- **Build + sign recipe** documented in
  `gaussclaw/crates/gaussclaw-desktop/README.md`: per-OS environment
  variables for Apple Developer ID notarisation, Windows Authenticode,
  and Linux GPG / AppImage signing. The `bundle.macOS` /
  `bundle.windows` / `bundle.linux` sections of `tauri.conf.json`
  are wired to consume them.
- **34 tests** (up from 8) cover every IPC command's envelope shape,
  kernel-denied edge cases, hotkey chord parsing, every
  `UpdaterVerifyError` variant, and the canonical signed-message
  format.

What's deferred:

- Tauri-side e2e tests with `webdriverio + tauri-driver` — those need
  a real WebView at test time. The pure-function test suite already
  covers the IPC contract.

### Sprint 3 follow-on — *signed-installer pipeline* ✅ **shipped**

The CI hookup that turns the Sprint-3 recipe into actual signed
artefacts on every tag push.

What landed:

- **`gaussclaw-release-sign` CLI** at
  `gaussclaw/crates/gaussclaw-desktop/src/bin/release-sign.rs`. Reads
  a built installer, computes its SHA-256, signs
  `version:target:sha256_hex` with the publisher's Ed25519 secret key,
  emits a JSON `ReleaseManifest` on stdout. Exposed via a new
  `[[bin]]` entry; default build (no `tauri-runtime` feature) still
  compiles on plain runners.
- **`ReleaseManifest::new()` constructor** — lets the bin construct
  the `#[non_exhaustive]` struct from a separate compilation unit.
- **`.github/workflows/desktop-release.yml`** matrix workflow:
  - Triggers on `v*` tag pushes (and `workflow_dispatch`).
  - Builds the Tauri 2 bundle on `ubuntu-22.04` /
    `macos-14` (aarch64) / `macos-13` (x86_64) /
    `windows-2022`.
  - Wires `APPLE_*`, `WINDOWS_*`, `GPG_*` GitHub Secrets into the
    Tauri bundler + the per-OS signing scripts.
  - Runs `gaussclaw-release-sign` against every produced artefact;
    emits `<artefact>.manifest.json` next to each one.
  - Uploads everything to a GitHub Release with auto-generated notes.
- **Per-OS signing scripts** under `scripts/desktop/`:
  - `sign-linux.sh` — GPG-detached `.asc` for each `.AppImage` /
    `.deb`, with `rpmsign --addsign` for `.rpm` when available.
  - `sign-macos.sh` — `codesign --options runtime --timestamp` plus
    `xcrun notarytool submit --wait` plus `xcrun stapler staple`.
  - `sign-windows.ps1` — auto-locates the latest Win SDK `signtool`
    and signs with SHA-256 + RFC 3161 timestamping.
- **`scripts/desktop/README.md`** documents every required secret,
  the operator-local fallback for manual signing, and the
  end-user verification recipe (`shasum` + manifest inspection).

Provenance contract: all signing code is in-tree under MIT; the
operator's cert material is never checked in — it lives exclusively
in GitHub Secrets and the ephemeral runner filesystem.

---

## 5. Concrete metrics we hold ourselves to

| Metric | Target | How we measure |
|---|---|---|
| Dashboard time-to-first-paint | < 100 ms | Lighthouse on local Axum server. |
| Dashboard bundle size | < 80 KB total | Sum of `frontend/dist/*` byte size. |
| TUI startup → ready | < 50 ms | `time gaussclaw --version` plus first-render trace. |
| Channels with real test suite | ≥ 4 (TG/DC/Slack/Email) by GA | `gaussclaw-conformance` adds parity tests. |
| Providers reachable from `gaussclaw model` | ≥ 14 | Catalogue test counts entries. |
| Tools in `gaussclaw-tools` | ≥ 18 | `registry.rs` length × `lib.rs` exports. |

---

## 6. Anti-goals

- **No SaaS lock-in.** GaussClaw runs on your hardware. No "GaussClaw Cloud."
- **No telemetry pings home.** Health metrics stay on-host.
- **No abandoning Hermes parity.** The replay corpus + OpenAI SDK + CLI `--help` diff are non-negotiable conformance gates.
- **No proprietary serialisation formats.** SFT / DPO JSONL stays bit-identical; envelopes are an optional sidecar.

---

## 7. Communication

Where the README marketing has drifted ahead of reality, fix the
README. Every "shipping" claim that doesn't survive the audit gets
either implemented this sprint or reframed as "on the roadmap" with a
visible target.

The credibility of GaussClaw's safety story rests on every claim being
provable. We hold the product surface to the same standard.
