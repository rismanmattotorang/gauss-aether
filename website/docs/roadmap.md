---
id: roadmap
title: Roadmap
sidebar_position: 10
---

# Roadmap

GaussClaw 1.0 ships every Hermes-parity surface (CLI, TUI, web, desktop,
gateway, ~20 channels, 20 vendor drivers, two meta-routers, 30+
first-party tools), every safety primitive (kernel admit, 4-layer
sandbox, taint lattice, receipt chain, schema-gated worker boundary,
polyhedral provider equivalence), and the Cryptographic Trajectory
Envelope for SFT/DPO export.

What we're working on next:

## Near term

- **Bug-bounty programme.** Independent review of the kernel, audit
  chain, and sandbox by an external security firm. Public scope and
  payout schedule.
- **Hardware attestation.** SGX / SEV-SNP / TDX backends for
  `gauss-attest`, so a remote verifier can prove a turn ran inside a
  genuine enclave.
- **iOS and Android desktop shells.** Tauri 2 Mobile (`gaussclaw-desktop`
  + platform-specific signing).

## Research vehicles

These ship behind stable trait contracts in the runtime; production
plugins implement them as additive crates.

| Crate | Purpose |
|---|---|
| `gauss-zk` | Zero-knowledge proofs over the receipt chain (Merkle commitments + statements). |
| `gauss-dp` | Differentially-private trajectory exporter — Laplace + Gaussian mechanisms. |
| `gauss-learnt` | Learnt risk classifier `Φ̂` — logistic scorer that *floors* the SAG rule table. |
| `gauss-robust` | Robust declassifiers — adversarial-rejection counters that tighten the declass map. |
| `proofs/lean/` | Lean 4 stubs of all nine axioms and twelve theorems; proofs discharged incrementally. |

## Non-goals

- **No proprietary cloud lock-in.** GaussClaw runs entirely on your
  hardware, your VPS, or your cluster — there is no "GaussClaw Cloud"
  product and no plan for one.
- **No telemetry pings home.** GaussClaw never reports usage to a
  central server.
- **No abandoning Hermes parity.** The replay corpus, the OpenAI SDK
  parity gate, and the CLI `--help` diff are part of the conformance
  suite and stay green.

Track the full plan in the GitHub
[milestones](https://github.com/rismanmattotorang/gauss-aether/milestones)
and on the
[project board](https://github.com/rismanmattotorang/gauss-aether/projects).
