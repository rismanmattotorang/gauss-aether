-- Gauss-Agent0 KnowledgeGraph schema (paper Appendix A, Listing 5).
-- Targets SurrealDB 2.x: HNSW vector indexes via DEFINE INDEX ... HNSW,
-- typed graph edges via DEFINE TABLE ... TYPE RELATION, k-NN via <|k,ef|>.

DEFINE NAMESPACE gauss; USE NS gauss;
DEFINE DATABASE core; USE DB core;

-- ============ node tables ============
DEFINE TABLE concept SCHEMAFULL;
DEFINE FIELD name      ON concept TYPE string;
DEFINE FIELD embedding ON concept TYPE array<float>;
DEFINE FIELD created   ON concept TYPE datetime VALUE time::now();
DEFINE INDEX concept_name ON concept FIELDS name UNIQUE;
DEFINE INDEX concept_vec  ON concept FIELDS embedding
  HNSW DIMENSION 1024 DIST COSINE TYPE F32 EFC 200 M 16;

DEFINE TABLE claim SCHEMAFULL;
DEFINE FIELD content    ON claim TYPE string;
DEFINE FIELD embedding  ON claim TYPE array<float>;
DEFINE FIELD confidence ON claim TYPE float ASSERT $value >= 0 AND $value <= 1;
DEFINE FIELD status     ON claim TYPE string DEFAULT 'candidate'
  ASSERT $value INSIDE ['candidate','verified','falsified'];
DEFINE FIELD tier       ON claim TYPE int;        -- verifier tier 1..3
DEFINE FIELD cycle      ON claim TYPE int;        -- admission cycle t
DEFINE FIELD provenance ON claim TYPE object;     -- models, premises, judge
DEFINE INDEX claim_vec  ON claim FIELDS embedding
  HNSW DIMENSION 1024 DIST COSINE TYPE F32 EFC 200 M 16;

DEFINE TABLE skill SCHEMAFULL;
DEFINE FIELD name      ON skill TYPE string;
DEFINE FIELD signature ON skill TYPE string;
DEFINE FIELD code      ON skill TYPE string;      -- sandboxed source
DEFINE FIELD lang      ON skill TYPE string;
DEFINE FIELD embedding ON skill TYPE array<float>;
DEFINE FIELD pass_rate ON skill TYPE float;       -- empirical p_hat
DEFINE FIELD ci_low    ON skill TYPE float;       -- PAC bound, eq. (11)
DEFINE FIELD m_tests   ON skill TYPE int;
DEFINE FIELD cycle     ON skill TYPE int;
DEFINE INDEX skill_vec ON skill FIELDS embedding
  HNSW DIMENSION 1024 DIST COSINE TYPE F32 EFC 200 M 16;

DEFINE TABLE model SCHEMAFULL;
DEFINE FIELD slug      ON model TYPE string;      -- OpenRouter slug
DEFINE FIELD provider  ON model TYPE string;
DEFINE FIELD price_in  ON model TYPE float;       -- USD / 1e6 tokens
DEFINE FIELD price_out ON model TYPE float;
DEFINE FIELD latency   ON model TYPE float;       -- rolling p50, ms
DEFINE INDEX model_slug ON model FIELDS slug UNIQUE;

DEFINE TABLE task SCHEMAFULL;
DEFINE FIELD family ON task TYPE string;
DEFINE FIELD spec   ON task TYPE object;
DEFINE FIELD probe  ON task TYPE bool DEFAULT false;  -- eval probe?

DEFINE TABLE snapshot SCHEMAFULL;
DEFINE FIELD cycle ON snapshot TYPE int;
DEFINE FIELD label ON snapshot TYPE string;
DEFINE FIELD taken ON snapshot TYPE datetime VALUE time::now();

-- ============ relation (edge) tables ============
DEFINE TABLE about        TYPE RELATION FROM claim   TO concept;
DEFINE TABLE relates      TYPE RELATION FROM claim   TO concept;
DEFINE TABLE supports     TYPE RELATION FROM concept TO claim;
DEFINE TABLE contradicts  TYPE RELATION FROM claim   TO claim;
DEFINE TABLE derived_from TYPE RELATION FROM claim   TO claim;
DEFINE TABLE evidences    TYPE RELATION FROM claim   TO skill;
DEFINE TABLE requires     TYPE RELATION FROM skill   TO concept;
DEFINE TABLE certified_on TYPE RELATION FROM skill   TO task;
DEFINE TABLE involves     TYPE RELATION FROM task    TO concept;
DEFINE TABLE emitted_by   TYPE RELATION FROM claim   TO model;

-- ============ recurring queries ============
-- vector path (Alg. 2): k-NN over verified claims
-- SELECT id, content, confidence FROM claim
-- WHERE status = 'verified' AND embedding <|8,40|> $qvec;

-- graph path seed expansion: claims about linked concepts (2 hops)
-- SELECT <-about<-claim AS premises FROM concept WHERE name INSIDE $entities;

-- synergy count for Theorem 2(b): multi-family derivations
-- SELECT count() FROM claim
-- WHERE status = 'verified' AND array::len(provenance.model_families) >= 2
-- GROUP ALL;

-- rollback: revert everything admitted after checkpoint $c
-- UPDATE claim SET status = 'candidate' WHERE cycle > $c;
-- DELETE derived_from WHERE out.cycle > $c OR in.cycle > $c;
