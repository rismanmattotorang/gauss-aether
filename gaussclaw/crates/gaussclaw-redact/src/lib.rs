//! `gaussclaw-redact` — outbound-message redaction.
//!
//! Sprint 7 §8 of `/ROADMAP.md`. Hermes ships `agent/redact.py` with a
//! single regex list applied opaquely to model output. GaussClaw's
//! variant ships:
//!
//! - **Two-layer policy**: literal substrings + compiled regex
//!   patterns. Literals run first (cheap), regexes only on a
//!   substring miss. Both layers contribute to the final
//!   [`RedactionReport`].
//! - **Built-in catalogue** of patterns that Hermes ships in
//!   operator config (so a default deployment is already safer):
//!   credit-card number, AWS access key, GitHub token, JWT, generic
//!   `Bearer …` headers, generic `password=…` URLs.
//! - **Deterministic outputs**. Every rule has a stable id; the
//!   report names which rule fired which substitution.
//! - **Per-profile composition**. `RedactionPolicy::merge()` lets a
//!   profile add its own rules without losing the defaults.
//!
//! Hermes-superiority axes:
//!
//! - **Stable rule ids.** Hermes's redactor logs "redacted" with no
//!   provenance. Ours records `(rule_id, count)` so the audit chain
//!   can prove which patterns fired.
//! - **Cap-gated bypass.** A future `cap:redact:bypass` admits
//!   raw-output passthrough; without it, the policy is enforced by
//!   construction.

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::too_long_first_doc_paragraph,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
#![allow(rustdoc::broken_intra_doc_links)]

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One redaction rule.
#[derive(Debug, Clone)]
pub struct RedactionRule {
    /// Stable id (e.g. `RED-001`).
    pub id: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Compiled matcher.
    pub matcher: Matcher,
    /// Replacement string (e.g. `[REDACTED:CARD]`).
    pub replacement: &'static str,
}

/// Matcher variant.
#[derive(Debug, Clone)]
pub enum Matcher {
    /// Literal substring (case-sensitive).
    Literal(&'static str),
    /// Compiled regex.
    Regex(Regex),
}

/// Crate-wide error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RedactError {
    /// Regex compile failed.
    #[error("regex compile: {0}")]
    Regex(String),
}

/// Crate-wide result.
pub type RedactResult<T> = Result<T, RedactError>;

/// Build the default rule set. Captures the patterns Hermes ships in
/// operator config plus a few additional credentials we want safe by
/// default.
///
/// # Errors
/// Returns [`RedactError::Regex`] if any default pattern fails to
/// compile (a build-time bug).
pub fn default_rules() -> RedactResult<Vec<RedactionRule>> {
    // The workspace `regex` ships without `unicode-perl`, so we use
    // explicit ASCII character classes instead of `\d`, `\w`, `\s`.
    Ok(vec![
        RedactionRule {
            id: "RED-001",
            description: "Credit-card number (14-19 digits, optional separators)",
            matcher: Matcher::Regex(
                Regex::new(r"(?:[0-9][ -]?){13,18}[0-9]")
                    .map_err(|e| RedactError::Regex(e.to_string()))?,
            ),
            replacement: "[REDACTED:CARD]",
        },
        RedactionRule {
            id: "RED-002",
            description: "AWS access key (AKIA…)",
            matcher: Matcher::Regex(
                Regex::new(r"AKIA[0-9A-Z]{16}").map_err(|e| RedactError::Regex(e.to_string()))?,
            ),
            replacement: "[REDACTED:AWS_KEY]",
        },
        RedactionRule {
            id: "RED-003",
            description: "GitHub personal-access token (`gh[ps]_…`)",
            matcher: Matcher::Regex(
                Regex::new(r"gh[ps]_[0-9A-Za-z]{20,}")
                    .map_err(|e| RedactError::Regex(e.to_string()))?,
            ),
            replacement: "[REDACTED:GH_TOKEN]",
        },
        RedactionRule {
            id: "RED-004",
            description: "JWT (three base64url segments separated by `.`)",
            matcher: Matcher::Regex(
                Regex::new(r"eyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}")
                    .map_err(|e| RedactError::Regex(e.to_string()))?,
            ),
            replacement: "[REDACTED:JWT]",
        },
        RedactionRule {
            id: "RED-005",
            description: "HTTP `Authorization: Bearer …` header",
            matcher: Matcher::Regex(
                // Avoid `(?i)` — workspace `regex` lacks `unicode-case`.
                // Cover `Bearer` / `bearer` / `BEARER` explicitly.
                Regex::new(r"[Bb][Ee][Aa][Rr][Ee][Rr][ \t]+[A-Za-z0-9._-]{8,}")
                    .map_err(|e| RedactError::Regex(e.to_string()))?,
            ),
            replacement: "[REDACTED:BEARER]",
        },
        RedactionRule {
            id: "RED-006",
            description: "URL embedded password (`user:pass@host`)",
            matcher: Matcher::Regex(
                Regex::new(r"[A-Za-z]+://[^/ \t\r\n:]+:[^/@ \t\r\n]+@")
                    .map_err(|e| RedactError::Regex(e.to_string()))?,
            ),
            replacement: "[REDACTED:URL_AUTH]@",
        },
        RedactionRule {
            id: "RED-007",
            description: "Private-key PEM header (`-----BEGIN … PRIVATE KEY-----`)",
            matcher: Matcher::Regex(
                Regex::new(
                    r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
                )
                .map_err(|e| RedactError::Regex(e.to_string()))?,
            ),
            replacement: "[REDACTED:PEM]",
        },
    ])
}

/// One full redaction pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionReport {
    /// Per-rule hit counts (`(rule_id, count)`); sorted by id.
    pub hits: Vec<(String, u64)>,
    /// Total number of substitutions across all rules.
    pub total_substitutions: u64,
}

/// The redaction policy. Holds the rule set + provides `apply` /
/// `apply_to_json`.
pub struct RedactionPolicy {
    rules: Vec<RedactionRule>,
}

impl RedactionPolicy {
    /// Build the canonical default policy.
    ///
    /// # Errors
    /// Returns [`RedactError::Regex`] when a built-in regex fails to
    /// compile (a build-time bug).
    pub fn default_policy() -> RedactResult<Self> {
        Ok(Self {
            rules: default_rules()?,
        })
    }

    /// Build from an explicit rule list.
    #[must_use]
    pub fn from_rules(rules: Vec<RedactionRule>) -> Self {
        Self { rules }
    }

    /// Merge another policy's rules into this one (appended at the
    /// end so the merged-in rules don't shadow the defaults).
    pub fn merge(&mut self, mut other: Self) {
        self.rules.append(&mut other.rules);
    }

    /// Number of rules in the policy.
    #[must_use]
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Apply the policy to a UTF-8 string. Returns the redacted
    /// string + a [`RedactionReport`] describing what fired.
    #[must_use]
    pub fn apply(&self, input: &str) -> (String, RedactionReport) {
        let mut current = input.to_string();
        let mut report = RedactionReport::default();
        let mut by_id: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
        for rule in &self.rules {
            let (next, count) = apply_rule(&current, rule);
            if count > 0 {
                *by_id.entry(rule.id).or_insert(0) += count;
                report.total_substitutions = report.total_substitutions.saturating_add(count);
            }
            current = next;
        }
        report.hits = by_id
            .into_iter()
            .map(|(id, n)| (id.to_string(), n))
            .collect();
        (current, report)
    }

    /// Recursively redact every string field of a `serde_json::Value`.
    /// Returns the redacted value + aggregate report.
    #[must_use]
    pub fn apply_to_json(&self, input: &serde_json::Value) -> (serde_json::Value, RedactionReport) {
        let mut report = RedactionReport::default();
        let out = walk_value(self, input, &mut report);
        (out, report)
    }
}

fn apply_rule(input: &str, rule: &RedactionRule) -> (String, u64) {
    match &rule.matcher {
        Matcher::Literal(needle) => {
            if needle.is_empty() || !input.contains(needle) {
                return (input.into(), 0);
            }
            let mut count: u64 = 0;
            let out = input
                .split(needle)
                .enumerate()
                .map(|(i, part)| {
                    if i > 0 {
                        count = count.saturating_add(1);
                    }
                    part
                })
                .collect::<Vec<_>>()
                .join(rule.replacement);
            (out, count)
        }
        Matcher::Regex(re) => {
            let count = re.find_iter(input).count() as u64;
            if count == 0 {
                (input.into(), 0)
            } else {
                (re.replace_all(input, rule.replacement).into_owned(), count)
            }
        }
    }
}

fn walk_value(
    policy: &RedactionPolicy,
    v: &serde_json::Value,
    report: &mut RedactionReport,
) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => {
            let (redacted, sub_report) = policy.apply(s);
            merge_report(report, &sub_report);
            serde_json::Value::String(redacted)
        }
        serde_json::Value::Array(arr) => {
            let mapped: Vec<serde_json::Value> =
                arr.iter().map(|v| walk_value(policy, v, report)).collect();
            serde_json::Value::Array(mapped)
        }
        serde_json::Value::Object(obj) => {
            let mut out = serde_json::Map::with_capacity(obj.len());
            for (k, val) in obj {
                out.insert(k.clone(), walk_value(policy, val, report));
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

fn merge_report(into: &mut RedactionReport, from: &RedactionReport) {
    let mut by_id: std::collections::BTreeMap<String, u64> = into.hits.iter().cloned().collect();
    for (id, n) in &from.hits {
        *by_id.entry(id.clone()).or_insert(0) += *n;
    }
    into.hits = by_id.into_iter().collect();
    into.total_substitutions = into
        .total_substitutions
        .saturating_add(from.total_substitutions);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_compiles() {
        let p = RedactionPolicy::default_policy().unwrap();
        assert_eq!(p.rule_count(), 7);
    }

    #[test]
    fn redacts_credit_card_number() {
        let p = RedactionPolicy::default_policy().unwrap();
        let (out, report) = p.apply("payment 4111 1111 1111 1111 ok");
        assert!(out.contains("[REDACTED:CARD]"));
        assert!(!out.contains("4111 1111 1111 1111"));
        assert_eq!(report.total_substitutions, 1);
        assert_eq!(report.hits[0].0, "RED-001");
    }

    #[test]
    fn redacts_aws_access_key() {
        let p = RedactionPolicy::default_policy().unwrap();
        let (out, _) = p.apply("export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE");
        assert!(out.contains("[REDACTED:AWS_KEY]"));
        assert!(!out.contains("AKIA"));
    }

    #[test]
    fn redacts_github_token() {
        let p = RedactionPolicy::default_policy().unwrap();
        let (out, _) = p.apply("token = ghp_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(out.contains("[REDACTED:GH_TOKEN]"));
    }

    #[test]
    fn redacts_jwt() {
        let p = RedactionPolicy::default_policy().unwrap();
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.abc-def_ghi";
        let (out, _) = p.apply(&format!("auth: {jwt}"));
        assert!(out.contains("[REDACTED:JWT]"));
        assert!(!out.contains("eyJhbGciOiJIUzI1NiJ9"));
    }

    #[test]
    fn redacts_bearer_header() {
        let p = RedactionPolicy::default_policy().unwrap();
        let (out, _) = p.apply("Authorization: Bearer abc.def.ghi-123-token");
        assert!(out.contains("[REDACTED:BEARER]"));
    }

    #[test]
    fn redacts_url_embedded_password() {
        let p = RedactionPolicy::default_policy().unwrap();
        let (out, _) = p.apply("postgres://alice:hunter2@db.example/db");
        assert!(out.contains("[REDACTED:URL_AUTH]@"));
        assert!(!out.contains("hunter2@"));
    }

    #[test]
    fn redacts_pem_private_key() {
        let p = RedactionPolicy::default_policy().unwrap();
        let pem =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAKj…\n-----END RSA PRIVATE KEY-----";
        let (out, _) = p.apply(pem);
        assert!(out.contains("[REDACTED:PEM]"));
        assert!(!out.contains("MIIBOgIBAAJB"));
    }

    #[test]
    fn benign_text_passes_through() {
        let p = RedactionPolicy::default_policy().unwrap();
        let (out, report) = p.apply("hello world");
        assert_eq!(out, "hello world");
        assert_eq!(report.total_substitutions, 0);
        assert!(report.hits.is_empty());
    }

    #[test]
    fn literal_rule_substitutes_substring() {
        let mut p = RedactionPolicy::from_rules(vec![]);
        p.merge(RedactionPolicy::from_rules(vec![RedactionRule {
            id: "RED-LIT",
            description: "literal sentinel",
            matcher: Matcher::Literal("SECRET"),
            replacement: "[X]",
        }]));
        let (out, report) = p.apply("a SECRET b SECRET c");
        assert_eq!(out, "a [X] b [X] c");
        assert_eq!(report.total_substitutions, 2);
    }

    #[test]
    fn apply_to_json_walks_nested_strings() {
        let p = RedactionPolicy::default_policy().unwrap();
        let input = serde_json::json!({
            "a": "Bearer abc.def.ghi-token-12345",
            "nested": {
                "b": "no secret here",
                "c": ["AKIAIOSFODNN7EXAMPLE", "benign"],
            }
        });
        let (out, report) = p.apply_to_json(&input);
        assert!(out["a"].as_str().unwrap().contains("[REDACTED:BEARER]"));
        assert!(out["nested"]["c"][0]
            .as_str()
            .unwrap()
            .contains("[REDACTED:AWS_KEY]"));
        assert_eq!(out["nested"]["b"], "no secret here");
        assert_eq!(report.total_substitutions, 2);
    }

    #[test]
    fn merge_appends_rules() {
        let mut a = RedactionPolicy::from_rules(vec![]);
        let b = RedactionPolicy::from_rules(vec![RedactionRule {
            id: "RED-EXTRA",
            description: "extra",
            matcher: Matcher::Literal("foo"),
            replacement: "[bar]",
        }]);
        a.merge(b);
        assert_eq!(a.rule_count(), 1);
        let (out, _) = a.apply("foo");
        assert_eq!(out, "[bar]");
    }

    #[test]
    fn report_counts_aggregate_across_invocations() {
        let p = RedactionPolicy::default_policy().unwrap();
        let (_, report) = p.apply("card 4111 1111 1111 1111 and another 5555 5555 5555 4444");
        assert_eq!(report.total_substitutions, 2);
        // Single rule fired twice → 1 hit entry.
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].1, 2);
    }
}
