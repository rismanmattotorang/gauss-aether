# Gauss-Aether — Security Posture & Threat Model

## Threat model

Gauss-Aether's safety story rests on four kinds of structural defence,
each tied to an axiom or theorem:

| Threat                                          | Defence                                                                                 | Pin            |
|-------------------------------------------------|-----------------------------------------------------------------------------------------|----------------|
| Tool fires before its action is logged          | WAL barrier in the DTE: `apply_actions_locally` unreachable before `memory.append` Ok.  | A1 / T1        |
| Capability escalation by a compromised tool     | `Kernel::contract` is the only mutator; can shrink, never grow; CAS-protected.          | A2 / T2        |
| Log tampering                                   | SHA-256 receipt chain + Ed25519 signature over canonical bytes.                         | A3 / T3 / T11  |
| One plane starves another                       | Three independent atomic token-bucket plane pools with `B/ρ` worst-case wait.           | A4 / T4        |
| Information-flow leak across observations       | Antitone declass map verified at registration; capability admission joins taint.        | A6             |
| Worker compromise contaminates parent context   | HWCA worker contexts; only `ValidatedValue` crosses the schema gate boundary.           | A7 / T9        |
| Indirect-prompt injection in tool output        | Four-stage schema gate (length cap → JSON Schema → instruction filter → taint join).    | T9 (≤ 2.19%)   |
| Sandbox escape                                  | Four orthogonal sandbox layers; the composite product bound covers single-layer breaks. | T10            |
| Unauthorised side-effect execution              | SAG approval round-trip between admission and the WAL append; deny-on-timeout.          | A8             |
| Provider swap behavioural divergence            | Polyhedral verifier — byte-equal canonical-JSON over a probe set.                       | T7             |
| Workload swap on a cluster node                 | TEE attestation simulator + canonical-pre-image verifier.                               | T10 §L4        |

## Out-of-scope threats (Phase 11)

These are explicitly **not** covered by the 1.0 release:

- **Direct hardware attestation** (SEV-SNP / TDX / CCA) — Phase-11
  plugin crates. Phase-10 ships the trait + Ed25519 simulator only.
- **Compromised CSPRNG** — `gauss-dp` uses `rand_core::OsRng` in
  production; auditing the host's randomness source is the operator's
  responsibility.
- **Cryptographic-curve weaknesses** — Ed25519 is the choice (ADR-0003);
  if Curve25519 falls, Phase-11 cycles via the `SigningBackend` trait.
- **Side-channel attacks on the schema gate** — the four-stage gate
  short-circuits on the *first* failure for diagnosability; a timing
  attacker may learn which stage failed. Production deployments that
  need constant-time semantics can wrap the gate.
- **Adversarial network conditions during anchoring** — RFC 3161 +
  OpenTimestamps backends are additive plugin crates; the v2
  `gauss-zk` crate provides offline anchoring as a complement.

## Privileged crates (Tier-0)

Changes to these crates require dual review:

- `gauss-kernel`
- `gauss-audit`
- `gauss-attest`

These crates' compile-time and runtime invariants are the foundation
of every other crate's safety story; lapses cascade.

## Cryptographic stack (ADR-0003)

- **Hashing**: SHA-256 (FIPS 180-4) for the receipt chain;
  BLAKE3-ready in `gauss-traits` but not currently used.
- **Signing**: Ed25519 via `ed25519-dalek` 2.x with `zeroize`-on-drop
  secret keys.
- **Symmetric**: out-of-scope at 1.0; deployments that need
  encryption-at-rest layer SurrealDB encryption.

## CSPRNG handling

- Phase-5 signing accepts a `CryptoRng + RngCore` via
  `Ed25519Signer::generate`. Production deployments pass `OsRng`.
- Phase-5 keys are also derivable from a 32-byte seed (`from_seed`)
  for deterministic test vectors.
- The DP mechanisms in `gauss-dp` *require* a `CryptoRng` — the
  trait bound makes weak RNGs a compile-time refusal.

## Responsible disclosure

If you find a security issue:

1. **Don't open a public issue.**
2. Email the maintainer team (see the repo's GitHub profile for the
   current contact). Include:
   - Affected crate + version.
   - Reproducer (minimal, runnable).
   - Impact assessment.
3. Expect an acknowledgement within 72 hours and a coordinated
   disclosure timeline. Default: 90 days from acknowledgement to
   public disclosure; faster for actively-exploited issues.

## Supply-chain hygiene

- `cargo deny check` runs on every PR (license + advisory + duplicate
  + sources lint).
- The workspace MSRV is pinned to 1.83; CI cycles MSRV on the first
  of every month.
- All optional dependencies are gated behind named features so the
  default build surface is small.
- `unsafe_code = "forbid"` workspace-wide. Plugin crates that need
  `unsafe` (e.g. real-hardware attestation backends) MUST scope it
  to a single function with an `# Safety` rustdoc justification.

## Security-related conformance pins

| Pin                                                  | What it checks                                                  |
|------------------------------------------------------|-----------------------------------------------------------------|
| `axiom_a1_wal_before_effect::crash_injection_*`      | A crash between WAL append and side-effect leaves state ∈ {s,s′}.|
| `axiom_a2_kernel_contract_only::*`                   | Capability grant can shrink, never grow.                         |
| `theorem_t3_merkle_tamper_evidence::*`               | Any payload mutation diverges the chain head.                    |
| `axiom_a7_and_theorem_t9_hwca::ipi_corpus_success_rate_is_below_paper_bound` | IPI escape rate ≤ 2.19 %. |
| `theorem_t10_composite_sandbox::composite_refuses_when_class_is_insufficient` | Composite refuses tools whose cap exceeds the layers. |
| `theorem_t6_stateless_scaling_and_attest::attestation_rejects_tampered_nonce` | Replay defence in the attestation verifier. |
| `phase11_release::one_point_zero_pareto_dominates_every_predecessor`   | 1.0 scorecard beats every predecessor on every axis. |

Run these specifically with:

```bash
cargo test --workspace --no-fail-fast --tests
```
