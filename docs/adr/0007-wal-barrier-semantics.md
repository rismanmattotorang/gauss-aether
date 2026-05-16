# ADR-0007 — WAL barrier semantics for the Differential Turn Engine

**Status:** Accepted (Phase 2)
**Date:** 2026-05-16

## Context

Axiom A1 (turn idempotency) and Theorem T1 (crash atomicity) require that
external side-effects fire **only after** the audit record is durably written.
SPECS §5.2 names this the WAL barrier. The Phase-2 Differential Turn Engine
is the first crate that actually exercises the barrier; this ADR fixes the
discipline.

## Decision

The Differential Turn Engine implements Algorithm 1 with the following
**structural** barrier (rather than a behavioural / convention-based one):

1. The engine `await`s `MemoryBackend::append(entry)`. The `append` future
   resolves only after the SurrealDB transaction commits.
2. **Only after `append` returns `Ok(_)`** does the engine invoke
   `apply_actions_locally(actions)` (Phase 3+ replaces this with the
   composite sandbox executor).
3. The CAS-protected `chain_head` field on `SurrealMemory` is updated in
   the same transaction that writes the row. A crash between (a) and (b)
   leaves the cache unchanged.

The barrier is *encoded in the engine's source order* — there is no
configuration that can disable it.

## Persistence guarantees per backend

| Backend             | `append` returns when …                                       | Crash recovery |
|---------------------|----------------------------------------------------------------|----------------|
| `kv-mem` (Phase 1)  | the in-process B-tree mutation completes.                      | Lost on crash (acceptable for tests). |
| `kv-surrealkv` (Phase 6) | the surrealkv WAL is `fsync`ed.                            | Replay from durable WAL.              |
| `kv-rocksdb` (Phase 6)   | the RocksDB WAL is `fsync`ed (`WriteOptions::sync(true)`). | Replay from RocksDB WAL.              |
| `kv-tikv` (Phase 10)     | the Raft replication round-trips a quorum.                 | Standard TiKV durability.             |

The conformance suite verifies the engine's source-order discipline on every
backend; the per-backend durability claim is part of the release gates.

## Consequences

- **Pro:** Axiom A1 cannot be bypassed without changing source code in
  `gauss-turn::engine`, which is a Tier-1 crate under ADR-0005's dual-review
  policy.
- **Pro:** The conformance suite's crash-injection test is shallow on
  `kv-mem` (we drop the engine, not the process) but the same test exercises
  the durable backends in CI from Phase 6 onward.
- **Con:** Async cancellation between `append.await` and the side-effect
  invocation could leave us with a durable record and no external effect.
  This is *the* allowed crash mode — A1 explicitly admits the post-recovery
  state being either `s` or `s′`. Recovery replays the record-bound effect
  deterministically (Phase 3 introduces the replayable effect-executor).

## Alternatives considered

- **Two-phase commit** with the external sandbox vote. Adds latency on the
  hot path and only helps for actions whose externality is local. Phase 3
  may reintroduce it for `Subprocess.spawn` once the sandbox executor lands.
- **Behavioural lint** ("don't call `apply_actions_locally` before
  `append`") instead of structural enforcement. Rejected: hard to audit at
  PR-review time, prone to subtle refactor breakage.
