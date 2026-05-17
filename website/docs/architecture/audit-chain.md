---
id: audit-chain
title: Audit chain
sidebar_position: 3
---

# Audit chain

Every inbound request, every turn start, every turn completion writes
into a `ReceiptChain` (from `gauss-audit`) wrapped in
`gaussclaw-agent::AuditTrace`. The chain links each entry's
canonical-JSON payload into a SHA-256 hash chain.

## Three structural properties

1. **WAL-before-effect (Axiom A1).** Surfaces call
   `record_inbound()` *before* admit, dispatch, or any side-effect.
   Even denied turns are auditable.
2. **Tamper-evidence (Theorem T3).** The chain head diverges on any
   byte changed in any past entry. `InclusionWitness` verifies a
   single entry in O(log n).
3. **Plane attribution.** Every entry carries the `Plane`
   (Conversation / Daemon / Approval) — the audit chain doubles as
   the cross-plane fairness witness.

## Entry kinds

```rust
pub enum AuditEntry {
    Inbound(InboundRecord),
    Outbound(OutboundRecord),
    TurnStart(TurnStartRecord),
    TurnComplete(TurnCompleteRecord),
}
```

Bodies are **never** retained — only `blake3_hex(body)`. Replay and
verification reconstruct the chain head from canonical-JSON of these
records, not the original payloads.

## Live head endpoints

| Endpoint | Surface |
|---|---|
| `GET /api/receipt/head` | Web dashboard |
| `GET /v1/audit/head` | SDK surfaces |
| `gc_receipt_head` IPC | Desktop app |
| `/receipt` slash command | TUI |

All return the **same** live chain head — production deployments
share one `AuditTrace` across every surface.

## Hermes baseline

The upstream Hermes writes free-form text into Python's `logging`
module. Unsigned files, no Merkle structure, no integrity guarantee.
GaussClaw's audit chain is **strictly better** by every measure.
