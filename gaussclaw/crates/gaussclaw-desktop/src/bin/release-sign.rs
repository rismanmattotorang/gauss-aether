//! `gaussclaw-release-sign` — produce a chain-verifiable
//! [`gaussclaw_desktop::updater::ReleaseManifest`] for a built
//! installer.
//!
//! This binary is what the release CI runs after each per-OS bundle
//! step. It reads the installer bytes, computes the SHA-256, signs the
//! canonical message `version:target:sha256_hex` with the publisher's
//! Ed25519 secret key, and emits a JSON manifest the desktop updater
//! verifies before swap-in.
//!
//! ## Inputs
//!
//! - `--version` — SemVer of the release (`"1.0.3"`).
//! - `--target` — Rust-style target triple (`"x86_64-apple-darwin"`).
//! - `--artefact` — path to the signed installer bytes.
//! - `--signing-key-base64` — 32-byte Ed25519 *secret* key, base64.
//! - `--chain-index` *(optional)* — chain index the publisher anchored
//!   this release at. Defaults to `0`.
//!
//! ## Output
//!
//! A `ReleaseManifest` printed to stdout as compact JSON. The CI uploads
//! it alongside the artefact under `releases.gauss.ai/desktop/<target>/
//! <arch>/<version>/manifest.json`.
//!
//! ## Operational discipline
//!
//! - The signing key never leaves the GitHub Actions runner. The CI job
//!   reads it from `${{ secrets.RELEASE_ED25519_SK_BASE64 }}` and pipes
//!   it directly into this binary's `--signing-key-base64` arg.
//! - The corresponding **public** key is baked into the desktop binary
//!   at build time as the trust anchor for `verify_release_artifact`.
//!   Rotating the publisher key is therefore a deliberate release.

use std::path::PathBuf;
use std::process::ExitCode;

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use gaussclaw_desktop::updater::{canonical_signed_message, ReleaseManifest};
use sha2::{Digest, Sha256};

struct Args {
    version: String,
    target: String,
    artefact: PathBuf,
    signing_key_base64: String,
    chain_index: u64,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            eprintln!();
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let body = match std::fs::read(&args.artefact) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read --artefact {}: {e}", args.artefact.display());
            return ExitCode::from(1);
        }
    };
    let sha256_hex = hex_lower(&Sha256::digest(&body));

    let sk_bytes = match base64::engine::general_purpose::STANDARD.decode(&args.signing_key_base64)
    {
        Ok(b) => b,
        Err(e) => {
            eprintln!("--signing-key-base64 is not valid base64: {e}");
            return ExitCode::from(1);
        }
    };
    let sk_arr: [u8; 32] = match sk_bytes.as_slice().try_into() {
        Ok(a) => a,
        Err(_) => {
            eprintln!(
                "--signing-key-base64 decoded to {} bytes, want 32",
                sk_bytes.len()
            );
            return ExitCode::from(1);
        }
    };
    let sk = SigningKey::from_bytes(&sk_arr);

    let message = canonical_signed_message(&args.version, &args.target, &sha256_hex);
    let sig = sk.sign(message.as_bytes());

    let manifest = ReleaseManifest::new(
        args.version,
        args.target,
        sha256_hex,
        hex_lower(&sig.to_bytes()),
        args.chain_index,
    );
    match serde_json::to_string(&manifest) {
        Ok(s) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("failed to serialise manifest: {e}");
            ExitCode::from(1)
        }
    }
}

const USAGE: &str = "\
gaussclaw-release-sign

Usage:
  gaussclaw-release-sign \\
    --version <SEMVER> \\
    --target <TRIPLE> \\
    --artefact <PATH> \\
    --signing-key-base64 <BASE64> \\
    [--chain-index <N>]

Emits a ReleaseManifest JSON on stdout that the desktop updater
verifies via gaussclaw_desktop::updater::verify_release_artifact.
";

fn parse_args() -> Result<Args, String> {
    let mut version = None;
    let mut target = None;
    let mut artefact = None;
    let mut signing_key = None;
    let mut chain_index: u64 = 0;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--version" => version = it.next(),
            "--target" => target = it.next(),
            "--artefact" => artefact = it.next().map(PathBuf::from),
            "--signing-key-base64" => signing_key = it.next(),
            "--chain-index" => {
                chain_index = it
                    .next()
                    .ok_or_else(|| "--chain-index needs a value".to_string())?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| format!("--chain-index: {e}"))?;
            }
            "-h" | "--help" => return Err("usage requested".into()),
            other => return Err(format!("unknown flag `{other}`")),
        }
    }
    Ok(Args {
        version: version.ok_or("--version is required")?,
        target: target.ok_or("--target is required")?,
        artefact: artefact.ok_or("--artefact is required")?,
        signing_key_base64: signing_key.ok_or("--signing-key-base64 is required")?,
        chain_index,
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
