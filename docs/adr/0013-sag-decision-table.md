# ADR-0013 — Supervised Autonomy Gradient: decision table + approval surface

**Status:** Accepted (Phase 7)
**Date:** 2026-05-16
**Locks:** Axiom A8 (Supervised Autonomy Gradient)

## Context

The Differential Turn Engine emits tool actions that the kernel admits
under joint capability + taint (Axiom A2 / A6). Some admissible actions
are still risky enough that a human should approve them before the
sandbox executes them. The paper §XI calls this the **Supervised Autonomy
Gradient (SAG)**: every action is classified into one of four bands —
`Auto`, `Notify`, `RequireApproval`, `Deny` — and the engine routes the
`RequireApproval` band through a human-in-the-loop surface (Telegram,
Slack, CLI, SSE) with a 5-minute deadline (paper §XI.C).

Three implementation choices needed locking:

1. **Classifier shape.** Decision table vs scorer vs learnt LM. The paper
   §XI.B specifies an audit-friendly, monotonicity-checkable rule set; a
   learnt classifier doesn't satisfy the auditability constraint and is
   deferred to Phase 10's research-track. We picked a rule-driven
   `DecisionTable`.
2. **Surface abstraction.** Telegram / Slack / Discord / Matrix / CLI /
   SSE are all production targets; tests need a deterministic in-process
   surface that doesn't talk to the network. We picked a trait
   (`ApprovalSurface`) with three test impls (`AutoApprove`, `AutoDeny`,
   `ChannelSurface`); production surfaces ship in Phase 9 as additive
   trait impls.
3. **Where the gate sits in the turn.** Before or after admission? Before
   or after the WAL append? The answer must preserve A1 (WAL barrier)
   and avoid duplicate appends for an action that was approved.

## Decision

### 1. `DecisionTable` of `Rule`s + monotonicity verifier

The classifier is an ordered `Vec<Rule>` plus a fall-through `Risk`. Each
[`Rule`] pairs a [`Predicate`] (a small algebra: `Always`, `ContainsCap`,
`TaintAtLeast`, `NonReversible`, `Tool`, `All`, `Any`) with an outcome
band + an operator-readable label. The first matching rule wins.

The paper §XI.B says the classifier MUST be monotone — relaxing one input
field (lower cap, lower taint, more reversible) can never tighten the
outcome. The verifier `verify_monotonicity(table)` enumerates a small
representative cap × taint × reversibility grid and asserts the property
holds; production tables call this once at startup. The Phase-7 default
table passes the verifier from the SAG-crate unit test AND from the
`gauss-conformance` cross-crate vantage.

### 2. `ApprovalSurface` trait + three test surfaces

```rust
#[async_trait]
pub trait ApprovalSurface: Send + Sync {
    async fn request_approval(
        &self,
        request: ApprovalRequest,
        deadline: Duration,
    ) -> GaussResult<ApprovalDecision>;
}
```

Test surfaces:

* `AutoApprove` — every request approves; useful for tests where the
  classifier path is the unit under test.
* `AutoDeny` — every request denies; useful for asserting the
  `AutonomyDenied` short-circuit.
* `ChannelSurface` — `tokio::sync::mpsc`-driven; a pretend operator
  receives the request and pushes a decision back. Used by the
  conformance test that asserts the round-trip.

Production surfaces (Phase 9): Telegram inline-keyboard, Slack
interactive message, Discord buttons, Matrix `m.room.message` w/ reply,
CLI/TUI blocking prompt, SSE web widget. Each is an additive impl of the
same trait — the contract here is the stable handoff.

### 3. SAG runs AFTER admission, BEFORE the WAL append

The Phase-7 turn lifecycle:

```text
1. INGEST     join taint(o) into ℓ
2. GENERATE   ask provider π for actions
3. ADMIT      kernel.admit(k(a), ℓ) for each tool action     (Axiom A2/A6)
3a. SAG GATE  classify(a) → maybe surface.request_approval(a)  (Axiom A8) ← NEW
4. WAL        memory.append(record(o, a, ℓ, sag_decisions))   (Axiom A1)
5. COMMIT     external effects fire AFTER the append
```

Why this position:

* AFTER admission: kernel-denied actions never reach the human, saving
  approver attention.
* BEFORE the WAL append: a denied / timed-out action leaves no chain
  entry, so the chain head reflects only the actions that committed.
* The `sag_decisions: Vec<SagDecisionRecord>` is bundled into the
  canonical payload that the Phase-5 signed receipt covers — so the
  approval verdict is part of the EUF-CMA signature alongside the action
  set.

### 4. Default deadline = 5 minutes, deny-on-timeout

Paper §XI.C: `Δ_approval = 300 s`. Missing the deadline returns
`GaussError::AutonomyApprovalTimeout`. The deadline is per-gate and
overridable via `ApprovalGate::with_deadline(...)`; tests run with
millisecond-level deadlines under `tokio::time::pause()`.

## Consequences

- **Pro:** Pure-Rust, no `unsafe`; trait-based surface keeps the
  production-adapter layer additive.
- **Pro:** The default rule list is auditable line-by-line, with a
  human-readable label per rule. A code review of the table is the
  policy review.
- **Pro:** Monotonicity is a property test, not a code review item;
  a misconfigured rule never lands.
- **Pro:** Approval decisions are part of the signed receipt — the
  chain captures who approved what and when, with cryptographic
  non-repudiation (Phase 5).
- **Con:** Decision-table expressiveness is bounded. A statistical-LM
  scorer for adversarial-prompt detection is Phase 10's research item.
- **Con:** The Phase-7 surfaces are all in-process; production adapters
  are gated on Phase 9's channel layer. Deployments that need a
  human-in-the-loop today should wrap an `ApprovalSurface` impl over
  the channel of their choice.
- **Con:** Approver authentication is the surface's responsibility — the
  trait surfaces an `approver: String` opaque to the engine. Phase 9
  ties this to the channel adapter's authenticated identity.

## Alternatives considered

- **Pure learnt LM classifier.** Not auditable; deferred to Phase 10.
- **Sandbox-level approval gate (Phase 3 layer).** The sandbox runs
  AFTER the WAL append; pushing approval that late would mean denied
  actions still committed to the chain. Rejected.
- **Per-tool inline classifier in the tool manifest.** Less flexible
  than a global table; admin would have to update each tool to change
  policy. Rejected.
- **3-band lattice (`Auto < Approve < Deny`).** Notify is a useful
  middle band for "loud auto-execute" (irreversible reads, slow
  network calls) — keep four bands.

## Migration / replacement

The boundary contract — `Classifier` + `DecisionTable` + `Predicate` +
`ApprovalSurface` + `ApprovalRequest` + `ApprovalDecision` —  lives in
`gauss-sag`. Either side can be replaced independently:

- New production approval surface (Phase 9): implement `ApprovalSurface`
  in `gauss-channel-*` and pass it to `ApprovalGate::new(...)`.
- Phase-10 statistical classifier: implement `Classifier` and wrap the
  existing `DecisionTable` as a fast-path / fallback.
- Per-tenant tables: load the `DecisionTable` from disk / config; the
  `serde` impl is already in place.

The conformance suite already exercises the gate through the engine; new
classifiers / surfaces drop in via the same trait surface.
