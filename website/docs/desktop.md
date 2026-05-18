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

## IPC command surface

Eighteen typed `gc_*` IPC commands live in
[`gaussclaw-desktop`](https://github.com/rismanmattotorang/gauss-aether/tree/main/gaussclaw/crates/gaussclaw-desktop).
The frontend invokes them as `invoke('gc_status')`, `invoke('gc_envelope_verify', {...})`,
etc. Every command is a pure async function over `ServerState`; the
`tauri::generate_handler!` wiring lives in `src/runtime.rs` behind the
`tauri-runtime` feature.

| Category | Commands |
|---|---|
| **Status & config** | `gc_status`, `gc_config_get`, `gc_config_set` |
| **Audit & caps** | `gc_receipt_head`, `gc_receipts_recent`, `gc_caps` |
| **Dashboard mirrors** | `gc_health`, `gc_sessions_recent`, `gc_tools_list`, `gc_envelope_verify`, `gc_skill_preview` |
| **Chat** | `gc_chat` |
| **Desktop-only** | `gc_clipboard_copy`, `gc_global_hotkey_register`, `gc_tray_menu`, `gc_notify`, `gc_updater_verify_artifact` |

## Build

The crate's default build is runtime-free so the library half always
compiles + tests on any CI runner. The full desktop binary is a
deliberate feature opt-in:

```bash
# Library half — always builds (no webkit2gtk / WebView2 required).
cargo build -p gaussclaw-desktop

# Full Tauri 2 binary. Requires the platform WebView dependencies:
#   Linux:    apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev
#   macOS:    xcode-select --install
#   Windows:  Microsoft Edge WebView2 Runtime (ships with Win 11)
cargo install tauri-cli@^2
cargo tauri build              # bundles into target/release/bundle/
```

## Distribution

| Platform | Targets | Signing |
|---|---|---|
| macOS | universal `.dmg` (aarch64 + x86_64), `.app` | Apple Developer ID + notarization + stapling |
| Windows | `.msi` (WiX), `.exe` (NSIS), winget manifest | Authenticode (EV cert preferred) |
| Linux | `.AppImage`, `.deb`, `.rpm`, Flatpak | GPG; Flathub manifest |
| iOS (v2) | TestFlight `.ipa` via Tauri 2 mobile | App Store |
| Android (v2) | `.apk`, `.aab` via Tauri 2 mobile | Play Console / F-Droid |

## Updater integrity (Hermes ships none of this)

Every release artefact ships with a `ReleaseManifest` carrying:

- the artefact's **SHA-256** (hex);
- the publisher's **Ed25519 signature** over `version:target:sha256_hex`;
- the **chain index** the publisher anchored the release at;
- the **target triple** (`x86_64-apple-darwin`, etc.).

The local updater calls
[`verify_release_artifact`](https://github.com/rismanmattotorang/gauss-aether/blob/main/gaussclaw/crates/gaussclaw-desktop/src/updater.rs)
**before** swapping the binary, checking:

1. Computed SHA-256 of the downloaded bytes matches the manifest claim.
2. The publisher's Ed25519 signature verifies under the locally
   trusted publisher key.
3. The artefact's target triple matches the running host (no
   cross-target swaps).
4. The manifest version is strictly **newer** than the running
   version (refuses downgrade attacks).

A compromised CDN cannot ship a malicious update. Hermes's updater
performs none of these checks — its installers are unsigned and its
update flow trusts whatever the CDN returns.

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
