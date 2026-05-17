# Gauss-Aether — Mechanised proofs (v2 horizon §XVIII.E.1)

The Lean 4 stubs in `GaussAether/Axioms.lean` state every axiom (A1–A9)
and theorem (T1–T12) from the source paper as Lean propositions. The
proofs are stubbed with `sorry` / `trivial` placeholders because the
v2 horizon ships the **type-signature contract** rather than the
proofs themselves; mechanising each theorem is a separate research
contribution.

## Building (when Lean is installed)

```bash
cd proofs/lean
lake build
```

## How this composes with `gauss-conformance`

Each Lean theorem above corresponds to a Rust conformance test:

| Lean theorem            | Rust conformance module                                   |
|-------------------------|-----------------------------------------------------------|
| `CrashAtomicity`        | `axiom_a1_wal_before_effect`                              |
| `CapNonInterference`    | `axiom_a2_kernel_contract_only`                           |
| `MerkleTamperEvidence`  | `theorem_t3_merkle_tamper_evidence`                       |
| `StarvationBound`       | `theorem_t4_starvation_bound`                             |
| `HybridRecallBound`     | `theorem_t5_hybrid_recall`                                |
| `StatelessScaling`      | `theorem_t6_stateless_scaling_and_attest`                 |
| `ProviderAdjunction`    | `theorem_t7_provider_adjunction`                          |
| `ParetoDominance`       | `phase11_release`                                         |
| `IPIContainment`        | `axiom_a7_and_theorem_t9_hwca`                            |
| `CompositeSandboxBound` | `theorem_t10_composite_sandbox`                           |
| `ReceiptNonRepudiation` | `axiom_a9_signed_receipts` (Phase 5 audit crate tests)    |
| `DeltaWarmSwitch`       | `theorem_t12_delta_warm_switch`                           |

A theorem is considered "validated at the 1.0 level" when both the
Lean side type-checks AND the Rust side passes; the Rust tests are
in-band today, the Lean proofs land incrementally as research
contributions.

## Coq mirror

A Coq mirror of these stubs (`proofs/coq/`) is the v2 follow-up for
operators on a Coq-only toolchain. The structure mirrors this Lean
directory byte-for-byte so contributions to one translate
mechanically to the other.
