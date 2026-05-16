# ADR-0003 — Receipt crypto suite: Ed25519 + BLAKE3 + SHA-256

**Status:** Accepted (Phase 0)
**Date:** 2026-05-16

## Context

The receipt chain (paper §XII, Axiom A9, Theorems T3 + T11) requires:

- A signature scheme over per-action records with EUF-CMA security.
- A collision-resistant hash for the record canonicalisation digest.
- A second collision-resistant hash for the chain link `c_i = H(c_{i-1} ‖ ρ_i)`,
  ideally interoperable with RFC 3161 / OpenTimestamps so the public anchor
  step is a one-call operation.

Performance matters. Per-turn signature cost translates directly into
end-to-end latency; per-chain-update hash cost translates into throughput.

## Decision

- **Signing:** **Ed25519** via `ed25519-dalek` v2.
  Justification: ~80 µs sign / ~120 µs verify on commodity hardware; batch
  verification supported; small key sizes; well-vetted; no external state.
  EUF-CMA security at λ = 128 bits matches the theorem statement (T11).
- **Record canonicalisation digest:** **BLAKE3**.
  Justification: 3–10× faster than SHA-256 on the same hardware; SIMD-friendly;
  collision-resistant; the record is internal to Gauss-Aether so we don't
  need RFC-3161 interop here.
- **Chain link hash:** **SHA-256** via `sha2`.
  Justification: RFC 3161 timestamping authorities sign SHA-256 digests; using
  SHA-256 for the chain link lets us submit the chain head directly to a TSA
  without re-hashing.

## Consequences

- Two hash functions live in the audit path. We pay the BLAKE3 cost on every
  receipt and the SHA-256 cost on every chain extension. Per-turn overhead is
  measured in single-digit microseconds.
- The verifier API publishes both `(record_hash, sig)` and `chain_head`, so
  external auditors can re-derive everything from a fixed crypto suite.
- Migrating off Ed25519 is a semver-major event (chain semantics change).
  We accept this in exchange for the simplicity of Ed25519.
- FIPS-only deployments cannot use BLAKE3 directly; we accept this for the
  base profile and will offer a FIPS-mode crate feature in Phase 10 that
  swaps to SHA-256 for the record digest as well.

## Alternatives considered

- **Ed448** (RFC 8032). Larger signatures, slower; no compelling reason over
  Ed25519 at the security level we target.
- **secp256k1 / ECDSA.** Wider HSM support, but malleability issues and the
  ecosystem is harder to reason about.
- **All-SHA-256.** Simpler suite, but BLAKE3's throughput gain on per-receipt
  hashing is genuinely valuable on hot agents.
- **All-BLAKE3.** Loses TSA interop, which is the whole point of the public
  anchor.
