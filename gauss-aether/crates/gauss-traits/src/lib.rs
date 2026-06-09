//! `gauss-traits` — public trait surface.
//!
//! Plugin authors depend on this crate (and `gauss-core`) only; the kernel
//! depends on this crate to consume implementations. The eight traits enumerated
//! by SPECS §11 land here as they are designed.
//!
//! Phase 1 stabilises three: [`Kernel`], [`MemoryBackend`], and [`Sealed`].
//! Future phases add `ProviderTrait`, `ChannelTrait`, `ToolTrait`,
//! `SandboxTrait`, `VoiceTrait`, `ApprovalTrait`, `CanvasTrait`.

use async_trait::async_trait;
use gauss_core::{Action, CapToken, GaussResult, Observation, TaintLabel, ToolId, TurnId};
use serde::{Deserialize, Serialize};

/// Sealed marker — prevents downstream crates from implementing kernel-private
/// traits. Place an unimplemented marker in every sealed trait to force the
/// pattern: `trait Foo: Sealed { ... }`.
pub mod sealed {
    /// The seal. Implement only inside the workspace.
    pub trait Sealed {}
}

pub use sealed::Sealed;

/// Kernel surface (Phase 1).
///
/// Implementations are not user-pluggable — there is one privileged kernel per
/// process. The trait references [`CapToken`] directly (which lives in
/// `gauss-core`), so this crate has no dependency edge on `gauss-kernel`.
pub trait Kernel: Send + Sync {
    /// Look up the agent's current capability grant.
    fn current_grant(&self) -> CapToken;

    /// Joint capability/taint admission. Returns the refusal reason on denial
    /// (encoded as the appropriate [`gauss_core::GaussError`] variant).
    ///
    /// The function is total: every action that would otherwise execute MUST
    /// pass through `admit` first.
    ///
    /// # Errors
    /// Returns [`gauss_core::GaussError::Denied`] when either the capability
    /// or taint bound is not satisfied.
    fn admit(&self, required: CapToken, taint: TaintLabel) -> GaussResult<()>;
}

/// Memory monoid surface (Phase 1, extended in Phase 6).
///
/// Real implementations live in `gauss-memory` (in-memory + `SurrealDB`). The
/// trait is `async_trait` because `SurrealDB` calls are async.
///
/// Phase 6 adds three recall methods — `fts_search`, `vector_search`, and
/// `hybrid_recall`. All three carry default implementations that return an
/// empty result so older backends keep compiling unchanged; the
/// `SurrealMemory` backend overrides them with real BM25 + HNSW + union
/// dedup logic.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Append a record. The implementation MUST ensure durability before
    /// returning (Axiom A1).
    async fn append(&self, entry: AppendEntry) -> GaussResult<AppendAck>;

    /// Current chain head (Phase 5 onward — Phase 1 returns the genesis or a
    /// stub hash).
    async fn chain_head(&self) -> GaussResult<ChainHeadSnapshot>;

    /// Number of records currently in the log.
    async fn len(&self) -> GaussResult<u64>;

    /// True iff `len() == 0`. Default implementation queries `len`.
    async fn is_empty(&self) -> GaussResult<bool> {
        Ok(self.len().await? == 0)
    }

    /// Phase-6 BM25 keyword recall over `payload_text` (default: empty).
    ///
    /// `limit` is the maximum number of hits to return. Backends that have
    /// not enabled FTS return `Ok(vec![])`.
    ///
    /// # Errors
    /// Backend-specific I/O errors are wrapped in
    /// [`gauss_core::GaussError::Io`].
    async fn fts_search(&self, _query: &str, _limit: usize) -> GaussResult<Vec<RecallHit>> {
        Ok(Vec::new())
    }

    /// Phase-6 HNSW k-nearest-neighbour recall over `embedding` (default:
    /// empty).
    ///
    /// `query` is a normalised vector of `DIMENSION` floats (default 384;
    /// pinned at deployment time). `k` is the result-set size.
    ///
    /// # Errors
    /// Backend-specific I/O errors are wrapped in
    /// [`gauss_core::GaussError::Io`].
    async fn vector_search(&self, _query: &[f32], _k: usize) -> GaussResult<Vec<RecallHit>> {
        Ok(Vec::new())
    }

    /// Phase-6 hybrid recall `ρ_hyb = ρ_fts ∪ ρ_vec` deduplicated by
    /// `turn_id`, with weighted score-merge per `query.alpha`. Default impl
    /// dispatches to `fts_search` + `vector_search` and applies the merge
    /// in-process; backends MAY override for a single `SurrealDB` round-trip.
    ///
    /// # Errors
    /// Propagates the underlying recall errors.
    async fn hybrid_recall(&self, query: HybridQuery) -> GaussResult<Vec<RecallHit>> {
        let fts = if let Some(text) = query.text.as_deref() {
            self.fts_search(text, query.k).await?
        } else {
            Vec::new()
        };
        let vec = if let Some(embedding) = query.embedding.as_deref() {
            self.vector_search(embedding, query.k).await?
        } else {
            Vec::new()
        };
        Ok(merge_hybrid(fts, vec, query.alpha, query.k))
    }
}

/// Merge two recall result sets by union-then-score.
///
/// `ρ_fts` and `ρ_vec` are combined with weight `alpha` on the FTS score and
/// `1 - alpha` on the vector score. Hits found only in one source are credited
/// with `0.0` for the missing channel. The result is truncated to `limit`
/// hits, sorted by descending merged score. Exposed as a free function so
/// `MemoryBackend::hybrid_recall` overrides can call it.
#[must_use]
pub fn merge_hybrid(
    fts: Vec<RecallHit>,
    vec: Vec<RecallHit>,
    alpha: f32,
    limit: usize,
) -> Vec<RecallHit> {
    use std::collections::HashMap;
    let alpha = alpha.clamp(0.0, 1.0);
    let beta = 1.0 - alpha;
    let cap = fts.len().saturating_add(vec.len());
    let mut by_turn: HashMap<TurnId, RecallHit> = HashMap::with_capacity(cap);
    for mut h in fts {
        h.score *= alpha;
        h.source = RecallSource::Fts;
        by_turn.insert(h.turn_id, h);
    }
    for h in vec {
        by_turn
            .entry(h.turn_id)
            .and_modify(|existing| {
                existing.score = h.score.mul_add(beta, existing.score);
                existing.source = RecallSource::Hybrid;
            })
            .or_insert_with(|| {
                let mut h = h;
                h.score *= beta;
                h.source = RecallSource::Vector;
                h
            });
    }
    let mut merged: Vec<RecallHit> = by_turn.into_values().collect();
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    merged.truncate(limit);
    merged
}

/// Audit record being appended.
///
/// Phase 6 adds two optional, FTS- and HNSW-targeted fields. Backends that do
/// not have those indices ignore them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AppendEntry {
    /// Identifier of the originating turn.
    pub turn_id: TurnId,
    /// Opaque record payload — Phase 2 will replace with a typed delta.
    pub payload: Vec<u8>,
    /// Taint of the record's underlying observation(s).
    pub taint: TaintLabel,
    /// Optional text payload materialised for BM25 keyword recall (Phase 6).
    pub payload_text: Option<String>,
    /// Optional sentence-embedding vector materialised for HNSW vector
    /// recall (Phase 6). Length MUST match the backend's HNSW dimension
    /// (`384` for the `SurrealMemory` default).
    pub embedding: Option<Vec<f32>>,
}

impl AppendEntry {
    /// Construct an append entry without recall enrichment (Phase-1
    /// compatibility).
    #[must_use]
    pub const fn new(turn_id: TurnId, payload: Vec<u8>, taint: TaintLabel) -> Self {
        Self {
            turn_id,
            payload,
            taint,
            payload_text: None,
            embedding: None,
        }
    }

    /// Attach a text payload for FTS recall (Phase 6).
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.payload_text = Some(text.into());
        self
    }

    /// Attach an embedding for vector recall (Phase 6).
    #[must_use]
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }
}

/// One hit from a recall query (Phase 6).
///
/// `payload` is the original bytes the turn appended; `payload_text` is the
/// enriched text the backend used for FTS scoring (handy for highlighting).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecallHit {
    /// Turn that produced this record.
    pub turn_id: TurnId,
    /// 0-based position of this record in the chain.
    pub seq: u64,
    /// Backend-specific relevance score. BM25 is unbounded; cosine distance
    /// is mapped into `1.0 - d ∈ [0, 1]` so higher = closer. Hybrid merges
    /// weight the two channels with the [`HybridQuery::alpha`] parameter.
    pub score: f32,
    /// Channel that produced this hit (FTS, vector, or the hybrid merge).
    pub source: RecallSource,
    /// Original opaque payload.
    pub payload: Vec<u8>,
    /// Materialised text (`payload_text` if it was set on append).
    pub payload_text: Option<String>,
    /// Taint of the underlying observation.
    pub taint: TaintLabel,
}

impl RecallHit {
    /// Construct a hit. Provided because the struct is `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(
        turn_id: TurnId,
        seq: u64,
        score: f32,
        source: RecallSource,
        payload: Vec<u8>,
        payload_text: Option<String>,
        taint: TaintLabel,
    ) -> Self {
        Self {
            turn_id,
            seq,
            score,
            source,
            payload,
            payload_text,
            taint,
        }
    }
}

/// Which channel a [`RecallHit`] came from.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RecallSource {
    /// BM25 keyword index.
    Fts,
    /// HNSW vector index.
    Vector,
    /// Union of the two channels.
    Hybrid,
}

/// Query against [`MemoryBackend::hybrid_recall`].
///
/// At least one of `text` / `embedding` must be present; if both are present
/// the merged score is `alpha · fts_score + (1 - alpha) · vector_score`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HybridQuery {
    /// Optional BM25 keyword query.
    pub text: Option<String>,
    /// Optional vector-space query (HNSW-indexed by the backend).
    pub embedding: Option<Vec<f32>>,
    /// Result-set size. The backend MAY return fewer hits.
    pub k: usize,
    /// Score-merge weight. `1.0` = FTS-only ranking; `0.0` = vector-only.
    /// `0.5` (the default) gives equal weight.
    pub alpha: f32,
}

impl Default for HybridQuery {
    fn default() -> Self {
        Self {
            text: None,
            embedding: None,
            k: 16,
            alpha: 0.5,
        }
    }
}

impl HybridQuery {
    /// Build a hybrid query.
    #[must_use]
    pub const fn new(
        text: Option<String>,
        embedding: Option<Vec<f32>>,
        k: usize,
        alpha: f32,
    ) -> Self {
        Self {
            text,
            embedding,
            k,
            alpha,
        }
    }
}

/// Acknowledgement of a successful append.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AppendAck {
    /// Position of the appended record in the log (0-based).
    pub index: u64,
    /// Chain head after the append.
    pub head: ChainHeadSnapshot,
}

impl AppendAck {
    /// Construct an acknowledgement. Provided because the struct is
    /// `#[non_exhaustive]` and cannot be struct-literal'd from outside this
    /// crate.
    #[must_use]
    pub const fn new(index: u64, head: ChainHeadSnapshot) -> Self {
        Self { index, head }
    }
}

/// Snapshot of the chain head returned by [`MemoryBackend::chain_head`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChainHeadSnapshot {
    /// SHA-256 digest of the chain at the time of the snapshot.
    pub digest: [u8; 32],
    /// Number of records the digest covers.
    pub length: u64,
}

impl ChainHeadSnapshot {
    /// Genesis snapshot (zero digest, length 0).
    pub const GENESIS: Self = Self {
        digest: [0u8; 32],
        length: 0,
    };

    /// Construct a snapshot. Provided because the struct is `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(digest: [u8; 32], length: u64) -> Self {
        Self { digest, length }
    }
}

// =================================================================
// Sandbox surface (Phase 3) — composite WASM ∧ Landlock ∧ ns/seccomp ∧ TEE.
// =================================================================

/// One layer in the composite sandbox stack. Each layer is independent so the
/// product bound of Theorem T10 (`Pr[compromise] ≤ Π pᵢ + p_T`) holds under
/// conditional orthogonality.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLayer {
    /// L1 — WebAssembly (wasmi in Phase 3; wasmtime in Phase 10).
    Wasm,
    /// L2 — Linux Landlock (5.13+) or macOS Seatbelt.
    Landlock,
    /// L3a — Linux namespaces (via bubblewrap).
    Namespace,
    /// L3b — Linux seccomp filter.
    Seccomp,
    /// L4 — TEE attestation (Phase 10).
    Tee,
}

/// Composite sandbox class derived from a capability — paper §IX.B.
///
/// The class is a bit-set so layers can be combined ergonomically. Higher
/// capability depth → larger required set.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SandboxClass(u8);

impl SandboxClass {
    /// Empty class (no layers). Only the `NoOpSandbox` in tests accepts
    /// this; production composite sandboxes will refuse.
    pub const NONE: Self = Self(0);
    /// `Wasm` only.
    pub const L1: Self = Self(0b0_0001);
    /// `Wasm` + `Landlock` / Seatbelt.
    pub const L2: Self = Self(0b0_0011);
    /// L1+L2 + namespace + seccomp.
    pub const L3: Self = Self(0b0_1111);
    /// L1+L2+L3 + TEE attestation (Phase 10).
    pub const L4: Self = Self(0b1_1111);

    /// Construct from a raw bitmask.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Return the raw bitmask.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// True iff this class requires `layer`.
    #[must_use]
    pub const fn requires(self, layer: SandboxLayer) -> bool {
        let bit: u8 = match layer {
            SandboxLayer::Wasm => 1 << 0,
            SandboxLayer::Landlock => 1 << 1,
            SandboxLayer::Namespace => 1 << 2,
            SandboxLayer::Seccomp => 1 << 3,
            SandboxLayer::Tee => 1 << 4,
        };
        (self.0 & bit) == bit
    }

    /// Bitwise union — combine two classes (largest stack wins per layer).
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Compute the **minimum** required sandbox class for a given capability
/// (SPECS §7.1). Higher capability depth → stricter stack.
///
/// * Read-only filesystem reads / canvas renders → L1 only.
/// * Scoped filesystem writes + network GET → L1 + Landlock.
/// * Subprocess spawn / network POST → L1 + L2 + L3 (ns + seccomp).
/// * Crypto signing → L4 (TEE; Phase 10) — Phase 3 returns L3 with a
///   software-only marker on the receipt.
#[must_use]
pub const fn min_sandbox_for(cap: CapToken) -> SandboxClass {
    // Walk highest-privilege bits first.
    if cap.contains(CapToken::CRYPTO_SIGN) {
        return SandboxClass::L4;
    }
    if cap.contains(CapToken::SUBPROCESS_SPAWN) || cap.contains(CapToken::NETWORK_POST) {
        return SandboxClass::L3;
    }
    if cap.contains(CapToken::FILESYSTEM_WRITE)
        || cap.contains(CapToken::NETWORK_GET)
        || cap.contains(CapToken::CANVAS_EMBED)
        || cap.contains(CapToken::CANVAS_FILE_WRITE)
    {
        return SandboxClass::L2;
    }
    SandboxClass::L1
}

/// Input to the sandbox executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SandboxRequest {
    /// Tool identifier (mostly for tracing).
    pub tool: ToolId,
    /// Capability the parent kernel has already admitted for this action.
    pub cap: CapToken,
    /// Tool-supplied arguments (opaque to the sandbox).
    pub args: serde_json::Value,
    /// Bytes piped to the tool's stdin (or its WASM `args.stdin` equivalent).
    pub stdin: Vec<u8>,
}

impl SandboxRequest {
    /// Construct a request.
    #[must_use]
    pub const fn new(tool: ToolId, cap: CapToken, args: serde_json::Value, stdin: Vec<u8>) -> Self {
        Self {
            tool,
            cap,
            args,
            stdin,
        }
    }
}

/// Outcome of a sandboxed invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SandboxOutcome {
    /// Bytes the tool wrote to stdout / its WASM out-channel.
    pub stdout: Vec<u8>,
    /// Layers the sandbox stack actually invoked. Used by the conformance
    /// suite to verify the cap → class mapping.
    pub layers_invoked: Vec<SandboxLayer>,
    /// Exit code; 0 on success.
    pub exit_code: i32,
}

impl SandboxOutcome {
    /// Convenience constructor for stub / success outcomes.
    #[must_use]
    pub const fn ok(stdout: Vec<u8>, layers_invoked: Vec<SandboxLayer>) -> Self {
        Self {
            stdout,
            layers_invoked,
            exit_code: 0,
        }
    }

    /// Full constructor; required because the struct is `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(stdout: Vec<u8>, layers_invoked: Vec<SandboxLayer>, exit_code: i32) -> Self {
        Self {
            stdout,
            layers_invoked,
            exit_code,
        }
    }
}

/// Sandbox trait — Phase 3.
///
/// An implementor MUST refuse the request if its layers do not cover the
/// class returned by [`min_sandbox_for`] for the requested capability. The
/// composite executor in `gauss-sandbox` enforces this; individual layers
/// only contribute their own confinement.
#[async_trait]
pub trait SandboxTrait: Send + Sync + core::fmt::Debug {
    /// Layers this implementor activates for the given capability. Inspected
    /// by the kernel to compare against [`min_sandbox_for`].
    fn class(&self, cap: CapToken) -> SandboxClass;

    /// Execute `request` inside the sandbox. The future MUST NOT resolve
    /// until the executor has either committed the tool's effect or rejected
    /// it.
    ///
    /// # Errors
    /// * [`gauss_core::GaussError::Denied`] — sandbox refused (cap or class
    ///   mismatch).
    /// * [`gauss_core::GaussError::Io`] — I/O / runtime failure.
    async fn exec(&self, request: SandboxRequest) -> GaussResult<SandboxOutcome>;
}

// =================================================================
// HWCA + Tool surface (Phase 4) — paper §X, A7, T9.
// =================================================================

/// Output schema published by a tool's manifest.
///
/// The HWCA schema gate validates every raw tool return value against this
/// schema before the validated payload is allowed to cross the
/// worker→parent boundary (Axiom A7 / Theorem T9).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OutputSchema {
    /// Inline JSON Schema 2020-12 document.
    pub json_schema: serde_json::Value,
    /// Per-field length caps; checked before structural validation runs so
    /// pathological inputs are short-circuited.
    pub max_string_len: usize,
}

impl OutputSchema {
    /// Default cap on free-text field length (paper §X.B). 4096 bytes
    /// matches a `body ≤ 4096` style manifest.
    pub const DEFAULT_MAX_STRING_LEN: usize = 4096;

    /// Build an output schema from a JSON Schema document and per-field caps.
    #[must_use]
    pub const fn new(json_schema: serde_json::Value, max_string_len: usize) -> Self {
        Self {
            json_schema,
            max_string_len,
        }
    }

    /// Build with the default 4096-byte cap.
    #[must_use]
    pub const fn with_default_caps(json_schema: serde_json::Value) -> Self {
        Self::new(json_schema, Self::DEFAULT_MAX_STRING_LEN)
    }
}

/// Schema-gate guards on free-text fields. The instruction-substring filter
/// is the headline guard for paper §X.B's adversarial-input mitigation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SchemaGuards {
    /// If true, raw string field values containing instruction-like
    /// substrings (e.g. "ignore previous instructions", "system:") are
    /// rejected at the schema gate before crossing the worker boundary.
    pub no_instruction_substrings: bool,
}

impl Default for SchemaGuards {
    fn default() -> Self {
        Self {
            no_instruction_substrings: true,
        }
    }
}

impl SchemaGuards {
    /// Build with the headline guard enabled.
    #[must_use]
    pub const fn strict() -> Self {
        Self {
            no_instruction_substrings: true,
        }
    }

    /// Build with no guards. **Tests / debug only.**
    #[must_use]
    pub const fn permissive() -> Self {
        Self {
            no_instruction_substrings: false,
        }
    }
}

/// A tool's manifest — exported by every tool implementation. Phase 4 reads
/// this at worker spawn to drive the schema gate and the cap admission
/// decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolManifest {
    /// Tool identifier.
    pub id: ToolId,
    /// Capability the tool requires before the kernel admits it.
    pub cap_required: CapToken,
    /// True iff the tool's external effect is reversible.
    pub reversible: bool,
    /// Output schema for the value returned across the worker boundary.
    pub output_schema: OutputSchema,
    /// Per-tool schema-gate guards.
    pub guards: SchemaGuards,
}

impl ToolManifest {
    /// Construct a manifest. Required because the struct is `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(
        id: ToolId,
        cap_required: CapToken,
        reversible: bool,
        output_schema: OutputSchema,
        guards: SchemaGuards,
    ) -> Self {
        Self {
            id,
            cap_required,
            reversible,
            output_schema,
            guards,
        }
    }
}

/// Tool surface — Phase 4. The HWCA worker invokes the tool's `invoke_raw`
/// and pipes the result through the schema gate; only the validated
/// `ValidatedValue` crosses back to the parent context (Axiom A7).
#[async_trait]
pub trait ToolTrait: Send + Sync {
    /// Tool manifest. The HWCA reads this at spawn time.
    fn manifest(&self) -> &ToolManifest;

    /// Invoke the tool, producing an *unvalidated* raw JSON return value.
    /// The HWCA schema gate refines this into a [`ValidatedValue`] before
    /// the parent context sees anything.
    ///
    /// # Errors
    /// Tool-side failures propagate verbatim.
    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value>;
}

/// Schema-validated value crossing the worker→parent boundary.
///
/// The HWCA boundary discipline is: **only the data described by this
/// struct survives the worker drop**. The raw tool output, the worker's
/// intermediate reasoning, and any retrieved content are dropped at turn
/// boundary (paper §X.A).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ValidatedValue {
    /// JSON payload that conformed to the tool's `OutputSchema`.
    pub value: serde_json::Value,
    /// Taint after the join: incoming taint ∨ `Web` (the default tool-
    /// output taint until Phase 6 wires the tool's declared source).
    pub taint: TaintLabel,
}

impl ValidatedValue {
    /// Construct. Required because the struct is `#[non_exhaustive]`.
    #[must_use]
    pub const fn new(value: serde_json::Value, taint: TaintLabel) -> Self {
        Self { value, taint }
    }
}

/// LLM policy trait `π` (Phase 2).
///
/// A `Provider` consumes an observation history (Phase 2 sees only the most
/// recent observation; Phase 6 will add full prefix retrieval) and emits a
/// sequence of actions. Real providers (Anthropic, `OpenAI`, Google) ship in
/// Phase 8 once the polyhedral-equivalence verifier is in place.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Generate one turn's worth of actions for the given observation.
    ///
    /// # Errors
    /// Implementations return [`GaussError::Io`] on transport failures and
    /// [`GaussError::Internal`] on policy / parser failures.
    ///
    /// [`GaussError::Io`]: gauss_core::GaussError::Io
    /// [`GaussError::Internal`]: gauss_core::GaussError::Internal
    async fn generate(&self, obs: &Observation) -> GaussResult<Vec<Action>>;
}
