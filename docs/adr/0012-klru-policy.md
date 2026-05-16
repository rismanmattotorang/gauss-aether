# ADR-0012 — K-LRU prefix-tree cache + checkpoint cadence

**Status:** Accepted (Phase 6)
**Date:** 2026-05-16
**Locks:** Axiom A5 (memory monoid laws)
**Proves:** Theorem T12 (delta-encoded warm switch)

## Context

The Trinity Memory substrate maintains a fast prefix-tree cache of recently-
seen turn prefixes keyed by `SessionId` (paper §VIII.C). The cache is the
operational backbone of Theorem T12 — a "warm" context switch must complete
in `≤ 10 ms p95` even on a 1000-turn chain — and is the only practical way
to avoid replaying the entire chain on every retrieval.

Three implementation choices needed locking:

1. **Cadence cleavage.** Every turn cannot be a checkpoint (memory cost),
   and every turn cannot be a pure delta (replay cost on cold-start
   unbounded). Some cadence `K` divides the chain into spans of
   `K` deltas anchored by a checkpoint.
2. **Eviction policy.** LRU vs LFU vs ARC vs 2Q. The cache holds an
   *evolving* prefix tree, so a "set-and-forget" LFU would starve fresh
   conversations; we want recency.
3. **Concurrency model.** Multi-tenant session traffic means the cache will
   be hit from many tokio tasks. Coarse `Mutex` vs `RwLock` vs sharded
   `DashMap`.

## Decision

### 1. `K = 128` checkpoint cadence

Paper §VIII.C calibrates `K` against the warm-switch bound:

* A `K`-step replay is the worst case. At ~50 µs per delta application on
  the Phase-6 Myers implementation, `K = 128` keeps the worst case under
  ~7 ms — comfortably inside the `10 ms p95` Theorem T12 bound.
* Each checkpoint is a full materialised state — typically 1–4 KB for a
  conversation transcript. `K = 128` spans roughly 8 turns of conversation
  per checkpoint, which is a fair trade between memory cost and rewind
  depth.

The cadence is configurable per-tenant. `PrefixTree::new(K, capacity)`
allows ops to dial `K` down for transcripts that need shallow rewinds (e.g.
the approval surface in Phase 7) and up for telemetry feeds that don't.

`PrefixTree::default()` returns `K = 128`, `capacity = 512` — the
`DEFAULT_K` / `DEFAULT_CAPACITY` constants.

### 2. LRU with explicit `VecDeque` access order

The cache is small enough (~512 entries) that a `VecDeque<Path>` access
order is faster than a tree-based ordering. The pattern is the textbook
LRU:

* `get(path)` hits → `order.retain(|p| p != path); order.push_front(path)`.
* Eviction → `order.pop_back()` until `nodes.len() < capacity`.

The data structure is a `HashMap<Path, Node<S>>` for `O(1)` lookup plus the
`VecDeque<Path>` for the order. The retain pass is `O(N)` in the cache size
but only fires on a hit; misses are `O(1)`. For `capacity = 512` the
constant factor is negligible (a hit costs ~1 µs).

We rejected ARC (Adaptive Replacement Cache) because the cache holds
*structured* nodes (checkpoints + deltas) where the cost of evicting a
checkpoint is much higher than evicting a delta — the policy needs to be
aware of node structure, which ARC isn't. Phase 10 may revisit this; for
Phase 6 the simpler LRU keeps the proof + benchmark surface tight.

### 3. `parking_lot::Mutex` over the cache map

The cache is hit-or-miss on every recall request. Holding a `tokio::sync::
Mutex` across await points is overkill — no async work happens under the
lock. `parking_lot::Mutex` is faster (~2x) than `std::sync::Mutex` and
already a workspace dep.

We rejected sharded `DashMap` because the LRU access order is global —
sharding would require a separate per-shard `VecDeque` and a meta-LRU
between shards, doubling the complexity for cache sizes that don't yet
need it.

### 4. Path is content-addressed (a vector of `u64` hashes)

The cache path is **not** a `(SessionId, TurnId)` pair — it's a
content-addressed vector. The first element is the session root (an 8-byte
prefix of the SHA-256 of the empty state); subsequent elements are the
8-byte hash of the canonicalised action set of each turn. Two sessions
that produce byte-equal turn sequences share a node — the "free recall"
property Trinity Memory inherits from its underlying merge graph
(paper §VIII.A).

This is also why we don't store the full ULID — for many sessions the
prefix-tree shape is more compact than the per-session linear store, and
the hash collision rate at 64 bits is negligible at session scale (`10^9
sessions × 10^3 turns = 10^{12}` paths vs `2^{64} ≈ 1.8 × 10^{19}`
addressable nodes; birthday-collision at ~`2^{32}` so we are five orders
of magnitude inside the safe range).

## Consequences

- **Pro:** Pure-Rust, no `unsafe`, sub-microsecond hot path, deterministic
  worst-case rewind bounded by `K`.
- **Pro:** The cache structure (checkpoint + delta path) is a faithful
  in-memory mirror of the on-disk SurrealDB log, so the warm path and the
  cold path share the same conceptual model.
- **Pro:** Tests are offline and deterministic; the conformance suite
  asserts `eviction_keeps_warm_nodes` and `warm_cache_lookup_below_10_ms`
  inline.
- **Con:** The cache is per-process. Phase 10's cluster mode will need a
  distributed cache (Redis / `kv-tikv` LRU) layered on top.
- **Con:** The `Patch` payload in `Node::Delta` is `myers::Patch` over
  `String` tokens only — diffing JSON ADT directly will need a future
  `myers::diff<T: Eq>` instantiation over typed AST nodes. The trait
  boundary is in place.
- **Con:** `K = 128` is calibrated against the Phase-6 Myers performance.
  If a Phase-10 hardening profile swaps in a slower diff or a heavier
  state representation, `K` will need to drop. The constant is exposed at
  the API surface for exactly this kind of re-tuning.

## Alternatives considered

- **`K = 1` (every turn is a checkpoint).** Storage cost prohibitive at
  scale; rejected.
- **`K = ∞` (linear log replay).** Theoretically correct but blows the
  Theorem T12 warm-switch bound on any chain longer than ~50 turns.
  Rejected.
- **Treap-keyed cache.** Cute but slower in the small-`N` regime where the
  cache spends 99 % of its time. Rejected.
- **Bloom-filter sharding.** Only meaningful at `> 10^5` nodes; we'll
  revisit when the in-cluster cache lands in Phase 10.
- **Persistent-data-structure cache (e.g. `im::HashMap`).** Avoids the
  retain pass but doubles the memory cost. Rejected for the same Phase-10
  / Phase-6 cutover argument.

## Migration / replacement

The cache lives in `gauss-memory::klru` with the trait surface
`PrefixTree<S>` + `Node<S>`. The Phase-10 distributed cache will:

1. Add a `gauss-memory::klru::DistributedPrefixTree` next to `PrefixTree`
   that implements the same `get`/`insert_*`/`stats` surface.
2. Optionally feature-gate the in-process `PrefixTree` behind
   `default = ["klru-inprocess"]` so the distributed build doesn't carry
   both implementations.
3. Use the same `myers::Patch` delta format on the wire so a cluster node
   restart that re-loads from the distributed cache costs nothing extra.

The conformance suite already exercises the cache through `PrefixTree::
get`; swapping in `DistributedPrefixTree` is a `use` change.
