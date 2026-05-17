//! [`EnvGetTool`] — read an environment variable from a caller-supplied allowlist.
//!
//! Cap-gated by `env:read`. The Hermes upstream's `env_get` returns *any*
//! env var the agent process can see — credentials, AWS keys, the lot.
//! GaussClaw bounds reads to an explicit allowlist passed at construction
//! time, so the tool can never surface a secret that the operator hasn't
//! pre-approved.

use std::collections::BTreeSet;

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "env_get"
description = "Read an environment variable from a caller-supplied allowlist."
usage       = "Use to surface non-secret configuration (LANG, TZ, PATH). Args: {name: string}."
caps        = ["env:read"]
taint       = "user"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 4096

[schema]
type = "object"
"#;

/// Allowlist-bounded env-var reader.
pub struct EnvGetTool {
    manifest: ToolManifest,
    allowlist: BTreeSet<String>,
}

impl EnvGetTool {
    /// Build with an empty allowlist (every read fails). Useful when the
    /// operator wants the tool registered for a uniform catalogue but not
    /// actually usable.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        Self::with_allowlist(std::iter::empty::<&str>())
    }

    /// Build with an explicit allowlist of permitted environment variable
    /// names. The allowlist is case-sensitive.
    pub fn with_allowlist<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("env_get".into()))
            .expect("embedded skill compiles");
        Self {
            manifest,
            allowlist: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Read-only access to the allowlist (used by introspection surfaces).
    #[must_use]
    pub fn allowlist(&self) -> impl Iterator<Item = &str> {
        self.allowlist.iter().map(String::as_str)
    }
}

impl Default for EnvGetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for EnvGetTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `name`".into()))?;
        if !self.allowlist.contains(name) {
            return Err(GaussError::Internal(format!(
                "env var `{name}` is not in the allowlist (configure the operator's allowlist to permit it)"
            )));
        }
        let value = std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
        Ok(serde_json::json!({
            "name": name,
            "found": value.is_some(),
            "value": value,
        }))
    }
}

/// Ensure the manifest declares the right cap token. Useful as a sanity
/// check in callers that compose the tool registry by hand.
#[must_use]
pub const fn env_read_cap() -> CapToken {
    CapToken::ENV_READ
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_var_not_on_allowlist() {
        let t = EnvGetTool::with_allowlist(["LANG"]);
        let err = t
            .invoke_raw(serde_json::json!({ "name": "AWS_SECRET_ACCESS_KEY" }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn allowed_var_returns_value() {
        // SAFETY: we set + read a uniquely-named var inside one test.
        // SAFETY: legal in tokio::test (single-thread current_thread runtime).
        std::env::set_var("GAUSSCLAW_TEST_ENV_GET", "hello");
        let t = EnvGetTool::with_allowlist(["GAUSSCLAW_TEST_ENV_GET"]);
        let out = t
            .invoke_raw(serde_json::json!({ "name": "GAUSSCLAW_TEST_ENV_GET" }))
            .await
            .unwrap();
        assert_eq!(out["found"], true);
        assert_eq!(out["value"], "hello");
        std::env::remove_var("GAUSSCLAW_TEST_ENV_GET");
    }

    #[tokio::test]
    async fn allowed_but_unset_returns_found_false() {
        let t = EnvGetTool::with_allowlist(["GAUSSCLAW_DEFINITELY_NOT_SET_FOO"]);
        let out = t
            .invoke_raw(serde_json::json!({ "name": "GAUSSCLAW_DEFINITELY_NOT_SET_FOO" }))
            .await
            .unwrap();
        assert_eq!(out["found"], false);
    }

    #[test]
    fn manifest_declares_env_read_cap() {
        let t = EnvGetTool::new();
        assert_eq!(t.manifest().cap_required, env_read_cap());
    }
}
