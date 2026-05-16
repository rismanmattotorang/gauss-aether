# ADR-0011 — Receipt-chain signing, TSA anchors, and offline conformance

**Status:** Accepted (Phase 5)
**Date:** 2026-05-16
**Locks:** Axiom A9 (EUF-CMA receipts + chain anchor)
**Proves:** Theorem T11 (receipt non-repudiation)

## Context

Phase 2 shipped an un-signed SHA-256 chain `c_i = H(c_{i-1} ‖ ρ_i)`. Phase 5
adds:

1. **Per-record signatures** so a third party can verify, off-chain and
   off-line, that a specific append happened under a specific identity.
2. **External anchors** (RFC 3161 / `OpenTimestamps`) so the chain has a
   trust-rooted timestamp that survives operator key compromise.
3. **A public verifier API** — same Rust surface for in-process verifiers
   and the Phase-9 HTTP wrapper.

Three implementation choices needed locking:

1. **Curve choice** — Ed25519 vs ECDSA-P256 vs BLS. SPECS §IX names Ed25519
   for performance and ecosystem reasons; ADR-0003 already pinned the curve
   for the workspace.
2. **Backend pluggability.** The conformance suite must run with an
   in-process key; production must run with an OS-keyring / HSM / cloud KMS.
3. **Anchor authority shape.** RFC 3161 needs DER + HTTP; `OpenTimestamps`
   needs Bitcoin Calendar HTTP; neither is offline-friendly for tests.

## Decision

### 1. Ed25519 with a pluggable `SigningBackend` trait

Phase 5 ships [`gauss_audit::sign::Ed25519Signer`] (in-process, dalek 2.x)
plus the [`SigningBackend`] trait:

```rust
pub trait SigningBackend: Send + Sync {
    fn public_key(&self) -> [u8; 32];
    fn sign(&self, message: &[u8]) -> GaussResult<[u8; 64]>;
}
```

Production deployments wire a custom impl backed by:

* `keyring` crate → OS keyring (Phase 5 follow-up).
* AWS KMS / GCP KMS / Azure Key Vault — async wrapped through a
  `block_in_place` shim.
* PKCS #11 HSM via `cryptoki`.

The `ReceiptSigner<B>` driver is generic over `B`; the [`DynSigningBackend`]
boxes any concrete backend so the `TurnEngine` can hold an `Arc<ReceiptSigner<Dyn…>>`
without a generic parameter on the engine.

The receipt's canonical bytes are layout-stable and documented inline:

```text
canonical := turn_id (16 LE) ‖ index (8 LE) ‖ prev_head (32) ‖
             payload_digest (32) ‖ post_head (32) ‖ taint (1) ‖
             signed_at_ms (8 LE)            // 129 bytes
signature := Ed25519.sign(sk, canonical)
```

Verifiers in any language can reconstruct this buffer bit-for-bit.

### 2. Pluggable `TsaClient` with an offline simulator for conformance

[`TsaClient`] is async and returns an [`Anchor`] with an opaque
`token: Vec<u8>`. Three [`AnchorKind`]s are recognised at Phase 5:

* `Rfc3161` — DER-encoded RFC 3161 reply (production HTTP client lands in
  Phase 9 alongside the public verifier wrapper).
* `OpenTimestamps` — `.ots` proof anchoring into Bitcoin (Phase 10
  feature-gated; needs a Bitcoin Calendar client).
* `Simulator` — Ed25519-signed `(kind ‖ index ‖ head ‖ ts_ms)` produced by
  [`SimulatorTsaClient`]. Used exclusively by tests and conformance; the
  verifier refuses this kind unless explicitly configured to trust the
  simulator public key.

Why simulator instead of mock: it actually exercises the
sign-then-verify code path, so a regression in the canonical layout (kind
byte, index ordering, head bytes, timestamp) fails the conformance suite
deterministically — without any network or PKI infrastructure.

### 3. `AnchorPolicy::SPECS_DEFAULT = every 1000 appends`

SPECS §IX.D specifies a per-tenant cadence with a default of 1000. The
[`Anchorer`] only fires the TSA call when `count % every_n_appends == 0`
on the new chain length; tests pin the cadence to `EVERY_APPEND` so they
exercise the firing path without 999 padding records.

The rewind-window argument: with cadence `N` and an honest authority's
RTT bounded by `Δ`, the strongest the adversary can rewind the chain is
`N` records or `Δ` time, whichever is smaller (Theorem T11 §III in the
paper).

### 4. Public verifier API as functions, not a service

The Phase-5 verifier API is a set of pure functions:

```rust
verify_receipt(receipt, payload) -> Result<()>
verify_chain(receipts, payloads, expected_final_head) -> Result<()>
verify_simulator_anchor(anchor, simulator) -> Result<()>
verify_anchor_replay(anchor, simulator, payloads) -> Result<()>
verifying_key_from_bytes(bytes) -> Result<VerifyingKey>
```

The HTTP wrapper (Phase 9) is `axum::Router::route(verify, body: SignedReceipt)`
calling these functions verbatim. Zero new logic at HTTP-time.

### 5. `serde-big-array` for the 64-byte signature field

The serde derives don't ship `Deserialize` for `[u8; 64]` out of the box.
Three options: a hex string (human-readable, slower), `serde_bytes`
(`Vec<u8>` round-trip), or `serde-big-array` (zero-copy, deserializes to
`[u8; 64]`). We picked `serde-big-array` for the signature field
(`#[serde(with = "BigArray")]`) since cross-language verifiers consume the
JSON as a length-64 number array, matching the canonical wire form.

## Consequences

- **Pro:** Pure-Rust crypto stack (curve25519-dalek, sha2, ed25519-dalek)
  — no OpenSSL / libsodium / system-crypto.
- **Pro:** Tests are offline and deterministic; the simulator key is
  derived from a 32-byte seed so a Phase-N regression yields a stable
  diff in the canonical bytes.
- **Pro:** The receipt is self-describing — it carries the public key
  whose signature it bears; verifiers MAY trust it (in which case
  `verify(payload)` suffices) or MAY supply an externally-trusted
  [`VerifyingKey`] (in which case `verify_with_key(payload, vk)` rejects
  pubkey rotation).
- **Pro:** The chain primitive stays unchanged; signing is additive.
- **Con:** Simulator anchors are NOT trustless. Production verifiers MUST
  refuse `AnchorKind::Simulator` unless the operator explicitly configures
  a trust root for it.
- **Con:** RFC 3161 + `OpenTimestamps` are not wired in Phase 5. They land
  in Phase 9 alongside the HTTP wrapper and the timestamp-aware UI.
- **Con:** The `signed_at_ms` field is informational. Receipt ordering is
  authoritative via the chain index, NOT the wall clock. The Phase-9
  external verifier surfaces both; the conformance suite covers only the
  index-based ordering.

## Alternatives considered

- **ECDSA-P256.** Wider hardware-token support (TPM 2.0, Yubikey OpenPGP)
  but slower and a larger attack surface. Phase-9 HTTP verifier MAY add
  ECDSA as an additive `AnchorKind` if a deployment requires it.
- **BLS signatures with aggregation.** Useful for cross-tenant batching;
  out of Phase 5 scope. Revisitable when the verifier surfaces a
  bandwidth bottleneck.
- **Anchor every append.** Strongest tamper-evidence but the most
  expensive (one HTTP RTT per append). Available as
  `AnchorPolicy::EVERY_APPEND`; the SPECS default of `every_n = 1000`
  matches the paper's §IX.D.
- **Re-signing on key rotation.** Out of scope — Phase 5 makes pubkey
  rotation observable (`verify_with_key` rejects mismatched pubkeys)
  but does not auto-re-sign older records. The operator manages this via
  the receipt-chain admin API in Phase 9.

## Migration / replacement

The boundary contract — `SigningBackend` + `TsaClient` + `SignedReceipt` +
`Anchor` + the verifier function surface — lives in `gauss-audit`. Either
can be replaced independently:

- Adding an HSM backend: a new `SigningBackend` impl, no other crate
  changes.
- Wiring real RFC 3161: a new `TsaClient` impl that produces
  `AnchorKind::Rfc3161` anchors; the chain code is unchanged.
- HTTP verifier (Phase 9): wraps the existing functions; no
  `gauss-audit` change.
- Cluster-mode signing (Phase 10): partition the chain by `SessionId`;
  each shard has its own Ed25519 key.

[`gauss_audit::sign::Ed25519Signer`]: ../../crates/gauss-audit/src/sign.rs
[`SigningBackend`]: ../../crates/gauss-audit/src/sign.rs
[`DynSigningBackend`]: ../../crates/gauss-turn/src/engine.rs
[`TsaClient`]: ../../crates/gauss-audit/src/tsa.rs
[`Anchor`]: ../../crates/gauss-audit/src/tsa.rs
[`AnchorKind`]: ../../crates/gauss-audit/src/tsa.rs
[`SimulatorTsaClient`]: ../../crates/gauss-audit/src/tsa.rs
[`Anchorer`]: ../../crates/gauss-audit/src/anchor.rs
[`AnchorPolicy::SPECS_DEFAULT`]: ../../crates/gauss-audit/src/anchor.rs
[`VerifyingKey`]: ../../crates/gauss-audit/src/sign.rs
