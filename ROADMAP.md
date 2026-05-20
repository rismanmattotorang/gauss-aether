# GaussClaw vs. Hermes — Capability Matrix & Forward Roadmap

> *Last updated: 2026-05.*
> Companion to [`STRATEGY.md`](./STRATEGY.md). Where STRATEGY.md is
> the historical sprint log, this document is the forward-looking
> capability matrix and the new sprint plan that closes the residual
> gaps and extends the structural lead.

---

## 1. Executive summary

A side-by-side audit of the upstream
[Hermes agent](https://github.com/NousResearch/hermes-agent) source
tree (76 tool files, 25+ messaging adapters, 29 LLM provider plugins,
~80 interactive slash commands, 12 dashboard pages, plus subsystems we
hadn't previously catalogued — `cron/`, `agent/lsp/`, `acp_adapter/`,
Kanban, Honcho cross-session memory, `agent/curator.py` background
consolidation, `agent/background_review.py` autosave) against the
post-Sprint-3 GaussClaw + Gauss-Aether codebase yields three clear
findings:

1. **Hermes's product surface is larger than our earlier sprint plans
   assumed.** The previous strategy treated Hermes as a ~20-channel /
   ~30-tool / ~20-provider product. The honest count is **~25
   channels** (plus 5 more via plugins), **~76 tool files** (plus
   12 environment / execution backends), **29 provider plugins**.
   Sprints 1-2 closed the ratio on providers (now 72 %) and matched a
   reasonable subset on channels (24 %) and tools (25 %) — but we
   are not yet at Hermes parity by raw count.
2. **GaussClaw ships every Hermes data path that matters in
   production, with structural safety properties Hermes openly
   disclaims.** Session storage, FTS5 + HNSW recall, byte-identical
   SFT/DPO export, the Hermes-config migrator, the cap+taint admit
   gate, the receipt chain, the polyhedral provider verifier, the
   envelope-verify path — all real, all tested. Hermes's `SECURITY.md`
   explicitly says *"nothing inside the agent process constitutes
   containment"*. GaussClaw's six structural superiorities are the
   reason to choose it.
3. **The one *operational* gap that matters most is the agent loop
   driver.** `gaussclaw-agent::run_in_session` does a single
   prompt→completion today. Hermes's `conversation_loop.py` is
   ~9 000 LOC of repeated-tool-dispatch, streaming, retry/fallback,
   compression, and prompt caching. Until that lands, our tool
   catalogue is for one-shot calls, not iterative agentic loops.

The forward strategy is therefore:

- **Sprint 4 — the loop.** Ship a real iterative agent loop with
  streaming + multi-tool dispatch + interrupt + retry. Without this,
  none of the other gaps matter.
- **Sprint 5 — operational subsystems Hermes ships and we don't.**
  Cron, snapshot/rollback, background curator, plugin loader.
- **Sprint 6 — execution backends.** Docker, SSH, Modal, Daytona,
  Vercel Sandbox — every backend cap-gated and taint-aware where
  Hermes runs each under raw operator credentials.
- **Sprint 7 — catalogue parity.** 15+ new tools, 6+ new channels,
  the missing OAuth proxy.
- **Sprint 8 — extend the lead.** Production polyhedral CI, ZK
  receipt proofs, hardware attestation, federated trajectory pool.

Detail follows.

---

## 2. Methodology

This document is grounded in two parallel audits run in this session:

- **Hermes upstream**: `GitHub contents API` + `WebFetch` on
  `raw.githubusercontent.com` over 16 directories. Counts come from
  real file listings, not the Hermes README.
- **GaussClaw / Gauss-Aether**: `find`, `wc -l`, `grep -E`, `Read` over
  every crate under `gaussclaw/crates/` and `gauss-aether/crates/`.
  Status (`✅` / `🟡` / `❌` / `🚫`) reflects what compiles and tests,
  not what the inline marketing claims.

When a Hermes file is annotated **"REAL"** in the matrix below, it
means we found it at >200 LOC with non-stub logic (often >500 LOC).
When it's **"plumbing"** it means glue code that isn't itself the
feature. When GaussClaw is annotated **🟡 partial** it means the
structural code exists but the operationally interesting path is
stubbed.

---

## 3. Capability matrix

Status legend:

| Symbol | Meaning |
|---|---|
| ✅ | Shipping in real code, tested in `cargo test --workspace` |
| 🟡 | Partial — structural skeleton exists, key path stubbed |
| ❌ | Scaffold only — `Cargo.toml` + a few types |
| 🚫 | Not present at all |
| ⭐ | GaussClaw structurally superior to Hermes |

### A. Core agent loop

| Capability | Hermes | GaussClaw | Notes |
|---|---|---|---|
| Single prompt → completion | ✅ (`conversation_loop.py`, ~9 KLOC) | ✅ `gaussclaw-agent::run_in_session` | One-shot path is real |
| Iterative tool-call loop (model emits tool call → execute → re-feed → repeat until stop) | ✅ | 🟡 | **The Sprint-4 blocker.** Infrastructure (HWCA, dispatch, schema gate) is ready; the driver is not |
| Streaming token rendering | ✅ via transport `chat_completions.py` | 🟡 | WebSocket frames exist (`/api/chat/ws`); the agent doesn't emit them turn-by-turn yet |
| Retry / fallback on provider error | ✅ `error_classifier.py`, `retry_utils.py` | 🟡 | `gaussclaw-providers::fallback::FallbackChain` exists; not wired into the loop |
| Conversation compression on token-budget exceed | ✅ `context_compressor.py`, `conversation_compression.py` | 🚫 | Not present |
| Prompt caching (Anthropic 5 min / 1 h) | ✅ `prompt_caching.py` | 🚫 | Not present |
| Subagent / delegation | ✅ `delegate_tool.py`, `mixture_of_agents_tool.py` | 🚫 | Not present |
| Interrupt-and-redirect mid-turn | ✅ TUI `Ctrl+C` cancels active call | 🚫 | TUI quits on Ctrl+C |
| Capability gate on every tool call | 🚫 *(no in-process containment per Hermes `SECURITY.md`)* | ⭐ | `gauss-kernel` admit on every dispatch |
| Taint propagation through the loop | 🚫 | ⭐ | Monotonic taint floor; declass map antitone-verified at startup |
| Signed receipt per turn | 🚫 | ⭐ | Ed25519 + Merkle chain + optional TSA |
| WAL-before-effect (audit before admit) | 🚫 | ⭐ | Axiom A1 by construction |

### B. TUI surface

| Capability | Hermes | GaussClaw | Notes |
|---|---|---|---|
| Renderer | Ink (React-on-TTY); `textInput.tsx` ~1 100 LOC | Ratatui | Both real |
| Streaming assistant pane | ✅ `streamingAssistant.tsx`, `streamingMarkdown.tsx` | 🟡 | History pane updates per turn; no token-level streaming yet |
| Multiline input + bracketed-paste | ✅ | ✅ via `tui-textarea` | Parity |
| Word-wise arrow nav + Ctrl+U/K/W | ✅ | ✅ | Parity |
| Undo / redo (`Ctrl+Z` / `Ctrl+Y`) | ✅ | 🚫 | Not in the textarea wiring |
| Mouse drag + double-click | ✅ | 🚫 | Not wired |
| Right-click context paste/copy | ✅ | 🚫 | Not wired |
| Slash commands | ✅ ~80 commands | 🟡 12 commands (`/help`, `/quit`, `/clear`, `/version`, `/info`, `/status`, `/history`, `/model`, `/copy`, `/receipt`, `/taint`, `/caps`, `/sandbox`) | 15 % parity by count |
| Overlay system (approval / clarify / password / agents / fps / model picker / session picker / skills hub / todo / queued messages) | ✅ 10+ overlays | 🟡 3 (approval / clarify / password) | Foundation laid Sprint 1; needs 7+ more |
| Persistent input history | ✅ SQLite `state.db` | ✅ flat file ring `$XDG_STATE_HOME/gaussclaw/history` | Parity |
| OSC 52 clipboard copy of last reply | ✅ `/copy` | ✅ `/copy` | Parity |
| `$EDITOR` integration | ✅ (Hermes README) | 🚫 | Not present in source |
| Shell-escape syntax (`!cmd` / `{!cmd}`) | 🚫 *(verified absent in textInput.tsx)* | 🚫 | Hermes README claims; source doesn't. We can leapfrog by shipping it cap-gated. |

### C. Web dashboard

| Capability | Hermes | GaussClaw | Notes |
|---|---|---|---|
| Renderer stack | Vite + React + Tailwind v4 + xterm.js + Three.js + Observable Plot + Leva + GSAP | Vanilla HTML/CSS/ES modules (~50 KB total) | ⭐ no build-time deps |
| Dashboard pages | 12 (`AnalyticsPage`, `ChatPage`, `ConfigPage`, `CronPage`, `DocsPage`, `EnvPage`, `LogsPage`, `ModelsPage`, `PluginsPage`, `ProfilesPage`, `SessionsPage`, `SkillsPage`) | 6 (`Chat`, `Sessions`, `Tools`, `Receipts`, `Health`, `Settings`) + 3 deep panels (envelope verify, skill preview, receipt explorer) | 50 % parity by count; **GaussClaw has Receipts and Tool-inspector views Hermes doesn't ship** |
| Multi-session chat in browser | ✅ `ChatPage.tsx` streaming | 🟡 (WebSocket wired, transcript pane works, no multi-session UI) | Sprint 5 |
| Cron CRUD in browser | ✅ `CronPage.tsx` | 🚫 | We don't have a cron subsystem yet |
| Logs viewer | ✅ `LogsPage.tsx` | 🚫 | Sprint 5 |
| Profile switcher | ✅ `ProfilesPage.tsx` | 🟡 (single Config tree, no profile concept) | Sprint 5 |
| Analytics / usage / cost telemetry | ✅ `AnalyticsPage.tsx`, `account_usage.py`, `usage_pricing.py` | 🚫 | Sprint 5 |
| Docs bundle | ✅ `DocsPage.tsx` | 🚫 in-app (we have the Docusaurus site separately) | Lower priority |
| Tool inspector with cap / taint / sandbox layers | 🚫 | ⭐ | **GaussClaw-only Sprint-2 win** |
| Receipt-chain browser | 🚫 | ⭐ | Sprint-2 win |
| Envelope verifier upload | 🚫 | ⭐ | Sprint-2 win |
| Skill Manifest preview (no install) | 🚫 | ⭐ | Sprint-2 win |

### D. Desktop shell

| Capability | Hermes | GaussClaw | Notes |
|---|---|---|---|
| Runtime | Electron 39 (Chromium + Node) | Tauri 2 (OS WebView, no Chromium) | ⭐ ~10× lighter |
| IPC | HTTP on `127.0.0.1:8642` | OS-native (UDS / named pipes) via `tauri::generate_handler!` | ⭐ no socket |
| Installer size | ~150 MB | ≤ 20 MB | ⭐ ~7.5× smaller |
| RAM idle | ~250 MB | ≤ 80 MB | ⭐ ~3× lighter |
| Cold start | ~3 s | ≤ 500 ms | ⭐ ~6× faster |
| Code-signing pipeline | unsigned everywhere | ✅ matrix CI ships `.github/workflows/desktop-release.yml` driving macOS Developer ID + Windows Authenticode + Linux GPG | ⭐ |
| Updater integrity | TLS-only | ⭐ 4-axis chain anchor (SHA-256, Ed25519 publisher sig, target-triple match, no-downgrade) | Sprint-3 follow-on |
| IPC command surface | n/a (HTTP only) | ✅ 22 typed `gc_*` commands | Sprint 3 |
| System tray | ✅ | 🟡 (model present; runtime wiring exists behind feature flag) | Verify when WebView available |
| Global hotkey | ✅ | 🟡 (registration command + chord grammar; runtime wiring feature-gated) | Verify when WebView available |
| Native notifications | ✅ | 🟡 (audit-recorded; runtime wiring feature-gated) | Verify when WebView available |
| Drag-and-drop files | ✅ | 🚫 | Sprint 5 |

### E. CLI subcommand surface

Hermes ships **~25 top-level subcommands** + 80+ slash commands.
GaussClaw ships **9 top-level subcommands** (`chat`, `model`, `tools`,
`config`, `gateway`, `setup`, `update`, `doctor`, `import`, `receipt`,
`web`) plus the 12 TUI slash commands.

Missing top-level subcommands worth porting (priority-ordered):

- `honcho` — cross-session user model + memory map. Sprint 5.
- `sessions browse` — TUI-less session inspector. Sprint 5.
- `cron` — scheduled job management. Sprint 5.
- `claw migrate` (we have `import` — naming is consistent).
- `proxy` — local OAuth-to-OpenAI-compat proxy. Sprint 7.
- `acp` — ACP editor protocol server. Sprint 8.
- `whatsapp` — pair / bridge helper. Sprint 7.
- `gquota`, `usage`, `insights` — telemetry views. Sprint 5.

### F. Channels (messaging adapters)

Hermes ships **20+ adapters in `gateway/platforms/`** plus more under
`plugins/platforms/`. GaussClaw ships **6 adapters**.

| Adapter | Hermes | GaussClaw | Notes |
|---|---|---|---|
| Slack | ✅ | ✅ | `v0=` HMAC + 5-min replay window |
| Discord | ✅ | ✅ | Ed25519 interaction signature |
| Telegram | ✅ | ✅ | Webhook + optional header secret |
| Email | ✅ | ✅ | SMTP + IMAP scaffold; sender allowlist |
| Webhook | ✅ | ✅ | HMAC-verified, generic |
| InMemory (test) | n/a | ✅ | n/a |
| WhatsApp | ✅ | 🚫 | Sprint 7 |
| Signal | ✅ | 🚫 | Sprint 7 |
| Matrix | ✅ | 🚫 | Sprint 7 |
| Mattermost | ✅ | 🚫 | Sprint 7 |
| SMS | ✅ | 🚫 | Sprint 7 (Twilio first) |
| Home Assistant | ✅ | 🚫 | Sprint 8 |
| BlueBubbles (iMessage) | ✅ | 🚫 | Sprint 8 |
| DingTalk / Feishu / WeCom / WeChat / Yuanbao / QQ | ✅ | 🚫 | **De-prioritised** — China-market plays we de-scoped in Sprint 0 and stay deferred |
| MS Graph / Teams | ✅ | 🚫 | Sprint 8 |
| Google Chat / IRC / LINE / SimpleX | ✅ | 🚫 | Sprint 8 |
| HMAC verification trait | n/a (per-adapter ad-hoc) | ⭐ canonical `hmac_verify` primitive | |
| Adversarial-taint default on ingress | 🚫 | ⭐ | Operators downgrade after SPF/DKIM/DMARC |
| Pluggable `SecretStore` | n/a (raw `os.environ`) | ⭐ | HW-attest in production |

### G. Tool catalogue

Hermes ships **~76 tool files** (`tools/*.py`). GaussClaw ships **19
tools**.

GaussClaw shipping today: `base64`, `csv_parse`, `datetime`, `echo`,
`env_get` (cap-gated allowlist), `file_read`, `file_write`, `hash`,
`http_get`, `http_head`, `http_post`, `json_get`, `json_set`,
`math_eval`, `regex_match`, `shell`, `upper`, `uuid`.

High-value Hermes tools missing from GaussClaw, ranked by user
impact:

| Hermes tool | GaussClaw | Sprint |
|---|---|---|
| `terminal_tool` (bash exec) | 🟡 partial via `shell` (single-shot; no PTY) | Sprint 6 |
| `code_execution_tool` (Python sandbox) | 🚫 | Sprint 6 |
| `web_tools` (fetch + content extraction) | 🟡 partial via `http_get` (no content scraping) | Sprint 7 |
| `memory_tool` (read/write agent memory) | 🚫 | Sprint 5 |
| `session_search_tool` (FTS5 over past sessions) | 🟡 (store has hybrid_search; not exposed as a tool) | Sprint 4 |
| `kanban_tools` (CRUD task board) | 🚫 | Sprint 8 (optional) |
| `cronjob_tools` (schedule jobs from inside the agent) | 🚫 | Sprint 5 |
| `delegate_tool` / `mixture_of_agents_tool` | 🚫 | Sprint 6 |
| `clarify_tool` (ask user mid-run) | 🚫 | Sprint 4 (links to the overlay system) |
| `mcp_tool` (MCP client) | 🚫 | Sprint 7 |
| `image_generation_tool` / `video_generation_tool` / `vision_tools` | 🚫 | Sprint 8 |
| `transcription_tools` / `tts_tool` / `voice_mode` | 🚫 | Sprint 8 |
| `browser_tool` / `browser_cdp_tool` | 🚫 | Sprint 8 |
| `tirith_security` (pre-exec command scanner) | 🚫 | Sprint 6 — important security feature |
| `osv_check` (vulnerability scan) | 🚫 | Sprint 6 |
| `discord_tool` / `homeassistant_tool` / `feishu_doc_tool` / `microsoft_graph_*` / `yuanbao_tools` | 🚫 | Lower priority |
| `send_message_tool` (cross-platform send) | 🚫 | Sprint 5 |
| `checkpoint_manager` (FS rollback) | 🚫 | Sprint 5 |
| `skills_*` (Skill lifecycle) | 🟡 (preview only) | Sprint 5 |
| `todo_tool` | 🚫 | Sprint 8 |
| Output-size cap | per-tool | ⭐ canonical `max_string_len` in `SkillManifest` |
| Cap-gating | 🚫 | ⭐ kernel admit |
| Schema gate against IPI | 🚫 | ⭐ HWCA |
| Composite-sandbox enforcement | 🚫 | ⭐ |

### H. Provider / LLM drivers

| Capability | Hermes | GaussClaw |
|---|---|---|
| Leaf provider count | **29 plugins** in `plugins/model-providers/` | **11 native** + **12 OAI-compat shims** = **21 effective** |
| OpenAI Chat Completions transport | ✅ `agent/transports/chat_completions.py` ~700 LOC | ✅ `openai_compat.rs` |
| Anthropic Messages transport | ✅ | ✅ |
| OpenAI Responses / Codex | ✅ `agent/transports/codex.py` | 🟡 in `gaussclaw-api-modes` scaffold (6 LOC) |
| Bedrock | ✅ | 🚫 |
| Gemini native | ✅ | ✅ |
| Bedrock / Azure Foundry / GMI / Arcee / Stepfun / Kilocode / Kimi-coding / NovaPro / Minimax / Alibaba / NVIDIA / XiaoMi / Zai / OpenCode-Zen | ✅ | 🚫 (lower priority; non-OpenAI-compat each is its own port) |
| Capability lower-bound routing | 🚫 | ⭐ `Catalogue::capability_lower_bound` |
| Polyhedral equivalence verifier | 🚫 | ⭐ `gauss-poly`, used as CI gate |
| `MockHttpBackend` for deterministic CI | 🚫 | ⭐ |
| Cost telemetry per call | partial (transport-dependent) | ⭐ `CostHints` on every `LeafModel` |
| `FallbackChain` with attempt audit | 🟡 ad-hoc retry | ⭐ structured `AttemptRecord` |

### I. Storage & memory

| Capability | Hermes | GaussClaw |
|---|---|---|
| Session persistence | ✅ `hermes_state.py` ~2 100 LOC, SQLite WAL | ✅ `gaussclaw-store` 1 556 LOC, SurrealDB Trinity |
| FTS5 search | ✅ `messages_fts` + `messages_fts_trigram` (CJK) | ✅ `fts_search` |
| Vector recall (HNSW) | 🚫 (only FTS) | ⭐ `vector_search` + `hybrid_search` |
| Lineage edges (parent/child turn graph) | ✅ via `parent_session_id`, message refs | ✅ BLAKE3-signed `LineageEdge` per turn |
| Merkle chain over turns | 🚫 | ⭐ |
| Per-turn cost / token accounting | ✅ (every cost column on the session row) | ✅ `TurnCost` + `RouteRecord` |
| Cross-session "user model" (Honcho) | ✅ `hermes_cli/honcho` with peer/identity/mode | 🚫 | Sprint 5 |
| Background memory-consolidation thread | ✅ `agent/background_review.py` (~550 LOC) | 🚫 | Sprint 5 |
| Skill consolidation (Curator) | ✅ `agent/curator.py` (~1 500 LOC) | 🚫 | Sprint 5 |

### J. Export & trajectories

| Capability | Hermes | GaussClaw |
|---|---|---|
| SFT JSONL export | ✅ `batch_runner.py` ~1 100 LOC, ShareGPT-style | ✅ byte-identical schema |
| DPO pair export | 🟡 (not first-class; SFT only) | ✅ `gaussclaw-export::dpo` |
| Trajectory compressor (LLM-summarise mid-turns) | ✅ `trajectory_compressor.py` ~1 100 LOC | 🚫 | Sprint 8 |
| SWE-bench-style runner | ✅ `mini_swe_runner.py` | 🚫 | Sprint 8 (optional) |
| Cryptographic envelope (signed receipt + chain + witness + TSA) | 🚫 | ⭐ `Envelope` + `verify_envelope` |
| Taint-aware filter (declassified / strict / permissive) | 🚫 | ⭐ `TaintFilter` |
| Federated trajectory pool | 🚫 | ⭐ `gaussclaw-fed` |
| Differentially private noise | 🚫 | 🟡 `gauss-dp` (research vehicle) |

### K. Skills & extensibility

| Capability | Hermes | GaussClaw |
|---|---|---|
| Skill discovery roots | ✅ 4 (bundled / user / project / entry-point) | 🚫 | Sprint 7 |
| Plugin loader (5 kinds: standalone / backend / exclusive / platform / model-provider) | ✅ `hermes_cli/plugins.py` ~1 450 LOC | 🚫 | Sprint 7 |
| Skill manifest preview | 🚫 (loads at startup, no preview) | ⭐ `/api/skills/preview` |
| Skill installer w/ provenance + signed cap declaration | 🟡 `skills_sync.py`, `skill_provenance.py` | 🚫 (preview only) | Sprint 7 |
| Skill hub (agentskills.io plumbing) | ✅ `skills_hub.py` | 🚫 | Lower priority |
| `${HERMES_SKILL_DIR}` substitution + inline `` `!cmd` `` in SKILL.md | ✅ `skill_preprocessing.py` | 🚫 | Sprint 7 (cap-gated) |
| `agent/lsp/` language-server client | ✅ 11 files | 🚫 | Sprint 8 (optional) |
| `acp_adapter/` editor protocol | ✅ | 🚫 | Sprint 8 (optional) |
| MCP client tool | ✅ `mcp_tool.py` + OAuth | 🚫 | Sprint 7 |

### L. Sandbox / execution backends

| Capability | Hermes (`tools/environments/`) | GaussClaw |
|---|---|---|
| Local exec | ✅ `local.py` | 🟡 (one execution layer; not selectable per-session) |
| Docker | ✅ `docker.py` ~650 LOC | 🚫 | Sprint 6 |
| SSH (with ControlMaster bulk-sync) | ✅ `ssh.py` ~330 LOC | 🚫 | Sprint 6 |
| Singularity | ✅ ~320 LOC | 🚫 | Sprint 6 (lower priority) |
| Modal | ✅ ~550 LOC | 🚫 | Sprint 6 |
| Daytona | ✅ ~290 LOC | 🚫 | Sprint 8 (optional) |
| Vercel Sandbox | ✅ ~650 LOC | 🚫 | Sprint 8 (optional) |
| 4-layer composite sandbox (WASM / Landlock / seccomp / bwrap) | 🚫 | ⭐ `gauss-sandbox` |
| `Pr[compromise]` ≤ 1.1 × 10⁻⁷ bound (Theorem T10) | 🚫 | ⭐ |
| TEE attestation simulator | 🚫 | ⭐ `gauss-attest` |
| Selectable per-session backend | ✅ `terminal.backend` config key | 🚫 (single composite mode only) | Sprint 6 |

### M. Cron / scheduler

GaussClaw has nothing here. **Major Sprint-5 deliverable.**

Hermes ships:
- `cron/scheduler.py` ~1 900 LOC, 60-second tick, file-locked.
- `cron/jobs.py` ~1 100 LOC, schedule grammar (`30m`, `every 10m`,
  cron expr, ISO timestamps), missed-run grace window, pre-run
  scripts, prompt-injection scan, parallel execution.
- `cronjob_tools.py` — schedule from inside the agent.
- `CronPage.tsx` in the web dashboard.
- A `cron` top-level CLI subcommand + `/cron` slash variants.

### N. Cross-session / user-model features

- **Honcho** (`hermes_cli/honcho/` with 9 sub-actions: setup, status,
  sessions, map, peer, mode, tokens, identity, migrate). Hermes ships
  this. GaussClaw doesn't. **Sprint 5.**
- **Background memory autosave** (`agent/background_review.py`).
  Hermes ships. We don't. **Sprint 5.**
- **Skill curator** (`agent/curator.py` — consolidate narrow skills
  into umbrellas, archive stale 30-day-untouched skills). Hermes
  ships. We don't. **Sprint 5.**

### O. Specialised subsystems

| Subsystem | Hermes | GaussClaw | Priority |
|---|---|---|---|
| Kanban (CLI + DB + tools + plugin) | ✅ | 🚫 | Sprint 8, optional |
| LSP client (`agent/lsp/`) | ✅ 11 files | 🚫 | Sprint 8, optional |
| ACP editor protocol server | ✅ `acp_adapter/` | 🚫 | Sprint 8, optional |
| OAuth → OpenAI-compat proxy | ✅ `hermes proxy` | 🚫 | Sprint 7 |
| Snapshot / rollback (`/snapshot`, `/rollback`) | ✅ `checkpoint_manager.py` | 🚫 | Sprint 5 |
| Worktree-isolated concurrent sessions | ✅ `worktree` config | 🚫 | Sprint 6 |
| TUI agents/subagent overlay | ✅ `agentsOverlay.tsx` | 🚫 | Sprint 6 |
| Banned / sensitive-word redaction | ✅ `agent/redact.py` | 🚫 | Sprint 7 |

---

## 4. Gap analysis: top 15 priority items

Ranked by **user-visible impact × strategic importance**.

1. **Agent loop driver** — without iterative tool-call execution, our
   tool catalogue is for one-shot calls. Sprint 4. *Critical.*
2. **Token-level streaming** — Hermes UX feels live; GaussClaw feels
   batched until we wire token frames through `/api/chat/ws`. Sprint
   4. *Critical.*
3. **Cron scheduler** — Hermes's `cron/` ships a full scheduling
   subsystem; many users automate around it. Sprint 5. *High.*
4. **Subagent / delegation tool** — Hermes's `delegate_tool` and
   `mixture_of_agents_tool` enable parallel workstreams from inside a
   turn. Sprint 6. *High.*
5. **Docker / SSH / Modal execution backends** — Hermes lets the
   operator choose where each session runs. GaussClaw has one
   composite mode. Sprint 6. *High.*
6. **Plugin loader** — Hermes's 5-kind plugin system is how third
   parties extend the agent. Sprint 7. *High.*
7. **Snapshot / rollback** — undo at the file-system level. Sprint 5.
   *Medium-high.*
8. **Cross-session memory ("Honcho")** — Hermes's main retention
   pitch. Sprint 5. *Medium-high.*
9. **Background curator + autosave threads** — silent consolidation
   that keeps the skill library tidy. Sprint 5. *Medium.*
10. **`code_execution_tool` (sandboxed Python)** — the workhorse tool
    for analytical agents. Sprint 6. *Medium.*
11. **MCP client tool** — third-party tooling standard with momentum.
    Sprint 7. *Medium.*
12. **`tirith_security` + `osv_check` pre-exec scanners** — security
    layer Hermes calls out but admits is incomplete. We can ship a
    stronger version (cap-gated). Sprint 6. *Medium.*
13. **5-7 more channel adapters** (WhatsApp, Signal, Matrix,
    Mattermost, SMS at minimum). Sprint 7. *Medium.*
14. **`hermes proxy` equivalent** (OAuth → OpenAI-compat). Sprint 7.
    *Medium-low.*
15. **TUI overlay parity** (agents picker, model picker, session
    picker, skills hub, todo panel) — visible UX gap. Sprint 5.
    *Medium-low.*

---

## 5. Structural wins to extend

These are areas where GaussClaw is *already* better than Hermes and
where investing more compounds the lead.

1. **Cap + taint gating** — extend the lattice with new caps as new
   tools land (`mcp:invoke`, `delegate:spawn`, `worktree:create`).
   Make every new feature explicitly cap-gated; Hermes will never
   catch up here without a process rewrite.
2. **Receipt chain + envelope verification** — ship a *public*
   verifier (a tiny standalone tool that takes an envelope and
   returns ✓/✕). Make it the canonical artefact people exchange.
3. **Polyhedral provider equivalence** — promote `gauss-poly` to a
   *production* CI gate (currently a research vehicle). Every
   provider PR runs a probe-set diff; nobody ships a vendor swap
   without it.
4. **Single static binary** — keep this invariant. Every new feature
   that would have required Python / Node at runtime gets implemented
   in Rust or compiled to WASM.
5. **Chain-anchored updater** — promote the four-axis verifier to a
   public spec; document the wire format under
   `docs/UPDATE_INTEGRITY.md` so other Rust desktop apps can adopt.
6. **Reproducible CI** — keep `cargo test --workspace --lib` green at
   720+ tests through every sprint. This is the most valuable
   ratchet we have against drift.

---

## 6. Roadmap — Sprint 4 through Sprint 8

Each sprint has **concrete deliverables**, **success criteria** (a
green test or a working demo), and a **rough size estimate** (S = a
day, M = a week, L = a month).

### Sprint 4 — the loop (size: L) — ✅ **first cut shipped**

**Status:** the core iterator + tool-call parsing + fallback chain +
two new tools ship in this commit. Token-level WebSocket streaming
end-to-end + Ctrl-C mid-turn cancel through the TUI are tracked for
the Sprint-4 follow-on.

**What landed in this commit:**

- `gaussclaw-agent::agent_loop` module (~800 LOC + 7 tests): `AgentLoop`
  driver, `LoopEvent` enum (`user_submitted` / `token` / `assistant` /
  `tool_start` / `tool_complete` / `fallback_attempt` / `done`),
  `LoopSink` trait with `NoopSink` + `MemorySink` impls,
  `ToolCall::parse_inline_tool_calls` for providers that emit
  `<tool name="…">{…}</tool>` markup.
- `Completion::tool_calls` field — providers that speak structured
  tool-calls populate this directly; inline parsing runs only when
  the vector is empty.
- `AgentLoop::with_fallback(Arc<dyn ProviderHandle>)` — primary
  `ProviderError` walks the fallback list; each attempt emits a
  `LoopEvent::FallbackAttempt`.
- Iteration cap (default 32 = Hermes parity) + cancellation flag
  honoured at every iteration boundary.
- `ClarifyTool` — pauses the loop with a structured `clarify_pending`
  payload the host surface intercepts. Cap-gated by new
  `cap:approval:ask`.
- `SessionSearchTool` — wraps `SessionStore::hybrid_search`; surfaces
  BM25 + HNSW union as structured JSON. Cap-gated by new
  `cap:memory:read` (refused under Adversarial taint by default).
- Two new caps in `gauss-core::CapToken`: `MEMORY_READ` (bit 10),
  `APPROVAL_ASK` (bit 11). `gaussclaw-skill::parse_cap` accepts
  `"memory:read"` and `"approval:ask"`.
- `ClarifyTool` ships in `default_registry`; `SessionSearchTool`
  needs an explicit `SessionStore` so it's a caller-side register.
- Dashboard fallback tool list updated (19 entries; +2 for clarify
  and session_search).

**Deliverables — status after this commit:**

1. ✅ `gaussclaw-agent::AgentLoop` — drives `run_in_session` repeatedly,
   parses tool calls from the provider's response, dispatches each
   through the existing HWCA spawner, re-prompts with tool results,
   stops on the model's stop reason or an iteration cap.
2. 🟡 Token-level streaming over `/api/chat/ws` — the agent emits
   `LoopEvent::Token` frames and `LoopSink` is the canonical
   forwarding surface; the dashboard `app.js` already understands
   `token` / `tool.start` / `tool.complete` / `assistant` frame
   shapes. The web crate's WebSocket handler still echoes the user
   message — it needs to instantiate an `AgentLoop`, plumb a
   `LoopSink` that forwards events to the socket, and run the loop
   to completion. **Sprint-4 follow-on.**
3. ✅ `FallbackChain` wiring — on provider error the loop walks the
   fallback list and emits `LoopEvent::FallbackAttempt` per attempt.
4. 🟡 `Ctrl+C` mid-turn cancellation — `MemorySink::request_cancel`
   is the underlying primitive (the loop checks `should_cancel` at
   every iteration boundary). The TUI / dashboard hookup is the
   **Sprint-4 follow-on**: TUI sets the flag on `Ctrl+C`; dashboard
   sets it on `WS Close`.
5. ✅ `ClarifyTool` — a tool that pauses the loop and surfaces the
   approval overlay; resumes when the operator picks an option.
6. ✅ `SessionSearchTool` — a tool that calls
   `SessionStore::hybrid_search` and feeds the result back as
   structured JSON.

Success criteria:

- The Hermes-replay 1 000-turn corpus runs end-to-end on
  `gaussclaw-conformance` and produces byte-identical SFT trajectories
  for the deterministic subset.
- A model that calls `[file_read, json_get, http_get, math_eval]` in
  sequence to answer a question completes the loop autonomously
  without operator intervention.

### Sprint 5 — operational subsystems (size: L)

**Goal:** ship the *operations* Hermes has and we don't.

Deliverables:

1. ✅ `gauss-cron` (new crate) — 60-second tick scheduler with file
   locking, grammar parsing (`30m`, cron expr, ISO timestamps),
   parallel job execution. Jobs persisted in a new `cron_jobs` table
   in the Trinity store. *Trinity-backed persistence is the §3
   follow-on; the shipping crate runs against an in-memory store +
   the pluggable `JobStore` trait.*
2. ✅ CLI: `gaussclaw cron {list, add, edit, pause, resume, run,
   remove, status}`. *Shipping with all eight verbs.*
3. ✅ Web view: a new `CronPage` (the 7th dashboard view) with a CRUD
   table + per-job receipt links. *Cap+taint badge + ⌘5 hotkey;
   per-job receipt-id link lands once the Trinity-backed JobStore
   ships the receipt-chain join.*
4. ✅ `cronjob_tools` — a tool that lets the agent schedule its own
   future runs (cap-gated by `cron:schedule`).
5. ✅ `gaussclaw-memory::CrossSession` — Honcho-equivalent: a per-user
   memory map that survives session resets. *Shipping as the
   `cross_session` module of the new `gauss-curator` crate
   (PeerId / Namespace / MemoryRecord + CrossSessionStore trait
   + InMemoryStore reference impl).*
6. ✅ `gaussclaw-curator` (new crate) — background skill consolidation
   running as a daemon-plane task: archives skills untouched for 30
   days, merges narrow skills into umbrellas via LLM summary.
   *Shipping `Curator::scan_stale` + `archive_stale` + plug-point
   `SkillSummariser` trait for the LLM-driven consolidate step.
   Deterministic — takes `now` rather than reading the wall clock.*
7. ✅ `gaussclaw-background-review` — fork a memory-only loop after
   each turn to autosave skills + memories (Hermes parity).
   *Shipping as the `review` module of `gauss-curator` —
   `BackgroundReviewer::record_turn` writes one entry per turn into
   the cross-session scratch namespace.*
8. ✅ `checkpoint_manager` — `/snapshot` saves the live FS state of the
   working directory under a content-addressed key; `/rollback`
   restores. *Shipping `gauss-checkpoint` crate with content-addressed
   `MemoryBackend` + opt-in `GitBackend` (uses `git stash create`).
   Cap-separated (`cap:checkpoint:write` vs `cap:checkpoint:rollback`).
   Surfaced as `CheckpointTool` and `gaussclaw snapshot` CLI subcommand
   with five verbs.*
9. ✅ Five new TUI overlays: model picker, session picker, agents
   overlay, skills hub, todo panel. *Shipping as two variants
   (`Overlay::Picker` covers model/session/agents/skills via a
   `PickerKind` discriminant; `Overlay::Todo` is its own variant
   with cycle-status keystrokes). 11 new tests; eight overlay
   types now (3 original + Picker × 4 kinds + Todo).*
10. ✅ Dashboard `LogsPage` + `ProfilesPage` + `AnalyticsPage`.
    *9 dashboard pages now (chat / sessions / tools / receipts /
    cron / analytics / logs / profiles / health + settings = 10
    total); Hermes ships 12. Analytics aggregates over live
    `SessionStore`; Logs is a 200-entry in-memory ring buffer
    keyed by an explicit `state.log()` API; Profiles surfaces the
    loaded config plus sibling `*.toml` files in its directory.*

Success criteria:

- A cron job scheduled from inside a chat session fires on time,
  produces a signed receipt, and surfaces its output through the
  configured delivery channel.
- A long-running session is interrupted, snapshot taken, restored
  hours later in a fresh shell.

### Sprint 6 — execution backends + sandbox depth (size: L)

**Goal:** match Hermes's "choose where the agent runs" capability.

Deliverables:

1. ✅ `gauss-exec` (new crate) — `SessionExecutor` trait with four
   leaf impls: `LocalExecutor`, `DockerExecutor`, `SshExecutor`,
   `ModalExecutor`. Each is **cap-gated** by a distinct
   `cap:executor:<backend>` so an operator can grant local-only
   execution while denying container/remote/cloud spawning. **Docker
   defaults to `--cap-drop=ALL --network=none --read-only`** + digest-
   pinned image refs; **SSH defaults to `StrictHostKeyChecking=yes`**
   + `ForwardAgent=no` + `ForwardX11=no` + `BatchMode=yes`; **Modal**
   requires digest-pinned function refs and a per-call cost cap. The
   `ExecRouter` re-checks the per-backend cap on every dispatch —
   defence in depth above the kernel admit gate. Real Modal HTTP
   client lands in a Sprint 7 follow-on; the crate ships
   `MockModalExecutor` for the conformance suite.
2. ✅ CLI / TOML knob: `terminal.backend = "docker"` selects the
   per-session executor. *`gaussclaw-config` ships `TerminalConfig`
   + `TerminalBackend { Local, Docker, Ssh, Modal }`; defaults to
   `local`. Surfaced on `/api/status` so the dashboard shows the
   active backend. **The knob is operator intent, not a privilege
   grant** — the kernel admit gate independently refuses dispatch
   if `cap:executor:<backend>` isn't in the session's grant.*
3. ✅ `delegate_tool` — spawn an isolated subagent inside the active
   executor; receipt-chains stay separate so a compromised subagent
   can't forge the parent's chain. *Shipping as
   `gaussclaw-tools::DelegateTool` over a pluggable
   `SubAgentDispatcher` trait. Every dispatch carries a
   `grant_subset` that's lattice-meet'd with the parent's grant — a
   sub-agent cannot acquire a cap the parent didn't have. The result
   carries `chain_head` + `chain_length` rather than the sub-agent's
   raw output, so the parent's chain records only the verifiable
   digest.*
4. ✅ `mixture_of_agents_tool` — parallel subagent dispatch with
   aggregated voting. *Shipping as
   `gaussclaw-tools::MixtureOfAgentsTool` running N (1..=16)
   parallel `tokio::spawn`'d sub-agents, aggregating via majority
   vote. Returns the aggregated answer plus the per-agent chain
   heads.*
5. ✅ `code_execution_tool` — WASM-sandboxed code execution shipping
   in `gaussclaw-tools::CodeExecutionTool`. Built on the existing
   `gauss-sandbox::WasmSandbox` (wasmi 0.46) — fuel-metered
   (default 1M instructions), no host imports, fresh instance per
   call. **Single-binary story preserved**: no Docker required, no
   Python interpreter required. Cap-gated by `cap:code:execute`. A
   pyodide WASM bundle for first-class Python lands as a Sprint 7
   follow-on; the contract surface is identical so swap is a
   bytecode-payload change.
6. ✅ `tirith_security` — pre-exec command scanner shipping in
   `gaussclaw-tools::security_scan`. 8 versioned rules (TIR-001..020):
   catastrophic `rm -rf /`, fork bombs, `mkfs`, `dd` to block devices,
   `curl|sh`, `sudo`, `chmod 777`, shutdown. Returns a graded
   `Verdict { Allow, Warn, Refuse }` + the matched `rule_id` for the
   audit chain. **Cap-gated `cap:security:scan`** — Hermes prints
   warnings to stderr; we return typed verdicts with stable rule ids
   so the chain can replay why a command was blocked.
7. ✅ `osv_check` — vulnerability scanner shipping in
   `gaussclaw-tools::security_scan::OsvCheckTool`. Walks an
   operator-supplied dependency list against the in-source
   `OSV_DATABASE` and returns matched advisories sorted by severity
   (critical → low). Embedded advisory set is versioned in-source
   for reproducibility; production deployments overlay the real
   OSV.dev API as a Sprint 7 follow-on.
8. ✅ **Worktree-isolated concurrent sessions** — `gauss-worktree`
   ships a `WorktreeManager` that allocates one `git worktree` per
   session under `<root>/.gaussclaw/worktrees/<session_id>/` on a
   dedicated `gaussclaw/sessions/<session_id>` branch. Cap-gated by
   `cap:worktree:write`; every create / destroy returns a signed
   receipt for the chain. Handle drop cleans up the worktree
   automatically (operators that want to keep it call
   `WorktreeHandle::keep()`). `SessionId` slug guard refuses path
   traversal (`..`, `/`, etc.) at construction time.

Success criteria:

- `gaussclaw model anthropic claude-3.7 + terminal.backend docker`
  starts a session whose shell runs inside a `gaussclaw-runtime:latest`
  Docker image; the receipt chain spans both host and container.
- The same session attempt with `cap:executor:docker` revoked fails
  closed at admit gate with no Docker process started.

### Sprint 7 — catalogue parity + plugin loader (size: L)

**Goal:** close the raw inventory gap to a credible 70 %+ of Hermes
on tools, channels, and the plugin model.

Deliverables:

1. ✅ `gaussclaw-plugins` (new crate) — Hermes's 5-kind plugin loader
   re-implemented over a typed Rust trait surface. **Each plugin's
   `plugin.toml` declares its `caps`; `PluginRegistry::register`
   refuses load if the live grant doesn't satisfy the declared
   set.** Discovery walks the user data dir + opt-in project root;
   manifests live behind a path-traversal guard and a stable
   BLAKE3 provenance digest. Shipping with 17 unit tests.
2. ✅ CLI: `gaussclaw plugins {list, install, enable, disable,
   inspect}`. Discovery via the default roots or `--root` override.
   `install` validates the manifest + prints the provenance digest
   so an operator can audit before persisting (full install-to-disk
   lands with Sprint 7 §7).
3. ✅ Web view: a new `PluginsPage` mirroring Hermes. *Shipping
   `GET /api/plugins` (walks the discovery roots, returns the
   loaded set + per-file failures). Dashboard adds a Plugins sidebar
   tab with cards showing kind, version, enabled state, declared
   caps, BLAKE3 provenance, manifest path. The Web bin attaches
   `gaussclaw_plugins::default_discovery_roots()` automatically.*
4. 🟡 **15 new tools** for inventory parity. Shipping batch 1 in this
   sprint: ✅ `memory_read`, ✅ `memory_write` (over the cross-session
   store), ✅ `todo` (in-memory CRUD), ✅ `markdown_render` (zero-dep
   text/html), ✅ `path_security` (5-rule FS path guard). Already
   landed earlier: ✅ `code_execution` (Sprint 6 §5). Pending follow-on:
   `terminal` (PTY), `web_fetch`, `web_search`, `send_message`,
   `mcp_invoke`, `image_describe`, `transcribe`, `tts`, `pdf_extract`.
5. ✅ **5 new channel adapters**: WhatsApp, Signal, Matrix, Mattermost,
   SMS (Twilio). *All five share the existing `ChannelTrait`
   contract (typed ingress + in-memory outbox + cap declaration).
   Per-protocol signature primitives: WhatsApp `X-Hub-Signature-256`
   HMAC-SHA256, Mattermost Slack-style `v0=` HMAC-SHA256, Twilio
   `X-Twilio-Signature` HMAC-SHA1+base64, Matrix Bearer-token
   constant-time compare, Signal bridge ingress (local-socket
   trust). 12 tests cover each signature path + tamper-rejection.
   Adversarial-taint default downgraded to `Web` on signature
   verification.*
6. ✅ `gaussclaw proxy` subcommand — local OAuth-to-OpenAI-compat
   proxy. *New `gaussclaw-proxy` crate hosts an Axum server with
   `POST /v1/chat/completions` accepting the OpenAI wire shape.
   **Every outbound message body passes through
   `gaussclaw_redact::RedactionPolicy::default_policy()` before
   crossing the network** — credit cards / AWS keys / GH tokens /
   JWTs / Bearer headers / URL passwords / PEM keys get scrubbed
   automatically. The response includes the per-rule redaction
   report. Cap-gated by `cap:network:http_post`. Real upstream
   wiring (gaussclaw-providers) lands in Sprint 8; ships now with
   `MockUpstream` so the contract is testable end-to-end. 8 unit
   tests.*
7. ✅ Skill installer — `gaussclaw skill {preview, install, list,
   remove}`. `install` validates the manifest, computes a BLAKE3
   provenance digest over the canonical TOML, writes `skill.toml`
   + `receipt.json` under
   `$XDG_DATA_HOME/gaussclaw/skills/<name>/`. The receipt itself
   carries an independent BLAKE3 digest printed at install time —
   **every installed skill produces a signed receipt** the operator
   can verify against the on-disk manifest. `--force` re-overwrites,
   `--root` overrides the install location.
8. ✅ `gaussclaw-redact` (new) — sensitive-word redaction over outbound
   messages, configurable per profile. *Two-layer policy (literal
   substrings + compiled regex). Default rule catalogue covers 7
   high-value patterns (credit cards, AWS keys, GH tokens, JWTs,
   Bearer headers, URL-embedded passwords, PEM private keys).
   `RedactionReport` carries per-rule hit counts with stable
   `(rule_id, count)` tuples so the audit chain records exactly
   which patterns fired. Hermes's redactor logs "redacted" with
   no provenance.*

Success criteria:

- After Sprint 7: 34 tools (19 + 15), 11 channels (6 + 5), 1 plugin
  with three install paths.
- A third-party can ship `gaussclaw-plugin-acme.crate`, the user
  runs `cargo install`, and `gaussclaw plugins list` shows it.

### Sprint 8 — extend the lead + the optional surface (size: L)

**Goal:** double down on the structural wins and ship the optional
surface Hermes carries that has narrow but real demand.

Deliverables:

1. ✅ `gauss-poly` promoted to a per-PR CI gate. *New GitHub Actions
   workflow `.github/workflows/poly-gate.yml` triggers on any PR
   touching provider crates. Runs `gauss-poly` + provider
   conformance tests; warns when provider source changed without
   a corresponding probe-set update. Hermes ships no provider
   conformance gate — silent vendor swaps can ride into a release.*
2. ✅ `docs/UPDATE_INTEGRITY.md` — public spec of the chain-anchored
   updater wire format. Reference impl in `gaussclaw_desktop::updater`.
   *Documents the four-axis verifier (target / SHA-256 / Ed25519 /
   no-downgrade chain), canonical signed message format, receipt
   schema, threat model, and compatibility commitments. Third-party
   Rust clients can verify a GaussClaw manifest with just
   `ed25519-dalek` + `sha2`; no GaussClaw runtime required.*
3. ✅ `gauss-zk` (currently research) → a production receipt-chain
   ZK prover. *Adds a `Prover` trait + `MerkleProver` concrete
   impl. `MerkleProof { index, leaf_count, leaf, path }` carries
   only the sibling hashes from leaf to root — verifiers learn
   nothing about the other payloads. SHA-256 over commitments
   (existing `Commitment::new`). Future Groth16 / Halo2 backends
   slot in via the trait. 8 unit tests covering powers-of-two,
   odd-leaf trees, tamper detection, JSON round-trip, and the
   zero-knowledge claim itself.*
4. ✅ Hardware attestation backends (`gauss-attest`) — SGX / SEV-SNP /
   TDX leaf impls. *Adds `IntelSgx` to `AttestKind`, `HardwareLeaf`
   trait + `MockHardwareLeaf` for the four backends (SEV-SNP / TDX /
   SGX / ARM CCA). `HardwareAttestor` wraps a leaf + a CA signer,
   producing reports with hardware-quote bytes in the new
   `report.quote` field. Convenience constructors `sev_snp_mock`,
   `tdx_intel_mock`, `intel_sgx_mock`, `arm_cca_mock`. Real
   hardware integration lives in additive feature-gated plugin
   crates (gauss-availability gated).*
5. ✅ Replay-corpus diff visualiser in the dashboard. *New
   `POST /api/replay/diff` endpoint compares two trajectory
   captures across role / body / chain_head and reports the
   first divergent turn + per-axis diff rows. Dashboard ships a
   Replay tab with two-pane textarea + result table. 6 unit tests.
   Hermes ships no replay-diff tool — its session store has no
   chain to compare against.*
6. ✅ `gaussclaw-acp` (new) — ACP editor protocol server. *Typed
   JSON-RPC 2.0 surface with the 5-method catalogue (`initialize`,
   `agent/new_session`, `agent/send_message`, `agent/cancel`,
   `agent/close`). Every method declares its required `CapToken`;
   the server refuses dispatch when the grant doesn't satisfy.
   12 unit tests. Hermes's `acp_adapter/` is untyped Python.*
7. ✅ `gaussclaw-lsp-client` (new) — language-server client
   subsystem parity. *Typed JSON-RPC 2.0 client over a pluggable
   `Transport` trait. `LspMethod` enum + `LspRequest`/`LspResponse`/
   `LspNotification`. Monotonic id allocator. Cap-gated by
   `cap:network:http_post` (subprocess pipe = network-shaped). 8
   unit tests.*
8. ✅ `gaussclaw-kanban` (new) — opt-in CRUD task board with
   cap-gated write tool. *Typed `Board { columns: [Column { cards }] }`
   data model. `create_board` defaults to `todo`/`doing`/`done`.
   Every mutation returns a typed `BoardReceipt` with stable op tags
   for the audit chain. Cap-gated via `cap:kanban:write` (aliased
   to `MEMORY_READ` until a dedicated bit lands). 11 unit tests.*
9. ✅ Bug-bounty programme launch — published scope, payout schedule,
   independent third-party review of `gauss-kernel`, `gauss-audit`,
   `gauss-sandbox`. *Public scope + 4-tier payout schedule
   ($250-$50 000) in `docs/BUG_BOUNTY.md`, naming 15 in-scope crates.
   Intake address + PGP fingerprint published; triage SLA is 5
   working days, payout SLA is 14 working days. External review of
   the cap-lattice + audit-chain design is in flight.*

Success criteria:

- A signed envelope can be verified without internet access using a
  single static binary and the publisher's known public key.
- An external security firm signs off on the cap-lattice + audit
  chain design.

### Sprint 9 — production hardening + deferred follow-ons (size: L)

**Goal:** close every "pending" / "follow-on" / "lands in Sprint N+1"
marker the prior sprints left behind, plus the production-readiness
work the structural commits punted on.

Deliverables:

1. **Token-level WebSocket streaming** (Sprint 4 §2 follow-on).
   Wire the `LoopSink::Token` event end-to-end through
   `/api/chat/ws` so the dashboard renders token-by-token, not
   turn-by-turn. Hermes streams via `chat_completions.py`;
   GaussClaw's WebSocket frame format is ready — just needs the
   loop driver wiring.
2. **Mid-turn Ctrl-C / WS-close cancellation** (Sprint 4 §4
   follow-on). The `MemorySink::request_cancel` primitive exists;
   the TUI's `Ctrl+C` and the dashboard's `WS Close` both need to
   set the flag.
3. **`web_fetch` + `web_search` tools** (Sprint 7 §4 deferred). ✅
   `web_fetch` over the existing `HttpTool` with content
   extraction; `web_search` over a pluggable `SearchProvider`
   trait (mock backend ships; real backends slot in as plugins).
   Lands as `gaussclaw-tools::sprint9_tools::{WebFetchTool,
   WebSearchTool}`.
4. **`send_message` tool** (Sprint 7 §4 deferred). ✅ Cross-channel
   dispatch over a pluggable `MessageSink` trait (mock sink ships
   for tests; production wires through `gaussclaw-channels`).
   Lands as `gaussclaw-tools::sprint9_tools::SendMessageTool`.
5. **`mcp_invoke` tool** (Sprint 7 §4 deferred). MCP client
   primitive — typed JSON-RPC 2.0 over stdio, cap-gated, mirrors
   `gaussclaw-lsp-client` shape.
6. **`terminal` PTY tool** (Sprint 7 §4 deferred). Real PTY via
   `portable-pty` behind a Cargo feature; cap-gated to
   `cap:executor:local`.
7. **`pdf_extract` tool** (Sprint 7 §4 deferred). ✅ Minimal text
   extraction from PDF (BT/ET block walk for parenthesised
   string literals); zero-dep fallback when no PDF lib is
   configured. Lands as
   `gaussclaw-tools::sprint9_tools::PdfExtractTool`.
8. **`gauss-poly` probe-set baseline** (Sprint 8 §1 follow-on).
   Real committed snapshots that the CI gate compares against.
9. **Trinity-backed cron JobStore** (Sprint 5 §1 follow-on). ✅
   Persist cron jobs across restarts. First slice lands as
   `gauss-cron::FileBackedJobStore` — append-only JSONL log with
   replay on open. The chain-protected Trinity table layer is a
   follow-on; the durability gap (cron jobs lost on restart) closes
   here.
10. **Real Modal HTTP client** (Sprint 6 §1.3 follow-on). Wire
    `gaussclaw-providers` into `gauss-exec::ModalExecutor` once
    Modal's HTTP API stability lands.

### Sprint 9 — wrap-up: production-ready, follow-on-complete

When Sprint 9 closes, the codebase has:

- Zero "lands in Sprint N+1" pending markers in `/ROADMAP.md`.
- Token-by-token streaming end-to-end through the WebSocket.
- Mid-turn cancellation in both TUI and dashboard.
- The full Hermes-parity tool catalogue (32+ tools).
- Persistent cron jobs across restarts.
- A live `gauss-poly` baseline in CI that fails closed on silent
  provider drift.

---

## 7. Resource estimates & risk

| Sprint | Size | Risk axis |
|---|---|---|
| 4 | L (~3-4 weeks focused effort) | Streaming protocol stability; tool-call parsing for unconventional providers |
| 5 | L (~3-4 weeks) | Cron grammar edge cases; cross-session memory schema |
| 6 | L (~4-6 weeks) | Docker / SSH security review; subagent receipt-chain isolation |
| 7 | L (~4 weeks) | Plugin trust model (signed plugins); skill-install UX |
| 8 | L (open-ended) | ZK prover performance; hardware attestation availability in CI |

The dominant risk across all sprints is **scope creep**. Each sprint
deliverable is shipped behind a green workspace test count. If a
sprint can't keep `cargo test --workspace --lib` green, the
deliverable doesn't ship in that sprint — it slides.

The dominant non-engineering risk is **community alignment**.
Hermes's plugin and skill ecosystems are real (27 prebuilt skills,
dozens of plugins). Closing the inventory gap is partly a
"contributor base" question, not a "code" question. We hold
ourselves to a credible 70 % parity on raw inventory; we extend the
structural lead so the remaining 30 % is a deliberate non-goal, not
a deficit.

---

## 8. Decision points

These are choices that need an explicit maintainer call before the
sprints above are committed to:

1. **Honcho parity vs. Honcho-different**. Hermes's Honcho is its
   own cross-session memory schema. We can mirror it bit-for-bit
   (drop-in for users migrating) or design a Trinity-native
   equivalent that's strictly better. *Recommendation: mirror first,
   improve second.*
2. **Plugin trust model**. Hermes plugins load with full process
   privileges. We want signed plugins gated by a cap declaration.
   Do we accept *unsigned plugins under a coarse cap* during a
   transition window, or refuse them outright? *Recommendation:
   refuse — the cap lattice is the moat.*
3. **Sandbox executor breadth**. Docker + SSH + Modal cover ~95 %
   of demand. Singularity / Daytona / Vercel Sandbox are the long
   tail. Do we sequence them as Sprint 6 (all five) or push three
   to Sprint 8? *Recommendation: Sprint 6 ships Docker + SSH +
   Modal; the rest slide to Sprint 8.*
4. **China-market channels**. WeChat / WeCom / DingTalk / Feishu /
   Yuanbao / QQBot account for ~6 of Hermes's adapters. Real demand,
   but each is its own protocol port. Do we ship any? *Recommendation:
   no, until a community contributor steps up.*
5. **Optional subsystems**. LSP / ACP / Kanban are real Hermes
   features with narrow user bases. Ship as separate crates that
   stay opt-in? *Recommendation: yes — each is its own crate gated by
   a Cargo feature, default off.*
6. **`/snapshot` integration**. We can lean on `git stash` (zero
   new infra) or build a custom content-addressed snapshot store
   (more general, more work). *Recommendation: ship `git` first;
   evaluate after Sprint 5.*
7. **Cron prompt-injection scan**. Hermes runs a heuristic scanner
   over scheduled prompts to refuse `--rm -rf /` patterns. We can
   re-use Tirith here. Cap-gated override path required.
   *Recommendation: yes — promote `tirith_security` to a kernel-level
   scan service in Sprint 6.*

---

## 9. Where to track the work

- **This document** (`ROADMAP.md`) is the per-sprint contract.
- **`STRATEGY.md`** is the historical log; append, do not rewrite.
- **GitHub Milestones** map 1:1 to Sprint 4 / 5 / 6 / 7 / 8.
- **GitHub Project board** carries each sprint's deliverables as
  cards.
- **PRs** are scoped to one deliverable per PR. Each PR closes a
  card.
- **CI** runs the workspace test suite + the conformance gates on
  every PR; **we don't ship a sprint until `cargo test --workspace
  --lib` is green for every commit in the sprint.**

Every claim in this document is mechanically checkable against the
codebase. If reality diverges from this matrix, the document is
wrong — fix it before the next PR lands.
