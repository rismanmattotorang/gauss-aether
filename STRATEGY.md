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
| `gaussclaw-tools` | 13 | 1,997 | **11 tools** real (base64, echo, file_read/write, hash, json_get, math_eval, regex_match, shell, upper). README claimed 30+. |
| `gaussclaw-providers` | 15 | 3,575 | **9 providers** real (Anthropic, OpenAI, Google, Cohere, Ollama, HuggingFace, Replicate, llama_cpp, openai_compat). README claimed 20. |
| `gaussclaw-providers-meta` | 3 | 567 | **Shipping** — OpenRouter + NotDiamond + router glue. |
| `gaussclaw-fed` | 4 | 820 | **Shipping** — federated pool client + backend. |
| `gaussclaw-channels` | 1 | 734 | **2 channels** real (Webhook, InMemory). README claimed ~20. |
| `gaussclaw-skill` | 1 | 432 | **Manifest parser only.** No synthesise / promote loop. |
| `gaussclaw-config` | 1 | 467 | **Shipping** — Hermes-compatible TOML. |
| `gaussclaw-migrate` | 1 | 488 | **Shipping** — `import hermes` driver. |
| `gaussclaw-cli` | 1 | 357 | **Shipping** — clap v4 subcommand surface. |
| `gaussclaw-bin` | 1 | 503 | **Shipping** — the single binary. |
| `gaussclaw-conformance` | 5 | 1,044 | **Shipping** — Hermes-parity tests. |
| `gaussclaw-tui` | 1 | 616 | **Bare minimum.** /help, /quit, /clear. No streaming, no slash-command autocomplete, no overlays, no $EDITOR, no history file. |
| `gaussclaw-web` | 1 | 763 | **Backend shipping.** Frontend is a placeholder `index.html`. |
| `gaussclaw-desktop` | 4 | 470 | **Scaffold only.** Tauri 2 wired, two commands, no screens. |
| `gaussclaw-api-modes` | 1 | 6 | **Empty.** Placeholder lib.rs. |

### Headline gaps vs. the README

- **Web frontend doesn't exist** — `frontend/dist/index.html` is a 60-line placeholder.
- **TUI is rudimentary** — no streaming, no overlay system, no slash-command parity.
- **Channels: 2 / 20** claimed.
- **Tools: 11 / 30+** claimed.
- **Providers: 9 / 20** claimed.
- **Desktop: scaffold** with two stub commands.

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

### Sprint 1 — *match parity*

- Channel adapters for Telegram, Discord, Slack, Email (each ~150 LOC).
- Five more providers (Mistral, Together, Groq, Cerebras, DeepSeek).
- Eight more tools (http_get, http_post, datetime, uuid, env_get, http_head, json_set, csv_parse).
- TUI overlay system (approval / clarify / password modals).

### Sprint 2 — *deepen the leapfrog*

- Receipt-chain explorer in the dashboard with a Merkle-proof viewer.
- Skill Manifest installer UI.
- Replay corpus diff visualiser.
- Trajectory Envelope upload + verify view.

### Sprint 3 — *the desktop story*

- Build out `gaussclaw-desktop` from scaffold to a real Tauri 2 app that wraps the same dashboard. Add `gauss-canvas`-driven dynamic widgets.
- Sign + notarise the macOS / Windows / Linux installers.

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
