#!/usr/bin/env bash
# Sign Linux bundle artefacts with GPG (and the AppImage built-in
# signing scheme).
#
# Usage:
#   GPG_PRIVATE_KEY="$(cat key.asc)" \
#   GPG_PASSPHRASE="..." \
#   ./scripts/desktop/sign-linux.sh target/release/bundle
#
# Required environment:
#   GPG_PRIVATE_KEY  ASCII-armoured GPG private key.
#   GPG_PASSPHRASE   Passphrase for the private key (may be empty).
#
# Behaviour:
#   - .AppImage : embedded signature via `gpg --detach-sign` next to
#                 the file (`.AppImage.asc`).
#   - .deb       : `gpg --detach-sign --armor` next to the file
#                 (`.deb.asc`); apt-get verifies via the repo's
#                 InRelease file.
#   - .rpm       : `rpm --addsign` if `rpmsign` is available; falls
#                 back to a detached signature.
#
# Exit codes:
#   0  every artefact for which signing was attempted signed cleanly.
#   1  GPG import or signing failed.
#   2  the bundle dir was empty.

set -euo pipefail

BUNDLE_DIR="${1:-target/release/bundle}"
if [[ ! -d "$BUNDLE_DIR" ]]; then
  echo "error: bundle dir does not exist: $BUNDLE_DIR" >&2
  exit 2
fi

if [[ -z "${GPG_PRIVATE_KEY:-}" ]]; then
  echo "GPG_PRIVATE_KEY is empty; refusing to sign." >&2
  exit 1
fi

# Import the key into the runner's keyring.
keyring="$(mktemp -d)/keyring"
mkdir -p "$keyring"
chmod 700 "$keyring"
export GNUPGHOME="$keyring"

echo "::group::Importing GPG private key"
printf '%s' "${GPG_PRIVATE_KEY}" | gpg --batch --pinentry-mode loopback --passphrase "${GPG_PASSPHRASE:-}" --import
gpg --list-secret-keys --keyid-format=long
echo "::endgroup::"

KEY_ID="$(gpg --list-secret-keys --keyid-format=long | awk '/^sec/ {split($2, a, "/"); print a[2]; exit}')"
if [[ -z "$KEY_ID" ]]; then
  echo "error: no secret key after import" >&2
  exit 1
fi
echo "Using key: $KEY_ID"

count=0
shopt -s globstar nullglob

for f in "$BUNDLE_DIR"/**/*.AppImage "$BUNDLE_DIR"/**/*.deb; do
  [[ -e "$f" ]] || continue
  echo "Signing $f"
  gpg --batch --yes --pinentry-mode loopback \
      --passphrase "${GPG_PASSPHRASE:-}" \
      --local-user "$KEY_ID" \
      --armor --detach-sign \
      --output "$f.asc" \
      "$f"
  count=$((count + 1))
done

for f in "$BUNDLE_DIR"/**/*.rpm; do
  [[ -e "$f" ]] || continue
  if command -v rpmsign >/dev/null 2>&1; then
    echo "rpmsign $f"
    rpmsign --addsign --define "_gpg_name $KEY_ID" "$f"
  else
    echo "rpmsign not available; falling back to detached signature for $f"
    gpg --batch --yes --pinentry-mode loopback \
        --passphrase "${GPG_PASSPHRASE:-}" \
        --local-user "$KEY_ID" \
        --armor --detach-sign \
        --output "$f.asc" \
        "$f"
  fi
  count=$((count + 1))
done

echo "Signed $count Linux artefact(s)."
if [[ "$count" -eq 0 ]]; then
  echo "warning: no artefacts found to sign" >&2
fi
