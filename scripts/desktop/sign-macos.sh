#!/usr/bin/env bash
# Sign + notarise + staple macOS bundle artefacts.
#
# In the normal release flow, Tauri's bundler invokes `codesign` and
# `xcrun notarytool` itself when the APPLE_* env vars are present —
# the GitHub workflow sets them, and Tauri does the right thing
# unattended. This script exists as a fallback / local-dev tool for
# operators who want to sign a previously-built bundle by hand.
#
# Usage:
#   APPLE_SIGNING_IDENTITY="Developer ID Application: Acme (ABCD1234)" \
#   APPLE_ID="me@example.com" \
#   APPLE_PASSWORD="app-specific-password" \
#   APPLE_TEAM_ID="ABCD1234" \
#   ./scripts/desktop/sign-macos.sh target/release/bundle
#
# Required environment:
#   APPLE_SIGNING_IDENTITY    Developer ID Application identity string.
#   APPLE_ID                  Apple ID for notarisation.
#   APPLE_PASSWORD            App-specific password (not your iCloud password).
#   APPLE_TEAM_ID             Apple team id (e.g. ABCD1234).
#
# Behaviour:
#   - codesign each .dmg / .app / inner Mach-O with --options runtime
#     + --timestamp, using the Developer ID identity.
#   - submit each signed .dmg to notarytool, wait for the verdict.
#   - staple the notarisation ticket on success.

set -euo pipefail

BUNDLE_DIR="${1:-target/release/bundle}"
if [[ ! -d "$BUNDLE_DIR" ]]; then
  echo "error: bundle dir does not exist: $BUNDLE_DIR" >&2
  exit 2
fi

for var in APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [[ -z "${!var:-}" ]]; then
    echo "error: $var is required" >&2
    exit 1
  fi
done

shopt -s globstar nullglob
count=0

for app in "$BUNDLE_DIR"/**/*.app; do
  [[ -d "$app" ]] || continue
  echo "::group::codesign $app"
  /usr/bin/codesign \
    --force \
    --deep \
    --timestamp \
    --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" \
    "$app"
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$app"
  echo "::endgroup::"
  count=$((count + 1))
done

for dmg in "$BUNDLE_DIR"/**/*.dmg; do
  [[ -f "$dmg" ]] || continue

  echo "::group::codesign $dmg"
  /usr/bin/codesign \
    --force \
    --timestamp \
    --options runtime \
    --sign "$APPLE_SIGNING_IDENTITY" \
    "$dmg"
  echo "::endgroup::"

  echo "::group::notarytool submit $dmg"
  xcrun notarytool submit "$dmg" \
    --apple-id    "$APPLE_ID" \
    --password    "$APPLE_PASSWORD" \
    --team-id     "$APPLE_TEAM_ID" \
    --wait
  echo "::endgroup::"

  echo "::group::stapler staple $dmg"
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  echo "::endgroup::"

  count=$((count + 1))
done

echo "signed + notarised $count macOS artefact(s)"
if [[ "$count" -eq 0 ]]; then
  echo "warning: no .dmg / .app artefacts found under $BUNDLE_DIR" >&2
fi
