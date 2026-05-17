# Hermes Adapter Matrix

The Phase 1 Task 1 audit deliverable from `GAUSSCLAW_ROADMAP.md`. This
document catalogues every Hermes surface, channel, and entry point the
GaussClaw port must replace, identifies the upstream stack each is
built on, and binds it to the GaussClaw crate that takes over.

It is the source of truth for:

- `gaussclaw-cli::SUBCOMMANDS` (the parity table).
- `gaussclaw-conformance::cli_parity::HERMES_SUBCOMMANDS` (the parity gate).
- The per-surface `surface` field value written into every turn record
  (preserved bit-for-bit under Principle 1).

Upstream reference: [github.com/NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent).
Audit cadence: re-run at the start of each phase; every upstream
release that touches a surface gets a row update in the same PR.

---

## 1. CLI entry points

| Hermes command | Behaviour (upstream) | GaussClaw subcommand | Crate | Phase |
|---|---|---|---|---|
| `hermes` | Launch the interactive TUI / chat | `gaussclaw` (no subcommand) | `gaussclaw-tui` | P1 |
| `hermes model` | Interactive model picker | `gaussclaw model {list,show,set}` | `gaussclaw-providers` | P4 |
| `hermes tools` | Enable / disable tools | `gaussclaw tools {list,show,enable,disable}` | `gaussclaw-skill`, `gaussclaw-tools` | P3 |
| `hermes config set <k> <v>` | Persist a config value | `gaussclaw config {list,get,set,path}` | `gaussclaw-config` | P1 |
| `hermes gateway` | Start the messaging gateway | `gaussclaw gateway {start,stop,status}` | `gaussclaw-channels` | P1 |
| `hermes setup` | First-run wizard | `gaussclaw setup [--non-interactive]` | `gaussclaw-config`, `gaussclaw-migrate` | P1 |
| `hermes update` | Self-update | `gaussclaw update [--channel <ch>]` | `gaussclaw-desktop` updater | P5 |
| `hermes doctor` | Diagnose installation | `gaussclaw doctor [--json]` | `gauss-health` (SDHE) | P1 |
| *(new in port)* | One-shot chat REPL | `gaussclaw chat [-m TEXT] [-s ID]` | `gaussclaw-agent` | P1 |
| *(new in port)* | Migrate from Hermes | `gaussclaw import <hermes-config>` | `gaussclaw-migrate` | P1 |
| *(new in port)* | Receipt-chain inspection | `gaussclaw receipt {head,verify}` | `gauss-audit`, `gaussclaw-export` | P2 / P5 |

**Parity gate.** `gaussclaw-conformance::cli_parity::hermes_subcommands_are_all_covered`
asserts every Hermes row appears as a GaussClaw subcommand.
`parity_flags_are_truthful` asserts every `(name, true)` row in
`SUBCOMMANDS` corresponds to a real Hermes entry — no over-claim.

---

## 2. UI surfaces

| Upstream path | Stack | GaussClaw crate | GaussClaw stack | Phase |
|---|---|---|---|---|
| `ui-tui/` | TypeScript · React · forked **Ink** renderer (`packages/hermes-ink`) | `gaussclaw-tui` | Rust · **Ratatui** + **crossterm** + `tui-textarea` | P1 |
| `web/` (frontend) | React 19 · Vite · Tailwind CSS v4 · shadcn-style primitives | `gaussclaw-web` (`/frontend`) | **Retained verbatim**; served via `rust-embed` | P1 |
| `web/` (backend) | FastAPI · **POSIX PTY** (WSL2-only) | `gaussclaw-web` (`/backend`) | Rust · **Axum** · WebSocket streaming | P1 |
| `tui_gateway/` | Python TUI ↔ gateway IPC | folded into `gaussclaw-tui` + the Conversation plane | Rust · in-process channel | P1 |
| `website/` | **Docusaurus** static site · i18n (en + `zh-Hans`) | `gaussclaw-website` (`/site`) | **Docusaurus** retained · same i18n | P1 → GA |
| Hermes Desktop ([hermes-ai.net/desktop](https://hermes-ai.net/desktop/)) | **Electron 39** · Chromium-bundled · HTTP-on-`127.0.0.1:8642` · unsigned | `gaussclaw-desktop` | **Tauri 2** · system WebView · typed-IPC sidecar · Ed25519-signed | P1 → P5 |

**Conformance.** TUI snapshot tests via `insta`; web e2e via Playwright;
desktop e2e via `webdriverio + tauri-driver` on macOS, Windows, Linux.

---

## 3. Network surfaces

| Hermes endpoint | Wire shape | GaussClaw crate | Phase |
|---|---|---|---|
| REST `/v1/...` | HTTP · JSON | `gaussclaw-surfaces::rest` (Axum) | P1 |
| WebSocket | JSON frames + token stream | `gaussclaw-surfaces::ws` (Axum upgrade) | P1 |
| OpenAI-compatible relay (`/v1/chat/completions`, `/v1/completions`, `/v1/models`) | OpenAI Chat Completions JSON + SSE stream | `gaussclaw-surfaces::oai_compat` | P1 |

**Parity gate.** The OpenAI Python SDK's official end-to-end suite is
parametrised by both backends; both must pass identically.

---

## 4. Messaging channels

Hermes upstream ships ~16 messaging gateways (the Desktop page advertises
"15+"). The GaussClaw port preserves every one as a `ChannelTrait`
impl in `gaussclaw-channels`, with auth secrets resolved through
`gauss-attest` (never round-tripped through the Python shim).

| Channel | Adapter module | Auth model | Phase |
|---|---|---|---|
| Telegram | `gaussclaw-channels::telegram` | Bot token | P1 |
| Discord | `gaussclaw-channels::discord` | Bot token + OAuth | P1 |
| Slack | `gaussclaw-channels::slack` | OAuth · signing-secret webhook verify | P1 |
| WhatsApp | `gaussclaw-channels::whatsapp` | WhatsApp Cloud API token | P1 |
| Signal | `gaussclaw-channels::signal` | signal-cli daemon | P1 |
| Matrix | `gaussclaw-channels::matrix` | access token | P1 |
| Email (IMAP/SMTP) | `gaussclaw-channels::email` | per-account creds | P1 |
| SMS (Twilio) | `gaussclaw-channels::sms` | Twilio API key | P1 |
| Feishu (Lark) | `gaussclaw-channels::feishu` | app id + secret | P1 |
| WeCom | `gaussclaw-channels::wecom` | corp id + secret | P1 |
| BlueBubbles | `gaussclaw-channels::bluebubbles` | server password | P1 |
| Home Assistant | `gaussclaw-channels::home_assistant` | long-lived token | P1 |
| Mattermost | `gaussclaw-channels::mattermost` | personal access token | P1 |
| IRC | `gaussclaw-channels::irc` | SASL | P1 |
| XMPP | `gaussclaw-channels::xmpp` | SASL / OAuth | P1 |
| Generic Webhook | `gaussclaw-channels::webhook` | HMAC | P1 |

The exact upstream list and current channel adapter count are pinned
in the audit at the version recorded in `docs/HERMES_VERSION_PIN.md`
(landed alongside the first channel port).

---

## 5. Provider plane

Hermes enumerates ~20 leaf provider vendors plus three API modes. The
GaussClaw port binds each to `ProviderTrait` and adds two first-class
meta-routers (OpenRouter aggregator and NotDiamond learned router).

| Hermes module | GaussClaw crate | API surface | Phase |
|---|---|---|---|
| `backends.anthropic` | `gaussclaw-providers::anthropic` | Messages API | P4 |
| `backends.openai` | `gaussclaw-providers::openai` | Chat Completions / Responses | P4 |
| `backends.google` | `gaussclaw-providers::google` | Gemini API | P4 |
| `backends.mistral` | `gaussclaw-providers::mistral` | Chat Completions | P4 |
| `backends.together` | `gaussclaw-providers::together` | Chat Completions | P4 |
| `backends.groq` | `gaussclaw-providers::groq` | Chat Completions | P4 |
| `backends.cerebras` | `gaussclaw-providers::cerebras` | Chat Completions | P4 |
| `backends.fireworks` | `gaussclaw-providers::fireworks` | Chat Completions | P4 |
| `backends.deepseek` | `gaussclaw-providers::deepseek` | Chat Completions | P4 |
| `backends.xai` | `gaussclaw-providers::xai` | Chat Completions | P4 |
| `backends.perplexity` | `gaussclaw-providers::perplexity` | Chat Completions | P4 |
| `backends.cohere` | `gaussclaw-providers::cohere` | Chat API v2 | P4 |
| `backends.replicate` | `gaussclaw-providers::replicate` | Predictions API | P4 |
| `backends.octoai` | `gaussclaw-providers::octoai` | Chat Completions | P4 |
| `backends.anyscale` | `gaussclaw-providers::anyscale` | Chat Completions | P4 |
| `backends.huggingface` | `gaussclaw-providers::hf` | Inference Endpoints | P4 |
| `backends.ollama` | `gaussclaw-providers::ollama` | Local · OpenAI-compat | P4 |
| `backends.llamacpp` | `gaussclaw-providers::llamacpp` | Local · llama.cpp server | P4 |
| `backends.vllm` | `gaussclaw-providers::vllm` | Local · OpenAI-compat | P4 |
| `backends.tgi` | `gaussclaw-providers::tgi` | Local · text-generation-inference | P4 |
| *(new)* | `gaussclaw-providers-meta::openrouter` | Aggregator · OpenAI-compat | P4 |
| *(new)* | `gaussclaw-providers-meta::notdiamond` | Learned router · advisory + joint | P4 |

API modes:

| Hermes module | GaussClaw module |
|---|---|
| `api_modes.chat_completion` | `gaussclaw-api-modes::chat_completion` |
| `api_modes.responses` | `gaussclaw-api-modes::responses` |
| `api_modes.oai_compat` | `gaussclaw-api-modes::oai_compat` |

---

## 6. Tool catalogue

Hermes ships ~30 first-party tools across 14 toolsets (web, browser,
terminal, file, code, vision, …). Every tool gets a `Skill Manifest`
(TOML) declaring its `caps`, `taint`, `schema`, `reversible`, and
`cost`. Default taint labels follow the roadmap §"Default taint policy":
web/email/RSS → `web`; shell/file/python → `user`; calendar/contacts/git
→ `trusted`; untrusted channel ingress → `adversarial`.

The full catalogue lands in `crates/gaussclaw-tools/manifests/*.toml`
during Phase 3 Task 4 ("First-party tool port") — one PR per toolset.

---

## 7. Storage and lineage

| Hermes module | Hermes storage | GaussClaw crate | GaussClaw storage | Phase |
|---|---|---|---|---|
| `store.session` | SQLite + FTS5 | `gaussclaw-store::session` | SurrealDB doc + BM25 FTS + HNSW vector | P2 |
| `store.lineage` | SQLite parent-pointer table | `gaussclaw-store::lineage` | SurrealDB `RELATE` graph edge (signed) | P2 |

---

## 8. Trajectory export

| Hermes module | Output | GaussClaw crate | Extension |
|---|---|---|---|
| `export.sft` | `sft.jsonl` | `gaussclaw-export::sft` | Cryptographic Trajectory Envelope (P5) |
| `export.dpo` | DPO preference pairs | `gaussclaw-export::dpo` | Taint-aware filter (P5) |

The wire schema (`prompt`, `completion`, `surface`, `session_id`,
`parent_id`, `ts`, lineage edges, DPO pairs) is preserved field-for-field.
New material is appended in an optional envelope alongside each record,
never inlined into existing fields (Binding Constraint #2).

---

## Version pin

The audit in this document was performed against the upstream
`NousResearch/hermes-agent` repository state observed during PR #1.
A precise commit pin will land in `docs/HERMES_VERSION_PIN.md` once
the first replay-corpus fixture is captured (Phase 1 Task 11,
"Audit-trace recording").
