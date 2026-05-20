# Bug bounty programme

> *Sprint 8 §9 of [`/ROADMAP.md`](../ROADMAP.md). This document is
> the public scope and payout schedule. Independent third-party
> review of `gauss-kernel`, `gauss-audit`, and `gauss-sandbox` is
> in flight.*

## 0. Status

This is the **published-scope, intake-open** phase of the GaussClaw
bug bounty. The intake email + PGP key are below; we triage every
report within five working days.

We do **not** yet have a public bug-tracker for security issues —
please file via the intake address rather than a GitHub issue.

## 1. Scope: in-scope crates

Findings against the following crates qualify for the full payout
schedule (§3):

| Crate | Path | Surface |
|---|---|---|
| `gauss-core` | `gauss-aether/crates/gauss-core` | cap lattice, taint lattice, audit types |
| `gauss-kernel` | `gauss-aether/crates/gauss-kernel` | admit gate, declass map, three planes |
| `gauss-audit` | `gauss-aether/crates/gauss-audit` | receipt chain, Ed25519 + Merkle |
| `gauss-sandbox` | `gauss-aether/crates/gauss-sandbox` | WASM / Landlock / seccomp / bwrap composite |
| `gauss-poly` | `gauss-aether/crates/gauss-poly` | polyhedral provider verifier |
| `gauss-exec` | `gauss-aether/crates/gauss-exec` | Docker / SSH / Modal / Local executors |
| `gauss-checkpoint` | `gauss-aether/crates/gauss-checkpoint` | snapshot + rollback |
| `gaussclaw-skill` | `gaussclaw/crates/gaussclaw-skill` | manifest parser + cap resolution |
| `gaussclaw-plugins` | `gaussclaw/crates/gaussclaw-plugins` | plugin loader + cap admit |
| `gaussclaw-tools` | `gaussclaw/crates/gaussclaw-tools` | first-party tool catalogue |
| `gaussclaw-channels` | `gaussclaw/crates/gaussclaw-channels` | webhook signature verification |
| `gaussclaw-redact` | `gaussclaw/crates/gaussclaw-redact` | outbound-message redaction |
| `gaussclaw-proxy` | `gaussclaw/crates/gaussclaw-proxy` | OpenAI-compat HTTP proxy |
| `gaussclaw-desktop` (updater) | `gaussclaw/crates/gaussclaw-desktop/src/updater.rs` | four-axis chain-anchored verifier — see [`UPDATE_INTEGRITY.md`](UPDATE_INTEGRITY.md) |
| `gauss-worktree` | `gauss-aether/crates/gauss-worktree` | per-session git worktree isolation |

## 2. Out of scope

- The `gauss-zk` research vehicle (Sprint 8 §3 still landing).
- The `gauss-attest` SGX / SEV-SNP / TDX backends (Sprint 8 §4
  still landing).
- The web dashboard's frontend assets (`gaussclaw-web/frontend`).
  Front-end XSS / CSP findings are still welcome but pay at the
  documentation rate, not the security rate.
- Third-party plugins distributed outside the `main` branch — file
  with the plugin author.
- DoS via resource exhaustion against unprivileged endpoints
  (rate-limit findings welcome but pay at the documentation rate).

## 3. Payout schedule

The numbers below are guidance. The triage committee assigns the
final tier based on severity, exploitability, and the quality of
the report.

| Severity | Examples | Payout (USD) |
|---|---|---|
| **Critical** | Cap-lattice escape (admit gate can be tricked into letting through a refused cap). Receipt-chain forgery (a verifier accepts a chain that wasn't signed by the publisher key). Sandbox escape (WASM module exfils to the host process). Updater four-axis bypass that doesn't require publisher key compromise. | $10 000 – $50 000 |
| **High** | Taint-lattice underflow (an Adversarial message gets read as User without a declassification entry). Plugin loader admits a plugin whose declared caps exceed the live grant. Outbound redaction bypass that leaks a credential into the audit chain. | $3 000 – $10 000 |
| **Moderate** | Tool input validation that lets a caller crash the worker without admit gate refusal. Channel adapter accepts a tampered webhook payload (HMAC bypass / replay window slip). Race in the receipt chain that produces a duplicate index. | $1 000 – $3 000 |
| **Low / Documentation** | Spec ambiguity that could lead to a future implementation bug. Inconsistent cap declarations across docs vs source. Discrepancy between `cargo clippy` and shipping code on a security-relevant warning. | $250 – $1 000 |

## 4. What we won't reward

- Reports that say "this Rust crate uses unsafe" without a
  demonstrable safety failure.
- Cap+taint findings against the **research** crates (`gauss-zk`,
  `gauss-attest`, `gauss-dp`). They're not yet production-grade
  and we say so in their crate-level doc.
- Generic AI-safety concerns (model output is unsafe / model
  refuses safe queries). Send those to the upstream model
  vendor — we do not own the inference.
- Findings already disclosed publicly without coordination.

## 5. How to file

1. Encrypt your report with the GaussClaw security PGP key:
   `1A2B 3C4D 5E6F 7890 ABCD EF01 2345 6789 ABCD EF01` (sample
   fingerprint; the real key is published on
   `https://gauss-aether.io/.well-known/security.txt`).
2. Email `security@gauss-aether.io` with subject
   `[bug-bounty] <one-line summary>`.
3. Include:
   - Affected crate + git commit / version.
   - Minimal reproduction (a failing test case is ideal).
   - Severity tier you believe applies.
   - Bank / cryptocurrency payment details (we pay in USD or USDC).

We acknowledge within 24 hours, triage within 5 working days, and
pay within 14 working days of confirming the finding.

## 6. Coordinated disclosure timeline

We treat 90 days from the first acknowledgement as the disclosure
window. We'll request an extension if the fix requires upstream
coordination (e.g. a `wasmi` or `axum` patch). Public disclosure
happens via:

1. A GitHub Security Advisory on `rismanmattotorang/gauss-aether`
   linked from the next release notes.
2. A CVE assignment when warranted.
3. A credit line on
   [`docs/SECURITY.md`](SECURITY.md) (unless the reporter prefers
   anonymity).

## 7. Independent review

We're contracting an external security firm for a one-time review
of the cap-lattice + audit-chain design. Findings from that
engagement land here under "External review" as a separate
section. (Sprint 8 §9 deliverable; will be linked once the report
ships.)
