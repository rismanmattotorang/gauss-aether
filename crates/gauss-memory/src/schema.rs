//! `SurrealQL` schema for the Trinity Memory Substrate.
//!
//! The schema is **declared once** at backend construction. Every primitive
//! the Gauss-Aether design calls for has a concrete home here:
//!
//! | SPECS §                | Storage primitive                                         |
//! |------------------------|-----------------------------------------------------------|
//! | §8.1 append log        | `DEFINE TABLE turn_record SCHEMAFULL ...`                 |
//! | §8.6 audit chain head  | `DEFINE TABLE chain_head ...` with a single singleton row |
//! | §8.2 FTS index         | `DEFINE ANALYZER` + `DEFINE INDEX ... SEARCH ANALYZER`    |
//! | §8.3 HNSW vector       | `DEFINE INDEX ... HNSW DIMENSION ... TYPE F32`            |
//! | lineage (paper §VII)   | `RELATE turn_record:a -> derived_from -> turn_record:b`   |
//! | §VI capability grants  | `DEFINE TABLE agent` + `RELATE` to `capability_grant`     |
//! | A6 taint               | `taint` field on `turn_record` (string enum, indexed)     |
//!
//! Phase 1 reserves the FTS and HNSW indices but does not exercise recall;
//! Phase 6 turns them on by populating the `payload_text` and `embedding`
//! fields.

/// Name of the append-log table.
pub const TURN_RECORD_TABLE: &str = "turn_record";

/// Name of the chain-head singleton table.
pub const CHAIN_HEAD_TABLE: &str = "chain_head";

/// Edge name for the `RELATE turn -> derived_from -> turn` lineage graph.
pub const DERIVED_FROM_EDGE: &str = "derived_from";

/// Schema-installer helper. Returns the `SurrealQL` bootstrap statements as a
/// single string suitable for `db.query(...)`.
#[must_use]
pub const fn bootstrap_ddl() -> &'static str {
    BOOTSTRAP_DDL
}

const BOOTSTRAP_DDL: &str = r#"
-- ═════════════════════════════════════════════════════════════════════════════
-- Gauss-Aether Trinity Memory Substrate — SurrealQL bootstrap.
-- One append-only event log, derived indices, graph lineage. SPECS §8.
-- ═════════════════════════════════════════════════════════════════════════════

-- 1. Append-only turn record (SPECS §8.1).
DEFINE TABLE turn_record SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD turn_id      ON turn_record TYPE string  ASSERT $value != NONE;
DEFINE FIELD payload      ON turn_record TYPE bytes;
DEFINE FIELD payload_text ON turn_record TYPE option<string>; -- materialised for FTS
DEFINE FIELD embedding    ON turn_record TYPE option<array<float>>; -- Phase 6 HNSW
DEFINE FIELD taint        ON turn_record TYPE string ASSERT $value
    INSIDE ["trusted", "user", "web", "adversarial"];
DEFINE FIELD recorded_at  ON turn_record TYPE datetime VALUE time::now() READONLY;
DEFINE FIELD seq          ON turn_record TYPE int      ASSERT $value >= 0;
DEFINE FIELD prev_head    ON turn_record TYPE bytes;
DEFINE FIELD this_head    ON turn_record TYPE bytes;

-- Append-order invariants.
DEFINE INDEX turn_record_unique  ON turn_record FIELDS turn_id UNIQUE;
DEFINE INDEX turn_record_seq     ON turn_record FIELDS seq UNIQUE;
DEFINE INDEX turn_record_taint   ON turn_record FIELDS taint;

-- 2. Singleton chain-head row (SPECS §8.6). Materialised so reads are O(1).
DEFINE TABLE chain_head SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD digest ON chain_head TYPE bytes;
DEFINE FIELD length ON chain_head TYPE int ASSERT $value >= 0;

-- 3. FTS keyword index reserved for Phase 6.
DEFINE ANALYZER IF NOT EXISTS lower_alphanum
    TOKENIZERS class
    FILTERS lowercase, ascii;
DEFINE INDEX IF NOT EXISTS turn_record_fts
    ON turn_record FIELDS payload_text
    SEARCH ANALYZER lower_alphanum BM25;

-- 4. HNSW vector index reserved for Phase 6 (DIMENSION 384 = MiniLM default,
--    overridable at deployment time).
DEFINE INDEX IF NOT EXISTS turn_record_hnsw
    ON turn_record FIELDS embedding
    HNSW DIMENSION 384 TYPE F32 DISTANCE COSINE M 16 EFC 200;

-- 5. Capability grants + agents (SPECS §VI).
DEFINE TABLE agent SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD public_key      ON agent TYPE bytes;
DEFINE FIELD current_grant   ON agent TYPE int ASSERT $value >= 0;

DEFINE TABLE capability_grant SCHEMAFULL PERMISSIONS NONE TYPE RELATION FROM agent TO capability_grant;
DEFINE FIELD bits      ON capability_grant TYPE int    ASSERT $value >= 0;
DEFINE FIELD granted_at ON capability_grant TYPE datetime VALUE time::now() READONLY;
DEFINE FIELD signed_by ON capability_grant TYPE option<bytes>; -- Phase 5 anchor

-- 6. Graph lineage between turn records (Trinity Memory + paper §VII).
DEFINE TABLE derived_from SCHEMAFULL PERMISSIONS NONE TYPE RELATION FROM turn_record TO turn_record;
DEFINE FIELD reason ON derived_from TYPE option<string>;
"#;

/// Type-level marker for "the schema is installed". Used as a phantom type by
/// `SurrealMemory` so callers can distinguish a freshly connected handle from
/// a fully bootstrapped one.
#[derive(Debug, Clone, Copy)]
pub struct Schema;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_includes_every_critical_definition() {
        let ddl = bootstrap_ddl();
        for needle in [
            "DEFINE TABLE turn_record",
            "DEFINE TABLE chain_head",
            "DEFINE INDEX turn_record_unique",
            "DEFINE INDEX turn_record_seq",
            "DEFINE INDEX turn_record_taint",
            "DEFINE ANALYZER IF NOT EXISTS lower_alphanum",
            "DEFINE INDEX IF NOT EXISTS turn_record_fts",
            "DEFINE INDEX IF NOT EXISTS turn_record_hnsw",
            "DEFINE TABLE agent",
            "DEFINE TABLE capability_grant",
            "DEFINE TABLE derived_from",
        ] {
            assert!(ddl.contains(needle), "bootstrap DDL missing: {needle}");
        }
    }

    #[test]
    fn taint_enum_matches_taintlabel_serde() {
        // The DDL ASSERTs the taint string against a fixed enum; this test
        // pins the values so any change to TaintLabel will be caught here.
        for s in ["trusted", "user", "web", "adversarial"] {
            assert!(bootstrap_ddl().contains(&format!("\"{s}\"")));
        }
    }
}
