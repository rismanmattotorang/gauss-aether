# ADR-0015 — Canvas core widget set + streaming migration

**Status:** Accepted (Phase 9)
**Date:** 2026-05-16
**Proves:** (alongside Phase-9 surface layer) Theorem T8 (Pareto-dominance scorecard)

## Context

The A2UI Live Canvas Protocol (paper §XIII.A) is the typed wire
format human-facing surfaces (web, TUI, IDE plugins) consume. Phase 9
freezes the **core widget set** — the kinds every conformant surface
must render — and the **update operations** (Insert / Update / Delete /
Reorder) that compose the live diff stream.

## Decision

### 1. Eight widget kinds in the Phase-9 freeze

* `Text` — plain text block.
* `Button` — push button (interactive; surface routes the press into
  the kernel as a [`gauss_sag::ApprovalDecision`] when wired).
* `KeyValueTable` — two-column key/value display.
* `Image` — single image / chart (URL or base64).
* `ApprovalPrompt` — renders a [`gauss_sag::ApprovalRequest`] inline
  with Approve / Deny buttons; the surface emits an
  `ApprovalDecision` over the channel.
* `Container` — generic section wrapper.
* `Markdown` — surface-rendered Markdown subset.
* `Custom` — escape hatch for surface adapters; the props are
  free-form JSON.

`WidgetKind` is `#[non_exhaustive]` so new kinds extend it at
semver-minor.

### 2. Four update operations

* `Insert { node, parent }` — adds a node, optionally under a parent.
* `Update { id, props }` — replaces a node's props.
* `Delete { id }` — removes a node + its descendants.
* `Reorder { parent, children }` — reorders an existing parent's
  children list (the new list MUST be a permutation of the current
  one).

The four ops form a complete reconciliation alphabet — any tree
mutation reduces to a sequence of these ops.

### 3. `Canvas` trait + `InMemoryCanvas` baseline

The trait is async because production backends (Phase-10 SurrealDB
live-query bridge, REST gateway, WebSocket pump) all return futures.
`InMemoryCanvas` is the deterministic Phase-9 backend: a
`HashMap<NodeId, CanvasNode>` plus a `tokio::sync::broadcast` channel
for subscribers. The same impl powers the conformance suite + the
`gauss doctor` health surface.

### 4. Subscribers receive the live stream

Surfaces call `Canvas::subscribe()` and receive `CanvasUpdate`s on a
`broadcast::Receiver`. New subscribers MUST request a snapshot via
`Canvas::snapshot()` before processing updates so the surface starts
from a known tree.

## Phase-10 migration

The Phase-10 cluster mode replaces `InMemoryCanvas` with
`SurrealCanvas`, a backend that:

1. Persists nodes in SurrealDB so cluster failover restores the tree
   without an in-process state transfer.
2. Uses SurrealDB live queries (`LIVE SELECT FROM canvas_node ...`) to
   produce `CanvasUpdate`s — eliminating the broadcast channel and
   letting multi-tenant deployments share the same backing store.

The trait surface stays unchanged; deployments swap backends via a
`use` change.

## Consequences

- **Pro:** Surfaces (web, TUI, IDE) implement against a stable wire
  contract; new surface types are additive.
- **Pro:** The reconciliation alphabet is small enough to verify
  against a property-test — the Phase-9 `reorder_rejects_non_permutations`
  test pins the invariant.
- **Pro:** `Subscribe` + `Snapshot` is the same model SurrealDB live
  queries expose, so Phase-10 migration is mechanical.
- **Con:** The Phase-9 backend is in-process only; cluster deployments
  need the Phase-10 `SurrealCanvas` impl.
- **Con:** `Custom` widgets bypass the schema. Surfaces SHOULD refuse
  unknown custom props rather than silently rendering them.

## Alternatives considered

- **JSON Patch (RFC 6902).** More general but loses the typed widget
  schema. Rejected.
- **Yjs / Automerge CRDT.** Production-grade collaborative editing but
  out of Phase-9 scope; revisit at Phase 11 for multi-operator
  sessions.
- **Server-side rendering (HTML).** Couples the protocol to a single
  surface (web). Rejected.
