---
id: web
title: Web dashboard
sidebar_position: 6
---

# Web dashboard

The dashboard runs on the Axum backend in `gaussclaw-web` and ships as
vanilla HTML + CSS + JS — **no npm at build time, no Node.js at runtime,
no Chromium bundled** — embedded into the binary via `rust-embed`. The
whole bundle is under 60 KB.

Hermes ships a React 19 + Vite + Tailwind 4 frontend whose stated scope is
"config editor, env / API-key page, agent status." GaussClaw's dashboard
ships six fully interactive views in a fraction of the size.

## Launch

```bash
gaussclaw serve --port 8080
```

Opens at [http://127.0.0.1:8080/](http://127.0.0.1:8080/). **Runs natively
on Linux, macOS, and Windows** — Hermes upstream needs WSL2 because its
dashboard chat pane uses a POSIX PTY; GaussClaw streams over WebSocket
instead.

## The six views

| View | Hermes equivalent | What it shows |
|---|---|---|
| **Chat** | not present | Streaming WebSocket chat, multiline composer, tool-activity lane, capability + taint chips. |
| **Sessions** | not present | Full-text + vector search over every conversation; resume by id. |
| **Tools** | not present | Every registered tool, its capability requirement, taint label, and the sandbox layers it runs inside. |
| **Receipts** | not present | The active receipt-chain head, latest turn, TSA anchor status, and a copy-to-clipboard button for verifications. |
| **Health** | StatusPage (subset) | The seven Self-Diagnostic Health Engine invariants from `gauss-health`, with status badges. |
| **Settings** | ConfigPage + EnvPage | Active provider, model, profile; provider catalogue; telemetry policy; runtime metadata. |

## UX features

- **Command palette** — `⌘K` / `Ctrl+K` opens a fuzzy command picker with keyboard navigation. Switch views, copy the chain head, reload health, reload tools, all without leaving the keyboard.
- **Tab keyboard shortcuts** — `⌘1`…`⌘6` jumps to Chat/Sessions/Tools/Receipts/Health/Settings.
- **Dark mode by default** with full light-mode parity via `prefers-color-scheme`. Cyan accent.
- **Responsive** — collapses to an icon-only sidebar below 900 px width; the chat-activity lane stacks on mobile.
- **Live WebSocket reconnection** — the sidebar status dot turns amber on disconnect and green when reconnected; messages queued during downtime are surfaced to the user instead of silently lost.
- **Accessible** — proper ARIA roles for the sidebar tabs, the command palette dialog, the live transcript, and the toast region.

## API endpoints

The same Axum router serves both the static dashboard assets and the
JSON API. Every API response wears the uniform envelope
`{ok:true,data:...}` or `{ok:false,error:{code,message}}`.

| Method | Path | Description |
|---|---|---|
| GET  | `/api/status`        | Liveness + version + active provider/model. |
| GET  | `/api/health`        | SDHE invariants snapshot. |
| GET  | `/api/config`        | Active config tree. |
| GET  | `/api/config/schema` | JSON schema for the config tree. |
| POST | `/api/config`        | Patch a config value (cap-gated; 403 today). |
| GET  | `/api/sessions`      | Recent sessions (FTS-searchable). |
| GET  | `/api/providers`     | Provider catalogue. |
| GET  | `/api/tools`         | Tool catalogue. |
| GET  | `/api/receipt/head`  | **Live** audit-chain head digest + turn. |
| WS   | `/api/chat/ws`       | Chat WebSocket — token + tool-event stream. |
| GET  | `/` *and* `/{*path}` | Static dashboard assets + SPA fallback. |

## Why no React build step

Hermes's web bundle is a Vite + React + Tailwind + Three.js stack that
needs Node.js to compile. Ours doesn't. That matches GaussClaw's central
promise: **one static binary, no runtime dependencies, no build-time
dependencies for the shipped artifact**. Releasing a new dashboard is a
matter of editing three files in `gaussclaw-web/frontend/dist/`; the
`rust-embed` macro picks them up at the next `cargo build`.

## WebSocket message protocol

The chat WebSocket accepts either plain text or a JSON envelope:

```json
{ "type": "user", "text": "your message" }
```

The server can stream back either format. The frontend understands the
following event types:

| `type` | Fields | Meaning |
|---|---|---|
| `token` | `{text}` | A streamed token to append to the current assistant turn. |
| `assistant` | `{text}` | A complete assistant message in one frame. |
| `tool.start` | `{tool, args}` | A tool call is about to run. |
| `tool.progress` | `{tool, note}` | Progress notification from the tool. |
| `tool.complete` | `{tool, result}` | Tool finished; result is on the way. |
| `receipt` | `{digest, turn, anchor?}` | New receipt-chain head after a turn commits. |
