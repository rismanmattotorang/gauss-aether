# Desktop release pipeline

Helper scripts + CI workflow that produce **signed, chain-anchored**
installers of `gaussclaw-desktop` for macOS, Windows, and Linux.

```text
.github/workflows/desktop-release.yml    ← matrix-builds + drives these scripts
scripts/desktop/
├── README.md                            ← this file
├── sign-linux.sh                        ← GPG-detached + AppImage signing
├── sign-macos.sh                        ← codesign + notarytool + stapler
└── sign-windows.ps1                     ← Authenticode via signtool

gaussclaw/crates/gaussclaw-desktop/src/bin/release-sign.rs
                                         ← chain-anchored manifest signer
```

## Single-shot release workflow

When you push a tag `vX.Y.Z`, the
[`desktop-release`](../../.github/workflows/desktop-release.yml)
workflow does the entire dance unattended:

1. Build the Tauri 2 bundle on each platform.
2. Sign each platform's artefact with the operator-supplied certificate
   material from GitHub Secrets.
3. Run `gaussclaw-release-sign` against every artefact to produce a
   chain-anchored `*.manifest.json` next to it.
4. Publish a GitHub Release with every artefact + manifest attached.

The desktop binary's built-in updater
([`gaussclaw_desktop::updater::verify_release_artifact`](../../gaussclaw/crates/gaussclaw-desktop/src/updater.rs))
then checks four invariants before applying any update:

1. SHA-256 of the downloaded bytes matches the manifest claim.
2. The publisher's Ed25519 signature over
   `version:target:sha256_hex` verifies under the locally-trusted
   publisher key.
3. The artefact's target triple matches the running host.
4. The manifest version is strictly newer than the running version
   (refuses downgrade attacks).

A compromised CDN cannot ship a malicious update. **Hermes ships
unsigned binaries with no chain anchor — its updater verifies nothing.**

## Required GitHub Secrets

Set these under repo **Settings → Secrets and variables → Actions**.
Missing secrets cause the corresponding signing step to be skipped (the
build still produces an unsigned bundle for local testing).

### Chain-anchored manifest

| Secret | What |
|---|---|
| `RELEASE_ED25519_SK_BASE64` | 32-byte Ed25519 *secret* key, base64. Generate once with `openssl rand -base64 32`. The public-key half is baked into the desktop binary at build time. |

### macOS

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | Developer ID Application `.p12`, base64-encoded. |
| `APPLE_CERTIFICATE_PASSWORD` | The `.p12` password. |
| `APPLE_SIGNING_IDENTITY` | `"Developer ID Application: Acme Corp (ABCD1234)"` |
| `APPLE_ID` | Apple ID for notarisation. |
| `APPLE_PASSWORD` | App-specific password (generate at appleid.apple.com → Security). |
| `APPLE_TEAM_ID` | Apple team id (e.g. `ABCD1234`). |

Tauri's bundler invokes `codesign` and `xcrun notarytool` itself when
these env vars are present — no separate script call needed.

### Windows

| Secret | What |
|---|---|
| `WINDOWS_CERTIFICATE` | Authenticode `.pfx`, base64-encoded. |
| `WINDOWS_CERTIFICATE_PASSWORD` | The `.pfx` password. |

The workflow runs `scripts/desktop/sign-windows.ps1` after the Tauri
build to invoke `signtool` with SHA-256 + RFC 3161 timestamping
(DigiCert by default; override via the script's `-TimestampUrl` arg).

### Linux

| Secret | What |
|---|---|
| `GPG_PRIVATE_KEY` | ASCII-armoured GPG private key. |
| `GPG_PASSPHRASE` | Passphrase for the key (may be empty). |

The workflow runs `scripts/desktop/sign-linux.sh` after the Tauri
build to produce detached `.asc` signatures next to each `.AppImage` /
`.deb` / `.rpm`, plus an embedded `rpmsign --addsign` when `rpmsign`
is available on the runner.

## Manual local signing

Each script is independently runnable for local-developer flows.

```bash
# Linux
GPG_PRIVATE_KEY="$(cat ~/key.asc)" GPG_PASSPHRASE='' \
  ./scripts/desktop/sign-linux.sh target/release/bundle

# macOS
APPLE_SIGNING_IDENTITY="Developer ID Application: ..." \
APPLE_ID="me@example.com" APPLE_PASSWORD='app-specific-pw' \
APPLE_TEAM_ID='ABCD1234' \
  ./scripts/desktop/sign-macos.sh target/release/bundle

# Windows
$env:WINDOWS_CERTIFICATE = (Get-Content key.pfx.b64)
$env:WINDOWS_CERTIFICATE_PASSWORD = '...'
pwsh ./scripts/desktop/sign-windows.ps1 -BundleDir target/release/bundle
```

Generate a chain-anchored manifest for any signed installer:

```bash
cargo run -p gaussclaw-desktop --bin gaussclaw-release-sign --release -- \
  --version 1.0.0 \
  --target x86_64-apple-darwin \
  --artefact target/release/bundle/dmg/GaussClaw_1.0.0_x64.dmg \
  --signing-key-base64 "$RELEASE_ED25519_SK_BASE64" \
  --chain-index "$(date +%s)" \
  > GaussClaw_1.0.0_x64.dmg.manifest.json
```

## Verifying a downloaded release yourself

The same trust path the in-app updater uses, exposed as a one-shot
check:

```bash
# Compute SHA-256 locally.
shasum -a 256 GaussClaw_1.0.0_x64.dmg
# → e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  …

# Inspect the manifest.
cat GaussClaw_1.0.0_x64.dmg.manifest.json | jq
# → { "version": "1.0.0", "target": "x86_64-apple-darwin",
#     "sha256_hex": "e3b0c44...", "publisher_signature_hex": "...",
#     "chain_index": 7 }

# The two sha256 strings must match. The publisher signature can be
# verified offline with any ed25519 verifier against the public key
# baked into the desktop binary.
```

## Provenance contract

The desktop binary, the release CLI, the CI workflow, and the
manifest format are all in this repository under MIT. The
**operator-specific cert material is never checked in**; it lives
exclusively in GitHub Secrets and the GitHub Actions runner's
ephemeral filesystem.
