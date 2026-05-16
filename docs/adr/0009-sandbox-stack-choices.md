# ADR-0009 — Composite sandbox stack: wasmi + Landlock + seccompiler + bwrap

**Status:** Accepted (Phase 3)
**Date:** 2026-05-16
**Supersedes parts of:** SPECS §7 (originally named `wasmtime` for L1 and `libseccomp-rs` for L3b)

## Context

SPECS §7 calls for a four-layer composite sandbox (Theorem T10): WASM ∧
Landlock/Seatbelt ∧ namespace+seccomp ∧ TEE. The original spec named
`wasmtime` and `libseccomp-rs` as the L1 and L3b dependencies. Phase 3
implementation has to balance three constraints:

1. **Build time.** wasmtime cold-builds in 5–10 minutes and pulls in
   Cranelift, regalloc2, etc. Every CI matrix lane pays this cost on cache
   miss. For a Phase-3 system whose hot path uses tiny tools, the JIT
   payoff is small.
2. **Pure-Rust preference.** `libseccomp-rs` requires libseccomp C headers
   at build and runtime. `seccompiler` (Firecracker / Cloud Hypervisor) is
   pure Rust and ships its own BPF assembler.
3. **Cross-platform footprint.** Linux-only layers (Landlock, seccomp,
   bwrap) must compile on macOS and Windows as **no-ops** so the rest of
   the workspace stays portable. Feature gates + `cfg(target_os)` gives us
   this for free.

## Decision

The Phase-3 composite ships with these layer implementations:

| Layer    | Phase 3                       | Phase 10 (planned)                |
|----------|-------------------------------|------------------------------------|
| L1 WASM  | `wasmi` 0.46 (pure-Rust)      | `wasmtime` 24+ (JIT, perf gates)   |
| L2 fs    | `landlock` 0.4 (Linux 5.13+)  | unchanged + `seatbelt` (macOS)     |
| L3a ns   | `bwrap` subprocess wrapper    | direct `clone()` + `unshare()`     |
| L3b sec  | `seccompiler` 0.5             | unchanged                          |
| L4 TEE   | (deferred)                    | SEV-SNP / TDX attestation crates   |

Each layer is feature-gated:

- `wasm-wasmi` (default-on) — wasmi backend
- `linux-layers` (default-on) — Landlock + seccompiler deps
- `macos-layers` (default-on) — Seatbelt subprocess wrapper (no extra dep)

The Linux-only crates additionally guard their **modules** with
`#[cfg(target_os = "linux")]` so non-Linux builds skip them entirely. The
`gauss-sandbox` crate therefore compiles cleanly on macOS and Windows; only
Linux gets the OS-level enforcement.

### WASM backend swap (wasmi → wasmtime)

The `SandboxTrait` is identical for both backends, so swapping is a build-
flag flip. The internal differences are:

- `wasmi::Config::consume_fuel(true)` ≅ `wasmtime::Config::consume_fuel(true)`.
- `wasmi::Engine::new(&cfg)` ≅ `wasmtime::Engine::new(&cfg)?`.
- The `instance.get_typed_func::<(), i32>` lookup is identical syntax.

Phase 10 introduces a second feature `wasm-wasmtime`; the two backends are
mutually exclusive at the workspace level. The Phase-10 benchmark gates a
release on the wasmtime profile so the production deploy never ships wasmi.

## Consequences

- **Pro:** Cold-build of `gauss-sandbox` is ~20 s (wasmi) vs ~5–10 min
  (wasmtime). Phase-3 development iteration stays fast.
- **Pro:** Pure-Rust dep tree — `libseccomp` is the only C dep we'd have
  pulled in, and `seccompiler` removes it.
- **Pro:** Non-Linux operators (macOS dev machines, Windows CI) get a
  clean compile without any "Linux only" feature errors.
- **Con:** wasmi is an interpreter — tools that need raw throughput will
  hit it. The Phase-10 wasmtime swap is on the critical path for the
  release benchmarks.
- **Con:** `bwrap` is a subprocess wrapper rather than a direct
  `clone()` + `unshare()` call. Phase 4's HWCA worker boundary moves the
  isolation into the worker subprocess directly; until then the layer's
  job is to fail loudly when bwrap is missing.

## Alternatives considered

- **wasmtime from the start.** Rejected for Phase 3: build cost dominates
  every iteration. Phase 10 takes the cost once, behind a release flag.
- **Just-WASM, no OS layers.** Rejected: composing layers is the whole
  point of T10. Even an imperfect L2/L3 contributes a factor to the
  product bound.
- **External sandbox processes only** (Firecracker microVMs, gVisor). Out
  of scope for in-process tool execution; revisit in v2.

## Migration / replacement

The trait surface is stable; swapping backends is a `Cargo.toml` flag flip.
If a future security review requires libseccomp, the `gauss-sandbox`
`linux-layers` feature can be split into `linux-landlock` and
`linux-libseccomp` to coexist.
