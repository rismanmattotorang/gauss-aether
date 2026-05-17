---
id: installation
title: Installation
sidebar_position: 1
---

# Installation

GaussClaw is a single static Rust binary that bundles the CLI, the
TUI, the web dashboard backend, and the embedded React frontend. The
desktop app is a separate Tauri 2 binary that ships through normal OS
installers.

Pick the path that matches your platform.

## Pre-built installers

The fastest path. Download from the
[Releases page](https://github.com/rismanmattotorang/gauss-aether/releases).

| Platform | Format | Notes |
|---|---|---|
| **macOS** | `.dmg` (universal: aarch64 + x86_64) | Apple Developer ID signed and notarised. |
| **Windows** | `.msi` (WiX) and `.exe` (NSIS) | Authenticode signed. |
| **Linux** | `.AppImage`, `.deb`, `.rpm`, Flatpak | GPG signed; Flathub manifest accepted. |
| **iOS** *(coming)* | TestFlight `.ipa` | Tauri 2 Mobile. |
| **Android** *(coming)* | `.apk`, `.aab` | Tauri 2 Mobile. |

Every installer is **Ed25519-signed**, and every release binary's
SHA-256 is anchored in the public receipt chain via `gauss-attest`.
The Tauri updater verifies both the certificate chain and the chain
inclusion before applying any update — so a compromised CDN cannot
ship you a malicious binary.

## From source

For developers, contributors, and anyone who wants the latest commit.

```bash
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether

# Build the shipping binary
cargo build --release -p gaussclaw-bin
./target/release/gaussclaw --help

# Or install it on your PATH
cargo install --path gaussclaw/crates/gaussclaw-bin
```

Requires **Rust 1.83 or newer**. The workspace builds out of the box on
Linux, macOS, and Windows.

## Optional: build the desktop app

```bash
cd gaussclaw/crates/gaussclaw-desktop
cargo tauri build
```

Produces a signed bundle in `target/release/bundle/` if you have the
relevant signing identities configured. See the Tauri
[signing guide](https://tauri.app/v2/distribute/sign-macos/) for the
keychain steps.

## Verify the install

```bash
gaussclaw doctor
```

Runs the seven Self-Diagnostic Health Engine invariants from
`gauss-health`. A green report means the kernel, the memory store,
the sandbox, the audit chain, and the surfaces are all coherent and
ready.

## Update later

```bash
gaussclaw update
```

The Tauri updater fetches the latest release, verifies the signature
*and* the receipt-chain anchor, then applies the patch. There is no
silent auto-update; you always run the command yourself.
