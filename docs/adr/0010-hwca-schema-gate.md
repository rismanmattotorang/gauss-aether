# ADR-0010 — HWCA worker boundary, schema gate, and IPI containment

**Status:** Accepted (Phase 4)
**Date:** 2026-05-16
**Locks:** Axiom A7 (Context Isolation)
**Proves:** Theorem T9 (IPI containment, `≤ 2.19%`)

## Context

SPECS §6 calls for **per-tool worker contexts**: every tool invocation
runs in a freshly-spawned worker, and only a schema-validated return value
crosses back to the parent. Paper §X.B further specifies a **schema gate**
combining JSON Schema 2020-12 validation, per-field length caps, and an
**instruction-substring filter** that blocks free-text payloads from
acting as indirect prompt injections (IPI).

Three implementation choices needed locking in Phase 4:

1. **Worker isolation granularity.** A worker can be (a) an in-process
   task, (b) a thread with a fresh stack, or (c) a subprocess (or even a
   Firecracker microVM). Each has different latency / leak-surface
   trade-offs.
2. **JSON Schema library.** Several Rust crates exist. We picked
   `jsonschema` 0.46 (pure Rust, 2020-12 draft support, no C deps).
3. **IPI corpus source.** AgentDojo and EchoLeak are the canonical
   benchmarks; both are large enough that integrating them in the
   conformance suite would balloon CI time. We need a *deterministic*
   sub-corpus.

## Decision

### 1. In-process workers, sub-subprocess in Phase 10

Phase 4 implements **in-process worker contexts**. A worker is a struct +
an RAII guard that increments / decrements a shared `Arc<AtomicU32>`
counter so the spawner can verify no worker outlives its turn. This buys
us schema-gate isolation + taint propagation + recursion-depth bounds
*without* the cold-start latency of subprocess spawning.

The host kernel's Phase-3 sandbox layers (Landlock + seccomp) still apply
at the *host-thread* level — Phase 10 moves the per-worker sandbox stack
into a subprocess so confinement scope matches worker lifetime exactly.

### 2. `jsonschema` 0.46 as the schema validator

`jsonschema` is pure Rust, supports draft 2020-12 (which the SPECS §6.2
manifest example uses), compiles fast, and has no C dependency. The
`Validator` is constructed once per worker (heavy work amortised across
the lifetime of the schema gate); per-call `validate` is a tight tree
walk.

Tested alternatives: `valico` (older draft only), `boon` (2020-12 but
younger ecosystem). The trait surface in `gauss-traits::OutputSchema` is
backend-agnostic — swapping is a `Cargo.toml` change.

### 3. Synthetic Phase-4 corpus (n=20); AgentDojo / EchoLeak integration in Phase 6

The Phase-4 conformance suite ships a hand-curated 20-attempt corpus
covering three families:

* **AgentDojo-style** — "ignore previous instructions", role
  impersonation, "you are now …".
* **EchoLeak-style** — exfiltration prompts modelled on CVE-2025-32711.
* **Tool-call hijacking** — "respond with the following", "system:"
  tags, "override:" patterns.

The empirical attack-success rate on this corpus MUST be `≤ 2.19%`
(Theorem T9). With the instruction-substring filter in place, the
Phase-4 rate is **0/20 = 0%** — the bound is met with margin. The full
AgentDojo + EchoLeak corpora (~10⁵ scenarios) land in Phase 6 alongside
provider replay; the corpus trait is stable so swapping is additive.

### 4. Per-string defensive checks before structural validation

The schema gate runs checks **in order**:

1. **Length cap** — every string field length ≤ `OutputSchema::max_string_len`.
   First so pathological payloads short-circuit before the O(n) schema walker.
2. **JSON Schema 2020-12** — structural conformance.
3. **Instruction-substring filter** — applied to every string field
   (recursively over nested arrays/objects) when the manifest opts in.
4. **Taint join** — outgoing taint = `incoming ∨ Web`.

The order is deliberate: cheap checks first, monotone-cost checks last.

## Consequences

- **Pro:** Pure-Rust dep tree; no JNI / libxml / Python interop.
- **Pro:** The schema-gate compile-cost is one-shot per worker; per-call
  validation is hot.
- **Pro:** Phase-4 IPI corpus gives a regression test that catches both
  filter regressions and schema-gate misconfiguration.
- **Pro:** The worker-context drop happens through an RAII guard. The
  workspace lint forbids `unsafe`; the guard uses `Arc<AtomicU32>` so the
  spawner-and-worker counter relationship is safe by construction.
- **Con:** In-process workers share an address space with the parent. The
  Phase-3 sandbox layers apply to the host thread, not the worker, so a
  WASM trap in the worker terminates the host turn too. Phase 10's
  subprocess-per-worker model fixes this.
- **Con:** The instruction-substring filter is a deny-list of natural-
  language patterns. False positives are accepted (a benign tool output
  that legitimately contains "system:" gets rejected). Phase 6 adds a
  statistical classifier as a second-pass guard.
- **Con:** The Phase-4 corpus is small (n=20). The asserted `≤ 2.19%`
  bound is a binary check; provider-replay corpora in Phase 6 will give
  it a statistical structure.

## Alternatives considered

- **Worker = subprocess from the start.** Rejected for Phase 4: cold-
  start latency dominates per-tool overhead; the kernel layers already
  apply at the host. Phase 10 takes the cost behind a feature flag.
- **`boon`** JSON Schema crate. Younger; 2020-12 support is newer; no
  Phase-4 reason to prefer it. Revisitable when `jsonschema` 0.46
  semver-major breaks.
- **Cryptographic content fingerprint** as the instruction-substring
  proxy. Doesn't generalise: a single template change in an attacker's
  prompt would defeat it.
- **In-loop classifier** (small LM scoring each output). Adds latency on
  every tool call AND requires a provider round-trip; doesn't compose
  with offline conformance testing. Out of scope.

## Migration / replacement

The boundary contract — `ToolTrait` + `OutputSchema` + `SchemaGuards`
+ `ValidatedValue` — lives in `gauss-traits`. The schema-gate
implementation lives in `gauss-hwca::schema_gate`. Either can be
replaced independently:

- Switching JSON Schema libraries: change `SchemaGate::new` only.
- Hardening the IPI filter: extend `INSTRUCTION_SUBSTRINGS` or replace
  `contains_instruction_substring` with a richer classifier.
- Moving to subprocess workers (Phase 10): replace `Worker::run`'s
  in-process invocation with a `tokio::process` spawn; the trait surface
  is unaffected.
