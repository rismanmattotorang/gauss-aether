---
id: installation
title: Installation
sidebar_position: 1
---

# Installation

GaussClaw is a single static Rust binary that bundles the CLI, the TUI,
the web dashboard backend, and the embedded React frontend. The desktop
app is a separate Tauri 2 binary that ships through normal OS installers.

## From source (current path)

```bash
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether
cargo build --release -p gaussclaw-bin
./target/release/gaussclaw --help
```

Requires Rust 1.88 or newer.

## Pre-built binaries (post-GA)

| Platform | Format | Notes |
|---|---|---|
| macOS | `.dmg` (universal: aarch64 + x86_64) | Apple Developer ID signed + notarized |
| Windows | `.msi` (WiX) and `.exe` (NSIS) | Authenticode signed |
| Linux | `.AppImage`, `.deb`, `.rpm`, Flatpak | GPG signed; Flathub manifest accepted |
| iOS *(v2)* | TestFlight `.ipa` | Tauri 2 mobile |
| Android *(v2)* | `.apk`, `.aab` | Tauri 2 mobile |

All installers are **Ed25519-signed**; every release binary's SHA-256
is anchored in the public receipt chain via `gauss-attest`. The Tauri
updater verifies both the certificate chain and the chain inclusion
before applying any update.

## Verify the install

```bash
gaussclaw doctor
```

Runs the seven Self-Diagnostic Health Engine invariants from
`gauss-health`. A green report means kernel, memory, sandbox, audit
chain, and surfaces are all coherent.
