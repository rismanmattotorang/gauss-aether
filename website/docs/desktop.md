---
id: desktop
title: Desktop app
sidebar_position: 7
---

# Desktop app

Built on **[Tauri 2](https://tauri.app)** — a Rust shell rendering
through the OS WebView (WebView2 on Windows, WKWebView on macOS,
WebKitGTK on Linux). No Chromium bundled.

The desktop binary supersedes the upstream **Hermes Desktop** (Electron
39 + Python backend over HTTP on `127.0.0.1:8642`) on every measurable
axis:

| metric | Hermes Desktop | GaussClaw Desktop |
|---|---|---|
| installer size | ~150 MB | **≤ 20 MB** |
| on-disk size | ~300 MB | **≤ 50 MB** |
| RAM idle | ~250 MB | **≤ 80 MB** |
| cold start (warm cache) | ~3 s | **≤ 500 ms** |
| code-signed | no | **yes (3 OSes)** |
| notarized (macOS) | no | **yes** |
| auto-update integrity | TLS-only | **Ed25519-signed + chained receipt** |
| IPC channel | HTTP on `127.0.0.1:8642` | **OS-native IPC (no socket)** |
| mobile path | none | **Tauri 2 mobile (v2)** |

## Architecture

The desktop binary holds the same `ServerState` as the `gaussclaw web`
HTTP backend, so the IPC commands and the HTTP routes share one source
of truth. The frontend talks to the binary through typed
`#[tauri::command]` invocations over the platform IPC channel
(Unix domain sockets / Windows named pipes) — there is no localhost
HTTP socket, no PTY.

## Capability alignment

Tauri 2's permission system maps directly onto the Gauss-Aether
capability lattice 𝒦. The build pipeline emits
`tauri.conf.json` capabilities from the same Skill Manifests the
kernel reads at admission time. A tool's `caps = ["fs:read:./data"]`
produces the matching scoped FS permission as a build-time artefact.
Front-door and tool-dispatch capabilities are one artefact — no
policy drift.

## Build

```bash
cd crates/gaussclaw-web/frontend && pnpm install && pnpm build
cd ../..
cargo install tauri-cli@^2
cargo tauri build --features tauri-runtime
```

On Linux, install `webkit2gtk-4.1` development headers first. On
macOS, Xcode CLT. On Windows, WebView2 ships with recent Windows 11.

## Distribution

| Platform | Targets | Signing |
|---|---|---|
| macOS | universal `.dmg` (aarch64 + x86_64), `.app` | Apple Developer ID + notarization + stapling |
| Windows | `.msi` (WiX), `.exe` (NSIS), winget manifest | Authenticode (EV cert preferred) |
| Linux | `.AppImage`, `.deb`, `.rpm`, Flatpak | GPG; Flathub manifest |
| iOS (v2) | TestFlight `.ipa` via Tauri 2 mobile | App Store |
| Android (v2) | `.apk`, `.aab` via Tauri 2 mobile | Play Console / F-Droid |

## Native features

| feature | notes |
|---|---|
| System tray | Quick-toggle for gateway, recent sessions, capability hold/release |
| Global hotkey | Default `Cmd/Ctrl+Shift+H` — Spotlight-style overlay |
| Native notifications | Tool-approval prompts, deadline elapses, channel arrivals |
| Single-instance lock | One process per OS user (Electron does not guarantee) |
| Deep links | `gaussclaw://session/{id}`, `gaussclaw://skill/install/{url}` |
| Autostart at login | Opt-in via `[desktop] autostart = true` |
| Multi-window | Skills inspector, Office 3D view, Memory editor, Sandbox monitor |
| Drag-and-drop files | Routed through scoped FS plugin; tainted `user` by default |
