# ADR-0006 — SurrealDB as the Trinity Memory storage engine

**Status:** Accepted (Phase 1)
**Date:** 2026-05-16

## Context

`SPECS.md` §8 calls for a single append-only event log feeding three derived
indices (FTS keyword, HNSW vector, Merkle/Ed25519 chain) plus graph lineage.
The Phase-0 stub used SQLite + `tantivy` + `hnsw_rs` as three separate
artefacts; that works but pushes a lot of cross-index consistency logic into
the application layer.

SurrealDB is a single Rust embedded-or-remote multi-model engine that ships
**all** of the primitives we need:

- `DEFINE TABLE … SCHEMAFULL` + `RELATE` for the append log and graph lineage.
- `DEFINE ANALYZER … TOKENIZERS class FILTERS lowercase` +
  `DEFINE INDEX … SEARCH ANALYZER … BM25` for the FTS path.
- `DEFINE INDEX … HNSW DIMENSION N TYPE F32 DISTANCE COSINE` for the vector
  path.
- `BEGIN TRANSACTION` / `COMMIT TRANSACTION` for the WAL barrier.
- Embedded modes (`kv-mem`, `kv-surrealkv`, `kv-rocksdb`, `kv-tikv`) that swap
  by feature flag without code changes.

## Decision

**Trinity Memory uses SurrealDB as its sole storage engine.**

- Phase 1 wires `kv-mem` so tests stay fully in-process.
- Phase 6 turns on `kv-surrealkv` (single-node persistent) and `kv-rocksdb`
  (durable) as additive feature flags.
- Phase 10 adds `kv-tikv` for the clustered profile (Theorem T6 scale-out).
- The bootstrap DDL lives in `gauss-memory::schema` and is applied once per
  fresh database. Every primitive the SPECS calls for has a concrete home in
  the DDL (see `crates/gauss-memory/src/schema.rs`).

The capability lattice, taint label, receipt chain, and Differential Turn
Engine remain Rust-side abstractions. SurrealDB is the storage, not the
authority.

## Consequences

- **Pro:** Single engine, single durability model, single observation
  surface. Graph lineage (`RELATE turn → derived_from → turn`) is native
  rather than emulated in application code.
- **Pro:** HNSW and FTS indexes reserved up-front at Phase 1 — Phase 6 only
  needs to start populating `payload_text` / `embedding`, not to schema-migrate.
- **Pro:** Live queries (Phase 9+) give us a free streaming substrate for the
  A2UI Canvas and the SDHE.
- **Con:** Compile cost. SurrealDB pulls a non-trivial dep tree. We mitigate
  by `default-features = false` and only enabling the engine we need per
  profile.
- **Con:** SurrealQL injection is a possibility if we ever string-interpolate
  into queries. All Phase-1 queries bind parameters via `.bind((name, value))`;
  the code-review checklist for `gauss-memory` PRs explicitly forbids
  interpolation.
- **Con:** SurrealDB's `bytes` column type requires `surrealdb::sql::Bytes`
  rather than raw `Vec<u8>`. We wrap at the boundary; the rest of the codebase
  sees normal `Vec<u8>`.

## Alternatives considered

- **SQLite + `tantivy` + `hnsw_rs`** (Phase-0 stub). Works, but every derived
  index needs its own cross-consistency story. Migrating between SQLite and
  Postgres for clustered mode is non-trivial.
- **PostgreSQL + `pgvector` + `pg_trgm`** for a single-engine alternative.
  Adds an external server dependency to every deployment, removes the
  embedded-database story, and `pgvector`'s HNSW is younger than SurrealDB's.
- **Custom embedded storage** layered over `redb` / `sled` / `bonsaidb`. We'd
  be re-implementing FTS analyzers, HNSW, and graph traversal. Not worth it.

## Migration / replacement

If SurrealDB ever has to be replaced, the boundary is one trait
(`gauss_traits::MemoryBackend`) implemented by one module
(`gauss_memory::surreal`). No other crate references SurrealDB directly.
