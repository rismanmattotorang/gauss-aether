# Sprint 14 — SOP engine (ZeroClaw-inspired)

> *Status: proposal. Not yet promoted into `ROADMAP.md` as the
> per-sprint contract. Once a maintainer approves, the §3 deliverables
> below get copied into `ROADMAP.md` and this file links there.*

---

## 1. Context

ZeroClaw (`zeroclaw-labs/zeroclaw`) ships an **SOP engine** —
"event-triggered automation" wired to MQTT / webhooks / cron /
peripheral events, with approval gates and resumable runs. It is the
one ZeroClaw subsystem GaussClaw has no equivalent for:

| ZeroClaw subsystem | GaussClaw equivalent (today) |
|---|---|
| 30+ channels | ✅ 11 (Sprint 7) — and the gap is China-market, deliberately deferred |
| ~20 providers | ✅ 21 effective (Sprint 1-2) |
| Cap+receipt security | ⭐ cap+taint kernel + Merkle chain + envelope verifier |
| ACP / JSON-RPC | ✅ `gaussclaw-acp` (Sprint 8 §6) |
| Plugin loader | ✅ `gaussclaw-plugins` (Sprint 7 §1) |
| Workspace boundaries / sandboxes | ⭐ 4-layer composite (`gauss-sandbox`) |
| Tool receipts | ⭐ Ed25519 + Merkle (`gauss-audit`) |
| Supervised / YOLO autonomy modes | 🟡 cap+taint grants — *not* surfaced as a profile UX |
| Hardware peripherals (GPIO/I2C/SPI/USB) | 🚫 |
| Firmware targets (STM32 / ESP32 / Arduino) | 🚫 |
| **SOP engine** (event → workflow → approval → resume) | **🚫** |
| K8s manifests + Helm chart | 🚫 |
| `install.sh` one-liner | 🚫 |
| Plugin marketplace (signed catalog) | 🟡 plugin loader exists; no catalog |

The other rows are real gaps but lower-leverage. SOP is the highest-
leverage because (a) it's the one operational subsystem with no
GaussClaw analogue at all, (b) it composes the structural wins we
already have — every SOP step is just a cap-gated dispatch that
chains into the same Merkle receipt log, so the safety story is
inherited for free, and (c) ZeroClaw's value prop "AI personal
assistant that does things on schedule and on events" is mostly
*just* the SOP engine.

This sprint ships the SOP engine. The other ZeroClaw-inspired items
get their own proposal docs if/when a maintainer prioritises them.

---

## 2. Karpathy-style framing

Before §3, the four principles applied to this sprint:

**Think Before Coding.** The genuine gap is *event-driven
orchestration*, not "another trigger source". GaussClaw already has
`gauss-cron` (Sprint 5 §1), HMAC-verified webhook channels (Sprint 1),
the cap+taint admit gate, and the Merkle receipt chain. The SOP engine
is the **glue** between them: a typed `(Trigger → ApprovalGate? →
Workflow → Receipt)` pipeline. It is **not** a new trigger source
catalogue, **not** a new sandbox, **not** a new permission model.
Assumption: an SOP run is a *first-class session* — same
`SessionStore`, same receipt chain, same cap grant. Anything else
would fork the safety story.

**Simplicity First.** Minimum shippable engine: one trait surface
(`Trigger`, `Workflow`, `ApprovalGate`), one durable runner with
checkpoint+resume, one reference `Trigger` impl (webhook — reuses the
existing channel signature primitive), one reference `Workflow` impl
(a sequence of `ToolCall`s). MQTT and peripheral triggers ship as
*follow-on* crates, not in this sprint.

**Surgical Changes.** New crate `gauss-sop`. Two new cap bits
(`SOP_DEFINE`, `SOP_TRIGGER`). One new CLI subcommand (`gaussclaw sop`)
with five verbs. One new dashboard page (`SopPage`). Zero changes to
`gauss-kernel`, `gauss-audit`, `gauss-sandbox` — the engine is a
*consumer* of those, not an extension.

**Goal-Driven Execution.** Each deliverable below has a green-test or
working-demo success criterion. The sprint doesn't ship until
`cargo test --workspace --lib` is green across all of them.

---

## 3. Deliverables

Status legend matches `ROADMAP.md`: ✅ shipping · 🟡 partial · ❌
scaffold · 🚫 absent · ⭐ structural superiority.

All eight deliverables target the new crate `gauss-aether/crates/
gauss-sop/`. The crate is `no_std`-incompatible (uses `tokio`) but
holds only the engine; trigger and workflow leaf impls are in adjacent
crates so the dep graph stays tight.

### §1. `gauss-sop` crate — engine + trait surface

```rust
// gauss-sop/src/lib.rs (target shape)
pub trait Trigger: Send + Sync {
    fn name(&self) -> &str;
    async fn next(&mut self, cancel: CancelHandle) -> Option<TriggerEvent>;
}

pub trait Workflow: Send + Sync {
    fn id(&self) -> WorkflowId;
    fn required_caps(&self) -> CapToken;
    async fn execute(
        &self,
        ctx: &mut WorkflowCtx,
        event: &TriggerEvent,
    ) -> Result<WorkflowOutcome, SopError>;
}

pub trait ApprovalGate: Send + Sync {
    async fn approve(&self, sop: &SopDef, event: &TriggerEvent)
        -> ApprovalDecision;
}
```

- `SopDef { id, trigger: Box<dyn Trigger>, workflow: Box<dyn
  Workflow>, gate: Option<Box<dyn ApprovalGate>>, caps: CapToken }`.
- `SopEngine::register` refuses if `sop.caps` isn't satisfied by the
  live grant — same shape as `PluginRegistry::register` (Sprint 7).
- `SopRunReceipt { sop_id, trigger_event_digest, workflow_outcome,
  chain_head_before, chain_head_after, signature }`.

**Success criterion.** `cargo test -p gauss-sop` passes 12+ unit tests
covering: engine registration, cap-refusal at register, single-trigger
single-event single-workflow dispatch, approval-gate accept / refuse /
defer, and `SopRunReceipt` hashing.

### §2. Durable run state + resume

ZeroClaw's pitch is "resumable runs". GaussClaw's equivalent must:

1. Persist `RunState { sop_id, trigger_event, step_index, partial_outputs,
   chain_head }` after every workflow step.
2. On engine restart, resume each `RunState` from `step_index` —
   *not* from step 0. Approval gates that were `Pending` re-fire.
3. Store `RunState` in the existing `SessionStore` (Trinity-backed) so
   the chain stays one chain.

**No new persistence layer.** Reuse `SessionStore`. The SOP engine
writes one session per `(sop_id, trigger_event.digest())` and steps
become turns. This is the keystone simplicity move — it inherits FTS,
HNSW, receipt-chain, and replay for free.

**Success criterion.** A test that registers an SOP, fires a trigger,
runs the workflow to step 2 of 4, kills the engine, restarts, and
sees the workflow complete to step 4 with one continuous Merkle chain.

### §3. Reference `Trigger`: webhook

The webhook ingress already exists in `gaussclaw-channels::webhook`
(HMAC-verified). The new `WebhookTrigger` is a *subscriber* on that
channel: a webhook event matching `(path, method, payload_schema)` is
re-emitted as a `TriggerEvent`. **Adversarial-taint default on
ingress** (same policy as the channel adapter).

- Cap-gated by `cap:sop:trigger` (new bit 12).
- The trigger's `next()` returns `None` only on cancel — never on
  transient error; transient errors emit `TriggerEvent::Error` so the
  workflow can decide whether to proceed.

**Success criterion.** Integration test: POST to the test webhook
adapter fires the trigger; the workflow records one receipt; the
adversarial taint propagates through to the workflow output. The
receipt chain replays byte-for-byte under `gaussclaw-conformance`.

### §4. Reference `Workflow`: tool sequence

Minimum-viable workflow runner. A workflow is a `Vec<WorkflowStep>`
where `WorkflowStep` is one of:

- `Tool(ToolCall)` — dispatch through the existing HWCA spawner.
- `Approval(GateRef)` — pauses with a structured pending state.
- `Branch { predicate, then_, else_ }` — predicate is a Rhai-free
  pure-Rust comparator (`eq` / `ne` / `lt` / `gt` / `contains`) over
  the running JSON value. Scripting languages are explicitly out of
  scope for this sprint.

Two non-goals (deliberate): no loops, no parallel steps. Both can land
as follow-ons; without them the engine is still useful for 80% of
ZeroClaw's quoted use cases (notification fan-out, scheduled
summaries, webhook → file write).

**Success criterion.** A workflow `[http_get → json_get → file_write
→ approval → send_message]` runs end-to-end against an in-memory
channel sink, producing five receipts on one chain.

### §5. CLI surface

```
gaussclaw sop list                          # registered SOPs
gaussclaw sop define <file>                 # load from TOML
gaussclaw sop run <id> [--event-json <p>]   # fire manually
gaussclaw sop status <run_id>               # inspect resumable state
gaussclaw sop receipts <id>                 # chain summary for an SOP
```

- Define accepts a TOML schema (`sop.toml`) that mirrors the in-process
  `SopDef` shape; one example ships in
  `gauss-sop/examples/webhook-to-file.toml`.
- All five verbs require `cap:sop:define` or `cap:sop:trigger` per
  action; the kernel admit gate refuses anything missing the cap.

**Success criterion.** End-to-end CLI test: define an SOP from
TOML, fire it manually, observe the receipt-chain head advance.

### §6. Web dashboard: `SopPage`

One more dashboard page (the 10th alongside chat / sessions / tools /
receipts / cron / analytics / logs / profiles / health / settings).

- Lists registered SOPs with cap+taint badge + last run timestamp +
  last-receipt chain head.
- Click an SOP → run history, with deep-link into the existing
  Receipts explorer.
- **No editor.** Operators define SOPs via TOML on disk; the UI is
  read+monitor only. The editor is a Sprint 15 follow-on if demand
  materialises. (Surgical Changes: don't ship a feature we haven't
  validated demand for.)

**Success criterion.** Dashboard test renders the page against a
recorded `SopEngine` snapshot; receipt-chain link round-trips.

### §7. Pending approval surface

Approval gates re-use the existing TUI `approval` overlay (Sprint 1)
and the dashboard `approval` modal. The SOP engine's `ApprovalGate`
trait has one production impl: `OperatorApprovalGate` — emits an
`ApprovalRequest` on the existing approval channel and blocks until
`accept` / `refuse` / `timeout`. Timeout defaults to 24h; configurable
per SOP.

**Success criterion.** Test that an SOP with an approval gate pauses,
operator approval routes through the existing channel, and the
workflow resumes from the gate step without re-running prior steps.

### §8. Conformance gate

A new `gaussclaw-conformance` test (`sop_replay.rs`) replays a frozen
SOP run trajectory (one webhook fire, three tool steps, one approval,
one final tool step) and asserts byte-identical receipt-chain heads.
This is the SOP equivalent of the existing 1 000-turn Hermes replay
gate.

**Success criterion.** `cargo test -p gaussclaw-conformance
sop_replay` green; the test fixture lives at
`gaussclaw-conformance/fixtures/sop_webhook_basic.jsonl`.

---

## 4. Out of scope (explicit non-goals)

The following ZeroClaw-adjacent features are **not** part of this
sprint. Each is a candidate for its own proposal:

- **MQTT trigger.** Follow-on crate `gauss-sop-mqtt` over `rumqttc`,
  cap-gated by a new `cap:network:mqtt` bit. Ships when there's a
  user who needs it.
- **Peripheral triggers (GPIO line interrupts).** Requires
  `gauss-hardware` first.
- **Workflow loops + parallel steps.** A real demand-driven extension
  of `WorkflowStep`. Don't speculate.
- **SOP marketplace (signed catalog of community SOPs).** Builds on the
  Sprint-7 plugin-loader trust model. Not now.
- **Workflow scripting language (Rhai / Lua / WASM).** Adds attack
  surface. Reject until a concrete use case can't be served by typed
  `WorkflowStep` variants.
- **ZeroClaw autonomy-profile UX (supervised / yolo).** Tracked as a
  separate proposal; orthogonal to SOP.
- **Hardware integration / firmware targets.** Separate proposal.
- **K8s manifests / install.sh one-liner.** Deployment proposals,
  separate.

Calling these out by name so the sprint can be reviewed against
"is the scope honest?" rather than "did we forget X?".

---

## 5. Resource estimate

| Deliverable | Size | Risk |
|---|---|---|
| §1 engine + trait surface | M | Low — clean trait design |
| §2 durable run + resume | M | Medium — re-using `SessionStore` keeps the surface honest but the step-as-turn mapping needs care |
| §3 webhook trigger | S | Low — channel adapter already exists |
| §4 tool-sequence workflow | M | Low — HWCA spawner already exists |
| §5 CLI surface | S | Low |
| §6 dashboard page | S | Low |
| §7 approval gate wiring | S | Low — overlays exist |
| §8 conformance replay | S | Medium — frozen-fixture stability |

**Total: L (~2-3 weeks focused effort).** Smaller than Sprints 5-8
because most surfaces (caps, audit chain, channels, HWCA, overlays,
SessionStore) already exist.

---

## 6. Decision points needing maintainer call

1. **Step = turn, or step = sub-turn?** Storing each workflow step as
   a full session turn is the maximum-reuse path; it also means an
   SOP run appears in `gaussclaw sessions list` next to interactive
   sessions. Acceptable, or do we need a discriminator?
   *Recommendation: ship as a turn with a `kind=sop` discriminator on
   the session row.*
2. **TOML schema or builder API as the primary definition surface?**
   ZeroClaw uses TOML. Hermes uses Python objects. We can do either.
   *Recommendation: TOML primary, builder for tests.*
3. **Cap bits.** Two new bits (`SOP_DEFINE` = 12, `SOP_TRIGGER` = 13).
   The lattice has room. Confirm before the bit allocation lands.
4. **Webhook trigger reuses the channel adapter, or a separate HTTP
   endpoint?** Reusing means `POST /webhook/...` ingresses can route to
   either a channel handler or an SOP trigger (or both). *Recommendation:
   reuse — one ingress, two dispatch paths.*
5. **Approval timeout default.** ZeroClaw doesn't specify. 24h matches
   typical operator-on-call cadence. Confirm or set per-SOP only.
   *Recommendation: 24h global default, per-SOP override.*

---

## 7. Where this lives

Until promoted: `docs/SPRINT_ZEROCLAW_SOP.md` (this file).

On approval: §3 deliverables get copied into `ROADMAP.md` as
"Sprint 14 — SOP engine" with the same numbering. This file then
becomes a one-line redirect: *"Promoted into ROADMAP.md §Sprint 14."*

Tracking on GitHub: one milestone (`Sprint 14`), eight cards (one per
deliverable), eight PRs (one per card). The sprint doesn't ship until
`cargo test --workspace --lib` is green for every commit.
