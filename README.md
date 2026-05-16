# Gauss-Aether

An axiomatic operating system for trustworthy autonomous LLM agents,
implemented in Rust.

> Status: **Phase 0** — workspace scaffolding, capability lattice, three-plane
> scheduler skeleton, and conformance harness. The full kernel, HWCA, sandbox,
> trinity memory, and receipt chain land across Phases 1–11 (see
> [`ROADMAP.md`](./ROADMAP.md)).

## Documents

- [`SPECS.md`](./SPECS.md) — normative engineering specification.
- [`ROADMAP.md`](./ROADMAP.md) — phased development plan, axiom/theorem locks.
- [`docs/adr/`](./docs/adr/) — Architecture Decision Records.

## Quick start

```bash
git clone https://github.com/rismanmattotorang/gauss-aether
cd gauss-aether
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Workspace layout (Phase 0)

| Crate                | Purpose                                                                |
|----------------------|-------------------------------------------------------------------------|
| `gauss-core`         | Shared types: identifiers, actions, observations, taint, `GaussError`. |
| `gauss-kernel`       | Privileged kernel: capability lattice, three-plane token-bucket sched.  |
| `gauss-turn`         | Differential Turn Engine — type-state machine (Ingest → Generate → Commit). |
| `gauss-memory`       | Trinity Memory Substrate (skeleton).                                    |
| `gauss-audit`        | Cryptographic Receipt Chain — SHA-256 head, Ed25519 signing in Phase 5. |
| `gauss-conformance`  | Axiom-by-axiom test harness (A1–A9, T1–T12).                            |

## Design tenets

1. **Axioms before features.** Every subsystem traces back to an axiom (A1–A9)
   or theorem (T1–T12) in the paper. See `SPECS.md` §14.
2. **No `unsafe` in privileged crates.** Workspace lints set
   `unsafe_code = "forbid"`.
3. **Property-tested algebra.** Lattice laws and chain integrity have
   proptest coverage from Phase 0.
4. **Type-state where possible.** The DTE encodes turn-phase ordering in the
   type system, not in runtime branches.
5. **`#[non_exhaustive]` on public enums/structs.** Field/variant additions
   stay semver-minor.

## Quality gates

Phase 0 CI enforces, on every PR:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
  (with `clippy::pedantic` + `clippy::nursery` at warn level)
- `cargo test --workspace`
- `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings`
- `cargo deny check`

Later phases add `cargo fuzz`, conformance suites, and the fifteen-axis
scorecard regression.

## Licence

Dual-licensed under Apache-2.0 or MIT, at the user's option.
