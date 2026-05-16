# ADR-0002 — Async runtime = Tokio (multi-thread)

**Status:** Accepted (Phase 0)
**Date:** 2026-05-16

## Context

Gauss-Aether is I/O-bound at every layer above the kernel: provider streaming,
channel adapters, sandbox subprocess management, TSA anchoring, A2UI canvas
sockets. The kernel itself is non-async (it handles capability resolution and
scheduling decisions synchronously), but the surrounding runtime is async-heavy.

Rust's ecosystem offers two production-grade async runtimes (Tokio and
async-std) plus a number of niche runtimes (smol, glommio). Mixing them in
one process is a well-documented source of subtle deadlocks.

The four reference platforms studied — Hermes (Python), OpenFang (Tokio),
OpenClaw (Node), ZeroClaw (Tokio) — all converge on Tokio when implementing
in Rust.

## Decision

The workspace uses **Tokio** (multi-thread runtime in production,
current-thread in unit tests) as the **sole** async runtime. We forbid
direct dependencies on `async-std`, `smol`, or `glommio` in workspace crates.

CPU-bound workloads (HNSW indexing, Myers diffs, SHA-256 chains over batches)
go through `tokio::task::spawn_blocking` or are dispatched to a `rayon` pool
explicitly — not through async tasks.

## Consequences

- Single-runtime invariant: `tokio::select!`, `JoinSet`, and the cancel-safety
  guarantees we get from Tokio cancellation tokens are usable everywhere.
- We pay Tokio's startup cost (~ms) and binary size; both are acceptable.
- Provider streaming uses Tokio's `Stream` + `futures-util` combinators.
- Phase 0 keeps the dep surface narrow — Tokio is added in Phase 1 where the
  first async surface (channel adapter trait) lands.

## Alternatives considered

- **async-std.** Rejected: maintenance has slowed; ecosystem inertia favours
  Tokio.
- **Single-threaded runtime as default.** Rejected: the three-plane scheduler
  needs CPU parallelism on multi-tenant deployments.
- **No async runtime; pure thread pool.** Rejected: provider/channel SDKs are
  almost universally async-first.
