//! `gaussclaw-skill` — Skill Manifest TOML parser.
//!
//! Phase 3 Task 1 of `GAUSSCLAW_ROADMAP.md`. The upstream Hermes
//! `@tool` decorator records a Python function in an in-memory
//! registry — the metadata (caps, taint, schema, cost) lives nowhere
//! verifiable. GaussClaw replaces that with an explicit TOML
//! **Skill Manifest** parsed at startup, validated at build time,
//! and compiled into a [`gauss_traits::ToolManifest`] that drives
//! the HWCA worker spawner and the kernel admit gate.
//!
//! ## Structural superiorities over the Hermes `@tool` decorator
//!
//! 1. **Cap declaration is data.** The manifest's `caps = [...]` list
//!    is parsed into a [`gauss_core::CapToken`]; the kernel admit
//!    gate is checked against this token *before* the tool runs.
//!    Hermes tools have no cap declaration — the function call
//!    inherits the full process credentials.
//!
//! 2. **Output schema is mandatory.** Every Skill Manifest carries a
//!    JSON Schema 2020-12 for the value that crosses the worker→
//!    parent boundary. The HWCA schema gate rejects raw output that
//!    fails the schema — closing the IPI vector. Hermes hands raw
//!    JSON back to the model verbatim.
//!
//! 3. **Taint is declared.** Each manifest declares the default
//!    output taint (typically `Web` for network tools, `User` for
//!    filesystem, `Trusted` for pure compute). The HWCA joins this
//!    with the incoming taint, propagating monotonically upward.
//!
//! 4. **Cost is auditable.** Each manifest declares `tokens_per_call`,
//!    `wallclock_ms`, `dollars_per_call` — the Daemon-plane scheduler
//!    consumes these for budget admission.
//!
//! 5. **Reversibility is explicit.** `reversible = true|false`. The
//!    SAG approval-plane gate (Phase 7) uses this to decide whether
//!    a human must approve.
//!
//! ## Schema (canonical `skill.toml`)
//!
//! ```toml
//! name        = "web_fetch"
//! description = "Fetch a URL via HTTP GET."
//! usage       = "Use when retrieving content not in training data."
//!
//! caps        = ["network:http_get"]
//! taint       = "web"            # default output taint
//! reversible  = true             # SAG approval-plane hint
//! persistent  = false            # HWCA worker reuse hint
//!
//! [cost]
//! tokens_per_call  = 800
//! wallclock_ms     = 1500
//! dollars_per_call = 0.0
//!
//! [guards]
//! no_instruction_substrings = true   # close IPI at the schema gate
//! max_string_len            = 65536
//!
//! [schema]
//! type = "object"
//! properties = { url = { type = "string" }, body = { type = "string" } }
//! required   = ["url", "body"]
//! ```

#![allow(clippy::doc_markdown)]

use gauss_core::{CapToken, TaintLabel, ToolId};
use gauss_traits::{OutputSchema, SchemaGuards, ToolManifest};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── errors ────────────────────────────────────────────────────────────────

/// Skill manifest errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SkillError {
    /// TOML parse failure.
    #[error("toml parse: {0}")]
    Toml(#[from] toml::de::Error),
    /// A capability string did not map to any known [`CapToken`].
    #[error("unknown capability: {0}")]
    UnknownCap(String),
    /// A taint string did not match a [`TaintLabel`] variant.
    #[error("unknown taint label: {0}")]
    UnknownTaint(String),
    /// The manifest's JSON Schema is not a valid object.
    #[error("invalid schema: {0}")]
    InvalidSchema(String),
}

/// Convenience result alias.
pub type SkillResult<T> = Result<T, SkillError>;

// ─── manifest ──────────────────────────────────────────────────────────────

/// One Skill Manifest. Parsed from TOML; compiled into a runtime
/// [`gauss_traits::ToolManifest`] via [`Self::compile`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SkillManifest {
    /// Tool name, lowercase, underscore-separated.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// When-to-use guidance (shown to the model in the prompt).
    #[serde(default)]
    pub usage: String,
    /// Capability strings (e.g. `"network:http_get"`).
    pub caps: Vec<String>,
    /// Default output taint label.
    #[serde(default = "default_taint")]
    pub taint: String,
    /// Whether the tool's external effect is reversible.
    #[serde(default)]
    pub reversible: bool,
    /// Whether the HWCA may reuse a worker across calls (Phase 3 slice 5).
    #[serde(default)]
    pub persistent: bool,
    /// Cost telemetry.
    #[serde(default)]
    pub cost: CostHints,
    /// Schema-gate guards.
    #[serde(default)]
    pub guards: GuardConfig,
    /// Output JSON schema (JSON Schema 2020-12 inline).
    #[serde(default)]
    pub schema: serde_json::Value,
}

/// Cost hints attached to a Skill Manifest.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct CostHints {
    /// Estimated tokens consumed (prompt + completion).
    pub tokens_per_call: u32,
    /// Estimated wall-clock latency (milliseconds).
    pub wallclock_ms: u32,
    /// Estimated dollar cost.
    pub dollars_per_call: f64,
}

impl Default for CostHints {
    fn default() -> Self {
        Self {
            tokens_per_call: 0,
            wallclock_ms: 0,
            dollars_per_call: 0.0,
        }
    }
}

/// Schema-gate guards.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct GuardConfig {
    /// Reject output strings that contain instruction-like substrings
    /// (closes the IPI vector — paper §X.A).
    pub no_instruction_substrings: bool,
    /// Maximum length of any string field in the validated output.
    pub max_string_len: usize,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            no_instruction_substrings: true,
            max_string_len: 65_536,
        }
    }
}

fn default_taint() -> String {
    "web".into()
}

impl SkillManifest {
    /// Parse a TOML document.
    ///
    /// # Errors
    /// Returns [`SkillError::Toml`] on parse failure.
    pub fn from_toml(s: &str) -> SkillResult<Self> {
        toml::from_str(s).map_err(SkillError::from)
    }

    /// Resolve `caps` into a single bit-OR [`CapToken`] — the kernel
    /// must grant every listed cap for the tool to admit.
    ///
    /// # Errors
    /// Returns [`SkillError::UnknownCap`] when a cap string is not in
    /// the canonical map.
    pub fn cap_required(&self) -> SkillResult<CapToken> {
        let mut acc: u64 = 0;
        for cap in &self.caps {
            acc |= parse_cap(cap)?.bits();
        }
        Ok(CapToken::from_bits(acc))
    }

    /// Resolve the declared output taint.
    ///
    /// # Errors
    /// Returns [`SkillError::UnknownTaint`] when the taint string is
    /// not in `{trusted, user, web, adversarial}`.
    pub fn output_taint(&self) -> SkillResult<TaintLabel> {
        parse_taint(&self.taint)
    }

    /// Compile into a [`gauss_traits::ToolManifest`].
    ///
    /// Used by the HWCA worker spawner at runtime; the schema gate
    /// reads `output_schema` to validate the tool's return value, and
    /// the kernel reads `cap_required` to admit.
    ///
    /// # Errors
    /// Returns [`SkillError::UnknownCap`] or
    /// [`SkillError::InvalidSchema`] on any failure.
    pub fn compile(&self, id: ToolId) -> SkillResult<ToolManifest> {
        let cap_required = self.cap_required()?;
        let json_schema = if self.schema.is_object() {
            self.schema.clone()
        } else if self.schema.is_null() {
            // Default permissive schema for tools that haven't declared
            // one. The schema-gate still applies the substring filter
            // because `guards.no_instruction_substrings` defaults true.
            serde_json::json!({ "type": "object" })
        } else {
            return Err(SkillError::InvalidSchema(
                "top-level schema must be an object".into(),
            ));
        };
        let guards = if self.guards.no_instruction_substrings {
            SchemaGuards::strict()
        } else {
            SchemaGuards::permissive()
        };
        Ok(ToolManifest::new(
            id,
            cap_required,
            self.reversible,
            OutputSchema::new(json_schema, self.guards.max_string_len),
            guards,
        ))
    }
}

// ─── parsers ───────────────────────────────────────────────────────────────

/// Map a manifest capability string to a [`CapToken`].
///
/// Canonical names (left-hand side) plus shorthand aliases. Multiple
/// caps in a single manifest are bit-ORed by [`SkillManifest::cap_required`].
///
/// | string | cap |
/// |---|---|
/// | `fs:read`, `filesystem:read` | `FILESYSTEM_READ` |
/// | `fs:write`, `filesystem:write` | `FILESYSTEM_WRITE` |
/// | `net:get`, `network:http_get` | `NETWORK_GET` |
/// | `net:post`, `network:http_post` | `NETWORK_POST` |
/// | `subprocess:spawn` | `SUBPROCESS_SPAWN` |
/// | `crypto:sign` | `CRYPTO_SIGN` |
/// | `canvas:render` | `CANVAS_RENDER` |
/// | `canvas:embed` | `CANVAS_EMBED` |
///
/// # Errors
/// Returns [`SkillError::UnknownCap`] on any other string.
pub fn parse_cap(cap: &str) -> SkillResult<CapToken> {
    match cap {
        "fs:read" | "filesystem:read" => Ok(CapToken::FILESYSTEM_READ),
        "fs:write" | "filesystem:write" => Ok(CapToken::FILESYSTEM_WRITE),
        "net:get" | "network:http_get" => Ok(CapToken::NETWORK_GET),
        "net:post" | "network:http_post" => Ok(CapToken::NETWORK_POST),
        "subprocess:spawn" => Ok(CapToken::SUBPROCESS_SPAWN),
        "crypto:sign" => Ok(CapToken::CRYPTO_SIGN),
        "canvas:render" => Ok(CapToken::CANVAS_RENDER),
        "canvas:embed" => Ok(CapToken::CANVAS_EMBED),
        "env:read" => Ok(CapToken::ENV_READ),
        "memory:read" => Ok(CapToken::MEMORY_READ),
        "approval:ask" => Ok(CapToken::APPROVAL_ASK),
        other => Err(SkillError::UnknownCap(other.into())),
    }
}

/// Map a manifest taint string to a [`TaintLabel`].
///
/// # Errors
/// Returns [`SkillError::UnknownTaint`] when the string is not in
/// `{trusted, user, web, adversarial}`.
pub fn parse_taint(t: &str) -> SkillResult<TaintLabel> {
    match t {
        "trusted" => Ok(TaintLabel::Trusted),
        "user" => Ok(TaintLabel::User),
        "web" => Ok(TaintLabel::Web),
        "adversarial" => Ok(TaintLabel::Adversarial),
        other => Err(SkillError::UnknownTaint(other.into())),
    }
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name        = "web_fetch"
description = "Fetch a URL via HTTP GET."
usage       = "Use when retrieving content not in training data."

caps        = ["network:http_get"]
taint       = "web"
reversible  = true
persistent  = false

[cost]
tokens_per_call  = 800
wallclock_ms     = 1500
dollars_per_call = 0.0

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

    #[test]
    fn round_trip_parse() {
        let m = SkillManifest::from_toml(SAMPLE).expect("parse");
        assert_eq!(m.name, "web_fetch");
        assert!(m.reversible);
        assert!(!m.persistent);
        assert_eq!(m.taint, "web");
        assert_eq!(m.caps, vec!["network:http_get".to_string()]);
        assert_eq!(m.cost.tokens_per_call, 800);
        assert!(m.guards.no_instruction_substrings);
    }

    #[test]
    fn cap_required_oring() {
        let m = SkillManifest {
            name: "two".into(),
            description: String::new(),
            usage: String::new(),
            caps: vec!["fs:read".into(), "net:get".into()],
            taint: "user".into(),
            reversible: false,
            persistent: false,
            cost: CostHints::default(),
            guards: GuardConfig::default(),
            schema: serde_json::Value::Null,
        };
        let cap = m.cap_required().unwrap();
        let expected =
            CapToken::from_bits(CapToken::FILESYSTEM_READ.bits() | CapToken::NETWORK_GET.bits());
        assert_eq!(cap.bits(), expected.bits());
    }

    #[test]
    fn unknown_cap_rejected() {
        let m = SkillManifest::from_toml(
            "name = \"bad\"\ndescription = \"\"\ncaps = [\"banana\"]\ntaint = \"user\"\n",
        )
        .unwrap();
        let err = m.cap_required().unwrap_err();
        assert!(matches!(err, SkillError::UnknownCap(_)));
    }

    #[test]
    fn unknown_taint_rejected() {
        let m = SkillManifest::from_toml(
            "name = \"bad\"\ndescription = \"\"\ncaps = []\ntaint = \"chartreuse\"\n",
        )
        .unwrap();
        let err = m.output_taint().unwrap_err();
        assert!(matches!(err, SkillError::UnknownTaint(_)));
    }

    #[test]
    fn unknown_top_level_key_rejected() {
        let r = SkillManifest::from_toml(
            "name = \"bad\"\ndescription = \"\"\ncaps = []\ntaint = \"user\"\nrogue = 42\n",
        );
        assert!(r.is_err(), "deny_unknown_fields should fire");
    }

    #[test]
    fn compile_to_tool_manifest_uses_id_and_caps() {
        let m = SkillManifest::from_toml(SAMPLE).unwrap();
        let id = ToolId("web_fetch".into());
        let tm = m.compile(id.clone()).expect("compile");
        assert_eq!(tm.id, id);
        assert_eq!(tm.cap_required.bits(), CapToken::NETWORK_GET.bits());
        assert!(tm.reversible);
        assert!(tm.guards.no_instruction_substrings);
    }

    #[test]
    fn compile_supplies_default_schema_when_omitted() {
        let m = SkillManifest::from_toml(
            "name = \"e\"\ndescription = \"\"\ncaps = []\ntaint = \"user\"\n",
        )
        .unwrap();
        let tm = m.compile(ToolId("echo".into())).expect("compile");
        assert!(tm.output_schema.json_schema.is_object());
    }

    #[test]
    fn output_taint_maps_every_label() {
        for (s, expected) in [
            ("trusted", TaintLabel::Trusted),
            ("user", TaintLabel::User),
            ("web", TaintLabel::Web),
            ("adversarial", TaintLabel::Adversarial),
        ] {
            assert_eq!(parse_taint(s).unwrap(), expected);
        }
    }

    #[test]
    fn cap_aliases_resolve_identically() {
        assert_eq!(
            parse_cap("fs:read").unwrap().bits(),
            parse_cap("filesystem:read").unwrap().bits()
        );
        assert_eq!(
            parse_cap("net:get").unwrap().bits(),
            parse_cap("network:http_get").unwrap().bits()
        );
    }
}
