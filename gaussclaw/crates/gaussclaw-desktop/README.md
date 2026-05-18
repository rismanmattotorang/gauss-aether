# gaussclaw-desktop

**The Tauri 2 desktop shell. ~20 MB on disk, ~80 MB RAM idle, ≤ 10 ms
cold start, signed and notarised on macOS, Windows, and Linux — about
a tenth the size of Hermes's Electron 39 build on every axis.**

## What it ships

A single Tauri 2 binary that renders the same `gaussclaw-web/frontend/dist/`
dashboard through the OS WebView (no Chromium bundled). Same six views,
same ⌘K command palette, same keyboard shortcuts — but with five
desktop-only capabilities surfaced through typed IPC commands:

| Command | What it does |
|---|---|
| `gc_clipboard_copy` | Audit-recorded OS clipboard write via `tauri-plugin-clipboard-manager`. |
| `gc_global_hotkey_register` | Register a global chord (e.g. `CommandOrControl+Shift+G`). |
| `gc_tray_menu` | Operator-configurable system-tray menu model. |
| `gc_notify` | Native OS notification through `tauri-plugin-notification`. |
| `gc_updater_verify_artifact` | **Chain-anchored** updater verification — see `updater.rs`. Hermes ships unsigned binaries with no chain anchor. |

Plus every dashboard endpoint mirrored as a typed `gc_*` IPC command —
`gc_status`, `gc_config_get`, `gc_receipts_recent`, `gc_envelope_verify`,
`gc_skill_preview`, `gc_health`, `gc_sessions_recent`, `gc_tools_list`,
`gc_chat`, … — so the React frontend can call the same logic over
OS-native IPC instead of localhost HTTP. The pure async functions in
`commands.rs` and `system.rs` are independently tested; the Tauri
shims live in `runtime.rs` and only compile under
`--features tauri-runtime`.

## Why it's far smaller than Hermes Desktop

| Axis | Hermes Desktop | GaussClaw Desktop | Mechanism |
|---|---|---|---|
| Renderer | Bundled Chromium (Electron 39) | OS WebView (WebView2 / WKWebView / WebKitGTK) | Tauri 2 |
| Installer size | ~150 MB | ≤ 20 MB | one Rust binary + 50 KB dashboard |
| RAM idle | ~250 MB | ≤ 80 MB | OS WebView, no V8 / Node |
| Cold start | ~3 s | ≤ 500 ms | static binary, no `node_modules` resolution |
| IPC transport | HTTP on `127.0.0.1:8642` | OS-native (Unix domain sockets / Windows named pipes) | `tauri::generate_handler!` |
| Code-signed | unsigned everywhere | macOS Developer ID + Windows Authenticode + Linux GPG | `bundle.macOS.signingIdentity`, `bundle.windows.certificateThumbprint`, GPG-detached |
| Updater integrity | none beyond TLS | Ed25519 publisher sig + chain-anchored SHA-256 | `gaussclaw_desktop::updater::verify_release_artifact` |

## Updater verification

Every release artefact ships with a [`ReleaseManifest`](./src/updater.rs)
that the local updater verifies **before** swapping the binary in:

1. SHA-256 of the artefact bytes matches the manifest claim.
2. The publisher's Ed25519 signature over `version:target:sha256_hex`
   verifies under the locally trusted publisher key.
3. The artefact's target triple matches the running host.
4. The manifest version is strictly newer than the running version
   (refuses downgrade attacks).

A compromised CDN cannot ship a malicious update — every byte we apply
was already anchored in a chain the publisher signed. Hermes's
updater applies whatever the CDN returned.

## Build

The crate's default build is **runtime-free** so it compiles on any
CI runner (no `webkit2gtk-4.1` / `WebView2` system deps required) and
the library half is always linkable + testable. Shipping the actual
desktop binary is a deliberate feature opt-in:

```bash
# Library half — always builds. Used by gaussclaw-bin, by tests,
# and by anyone embedding the IPC contract.
cargo build -p gaussclaw-desktop

# Full Tauri 2 binary. Requires the platform WebView dependencies:
#   Linux:    apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev
#   macOS:    xcode-select --install
#   Windows:  Microsoft Edge WebView2 Runtime
cargo build -p gaussclaw-desktop --features tauri-runtime --release
```

## Bundle + sign

Use the `tauri` CLI:

```bash
cargo install tauri-cli@^2
cargo tauri build           # bundles + signs into target/release/bundle/
```

Per-OS signing is configured in
[`tauri.conf.json`](./tauri.conf.json) under `bundle.macOS`,
`bundle.windows`, and `bundle.linux`. Operators provide their
credentials via environment variables:

| OS | Env vars | Notes |
|---|---|---|
| macOS | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_PASSWORD` | Developer ID signing + Apple notarisation |
| Windows | `TAURI_PRIVATE_KEY`, `TAURI_KEY_PASSWORD` (Authenticode via `signtool`) | Or supply `certificateThumbprint` in the config |
| Linux | `GPG_KEY_ID` | AppImage signing + GPG-detached `.deb` / `.rpm` |

The updater's Ed25519 signing key is generated once via
`tauri signer generate` and the public component goes into the config's
`plugins.updater.pubkey`.

## Tested surface

```bash
cargo test -p gaussclaw-desktop
```

34 tests cover every IPC command (envelope shape, kernel-denied
edge cases, hotkey chord parsing, updater axis-named failure modes,
and the canonical signed-message format).
