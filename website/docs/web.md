---
id: web
title: Web dashboard
sidebar_position: 6
---

# Web dashboard

The dashboard runs on the Axum backend in `gaussclaw-web`, serving the
retained Hermes React 19 + Vite + Tailwind frontend embedded into the
binary via `rust-embed`.

## Launch

```bash
gaussclaw web --port 8642
```

Opens at [http://127.0.0.1:8642/](http://127.0.0.1:8642/). **Runs natively on Linux, macOS,
and Windows** — Hermes upstream needs WSL2 because its dashboard chat
pane uses a POSIX PTY; GaussClaw streams over WebSocket instead.

## API endpoints

All endpoints wear the uniform envelope `{ok:true,data:...}` or
`{ok:false,error:{code,message}}` — the shape the retained Hermes
frontend already speaks.

| Method | Path | Description |
|---|---|---|
| GET | `/api/status` | Liveness + version + active provider/model |
| GET | `/api/health` | SDHE invariants snapshot |
| GET | `/api/config` | Active config tree |
| GET | `/api/config/schema` | JSON schema (Phase 3 wires this) |
| POST | `/api/config` | Patch a config value (cap-gated, 403 today) |
| GET | `/api/sessions` | Recent sessions (Phase 2) |
| GET | `/api/providers` | Provider catalogue (Phase 4) |
| GET | `/api/tools` | Tool catalogue (Phase 3) |
| GET | `/api/receipt/head` | **Live** audit-chain head |
| WS | `/api/chat/ws` | Chat WebSocket — token + tool-event stream |

## Frontend pages

Preserved from Hermes:

- **StatusPage** — host info, version, recent sessions.
- **ConfigPage** — TOML schema-driven config editor.
- **EnvPage** — environment + secret inspector.

Added in GaussClaw (lands in follow-on phases):

- **ReceiptPage** — chain head, verify upload, TSA proofs.
- **LineagePage** — interactive conversation graph.
- **SandboxPage** — per-tool layer status, fuel/epoch counters.
- **ProvidersPage** — catalogue, polyhedral-equivalence badges,
  OpenRouter / NotDiamond aggregator views, cost telemetry.
- **ExportPage** — envelope viewer, taint-filter mode toggle,
  federated-pool publisher.
