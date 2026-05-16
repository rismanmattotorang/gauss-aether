# ADR-0016 — TEE attestation matrix for 1.0

**Status:** Accepted (Phase 10)
**Date:** 2026-05-16
**Proves:** Theorem T10 Layer 4 (TEE attestation; software-only Phase 10)

## Context

Theorem T10 (paper §IX) bounds compromise probability by the product of
the per-layer bounds plus the TEE term `p_T`. Phases 3–7 shipped the
four software layers (WASM, Landlock/Seatbelt, namespaces/seccomp,
HWCA worker isolation). Phase 10 adds the L4 attestation layer.

Three hardware backends are in scope at 1.0:

* **AMD SEV-SNP** (`SevSnp`).
* **Intel TDX** (`TdxIntel`).
* **ARM Confidential Compute Architecture** (`ArmCca`).

Production hardware backends require attestation-service round trips and
specific kernel modules — they ship as additive plugin crates that
implement the `Attestor` trait. Phase 10's `gauss-attest` crate ships
the **wire format + verifier** that all three plus a software simulator
share.

## Decision

### 1. Trait + canonical wire format

`Attestor::attest(claims, nonce) -> AttestationReport`. The report's
canonical pre-image is:

```text
kind (1) ‖ measurement (32) ‖ nonce (32) ‖ generated_at_ms (8 LE) ‖
workload_len (4 LE) ‖ workload (utf8) ‖
version_len (4 LE) ‖ version (utf8) ‖
node_present (1) ‖ node_len (4 LE) ‖ node (utf8)?
```

`Ed25519.sign(sk, pre)` is the attestor's signature; verifiers
reconstruct the pre-image and check the signature against the
trusted-keys set.

### 2. Software simulator is the Phase-10 ship

`SoftwareSimAttestor` is an Ed25519-backed simulator that runs in the
conformance suite and on developer laptops. Verifiers refuse the
`Simulator` kind unless explicitly configured to trust the simulator
public key. Production deployments wire one of the three hardware
backends; the trait surface is identical.

### 3. Verification API: replay defence + measurement pinning + key set

`verify_report(report, expected_nonce, trusted_keys, trusted_baseline)`:

* Nonce mismatch → `AttestError::NonceMismatch` (replay defence).
* Measurement mismatch → `AttestError::MeasurementMismatch` (workload
  swap detection).
* Public key not in trust set → `AttestError::UntrustedKind`.
* Signature invalid → `AttestError::SignatureInvalid`.

The trust set is a `&[[u8; 32]]`; passing an empty slice skips the key
check (only safe when the verifier pinned the key out-of-band).

### 4. Measurement is SHA-256 over the workload bytes

`measure_workload(bytes) -> [u8; 32]` — pure-Rust, deterministic.
Production backends substitute their hardware register snapshot.

## Consequences

- **Pro:** Pure-Rust crypto stack (ed25519-dalek, sha2) — no OpenSSL
  / TPM PKCS#11 plumbing required for the conformance suite.
- **Pro:** Wire format is layout-stable + documented; cross-language
  verifiers reconstruct the pre-image bit-for-bit.
- **Pro:** Verifier short-circuits on the first failure with a
  diagnosable error variant.
- **Con:** Production hardware backends are NOT in `gauss-attest`
  itself — they ship as additive plugin crates that take the same
  trait + helper functions.
- **Con:** Attestation freshness is bounded by the verifier's nonce
  cadence; deployments need to cycle nonces ≥ N times per session.

## Hardware-backend plugin migration

```text
gauss-attest-sevsnp/   ← Phase-10 plugin crate, wraps amd-sev-snp-rs
gauss-attest-tdx/      ← Phase-10 plugin crate, wraps Intel TDX guest tools
gauss-attest-armcca/   ← Phase-10 plugin crate, wraps ARM CCA Realm
```

Each:

1. Implements `Attestor` over the platform's attestation service.
2. Returns reports with the same canonical pre-image.
3. Provides a `trusted_keys` set the operator pins out-of-band.

The conformance suite swaps the simulator for the hardware backend via
a `use` change — no engine surface modification.
