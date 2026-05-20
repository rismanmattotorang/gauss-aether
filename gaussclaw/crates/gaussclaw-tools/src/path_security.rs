//! `path_security` tool (Sprint 7 §4).
//!
//! Pre-execution filesystem-path guard. Inspects a candidate path
//! string against a structured rule set (path-traversal, absolute
//! paths outside an allowlist, paths under `/etc`, `/proc`, `/sys`,
//! etc.). Returns a graded [`PathVerdict`].
//!
//! Hermes runs every FS access under raw operator credentials; the
//! only guard is "is the file there". GaussClaw's tool is the
//! kernel-side pre-check **plus** an auditable verdict record.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use serde::{Deserialize, Serialize};

const MANIFEST_TOML: &str = r#"
name        = "path_security"
description = "Validate a candidate filesystem path against a structured rule set. Returns verdict + rule id."
usage       = "Args: {path: string, allow_roots?: [string]}. Returns {verdict, rule_id?, reason}."
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

/// Verdict from one path scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PathVerdict {
    /// Path is in scope and benign.
    Allow,
    /// Path is unusual but not catastrophic; operator should review.
    Warn,
    /// Path is dangerous — caller refuses by default.
    Refuse,
}

/// One pattern rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRule {
    /// Stable identifier.
    pub id: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// Verdict.
    pub verdict: PathVerdict,
}

/// Builtin path-security rule set.
pub const PATH_RULES: &[PathRule] = &[
    PathRule {
        id: "PATH-001",
        description: "path-component traversal (`..` segment)",
        verdict: PathVerdict::Refuse,
    },
    PathRule {
        id: "PATH-010",
        description: "absolute path under /etc /proc /sys (system config)",
        verdict: PathVerdict::Refuse,
    },
    PathRule {
        id: "PATH-011",
        description: "absolute path under /root /var/log (privileged)",
        verdict: PathVerdict::Refuse,
    },
    PathRule {
        id: "PATH-020",
        description: "absolute path outside operator-supplied allowlist",
        verdict: PathVerdict::Warn,
    },
    PathRule {
        id: "PATH-030",
        description: "path with NUL byte (filesystem injection)",
        verdict: PathVerdict::Refuse,
    },
];

/// Scan a candidate `path` against the rule set. `allow_roots`
/// (operator-supplied) is the set of absolute prefixes a benign
/// absolute path may live under; an absolute path outside any
/// allowed root produces a Warn unless it hits a refuse rule first.
#[must_use]
pub fn scan_path(path: &str, allow_roots: &[PathBuf]) -> (PathVerdict, Option<&'static PathRule>) {
    if path.contains('\0') {
        return (
            PathVerdict::Refuse,
            Some(rule_by_id("PATH-030").expect("PATH-030")),
        );
    }
    let p = Path::new(path);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return (
            PathVerdict::Refuse,
            Some(rule_by_id("PATH-001").expect("PATH-001")),
        );
    }
    let absolute = p.is_absolute();
    let lower = path.to_ascii_lowercase();
    let refuse_etc = ["/etc/", "/proc/", "/sys/"]
        .iter()
        .any(|prefix| lower.starts_with(prefix));
    if refuse_etc {
        return (
            PathVerdict::Refuse,
            Some(rule_by_id("PATH-010").expect("PATH-010")),
        );
    }
    let refuse_privileged = ["/root/", "/var/log/"]
        .iter()
        .any(|prefix| lower.starts_with(prefix));
    if refuse_privileged {
        return (
            PathVerdict::Refuse,
            Some(rule_by_id("PATH-011").expect("PATH-011")),
        );
    }
    if absolute {
        let within = allow_roots.iter().any(|root| p.starts_with(root));
        if !within {
            return (
                PathVerdict::Warn,
                Some(rule_by_id("PATH-020").expect("PATH-020")),
            );
        }
    }
    (PathVerdict::Allow, None)
}

fn rule_by_id(id: &str) -> Option<&'static PathRule> {
    PATH_RULES.iter().find(|r| r.id == id)
}

/// `path_security` tool.
pub struct PathSecurityTool {
    manifest: ToolManifest,
}

impl PathSecurityTool {
    /// Build.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("toml");
        let manifest = skill
            .compile(ToolId("path_security".into()))
            .expect("compile");
        Self { manifest }
    }
}

impl Default for PathSecurityTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for PathSecurityTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `path`".into()))?;
        let allow_roots_val = args.get("allow_roots").and_then(|v| v.as_array());
        let allow_roots: Vec<PathBuf> = match allow_roots_val {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(PathBuf::from))
                .collect(),
            None => Vec::new(),
        };
        let (verdict, rule) = scan_path(path, &allow_roots);
        let verdict_str = match verdict {
            PathVerdict::Allow => "allow",
            PathVerdict::Warn => "warn",
            PathVerdict::Refuse => "refuse",
        };
        Ok(serde_json::json!({
            "kind":    "path_verdict",
            "verdict": verdict_str,
            "rule_id": rule.map(|r| r.id),
            "reason":  rule.map(|r| r.description),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benign_relative_path_is_allowed() {
        let (v, _) = scan_path("src/lib.rs", &[]);
        assert_eq!(v, PathVerdict::Allow);
    }

    #[test]
    fn traversal_is_refused() {
        let (v, r) = scan_path("src/../etc/passwd", &[]);
        assert_eq!(v, PathVerdict::Refuse);
        assert_eq!(r.unwrap().id, "PATH-001");
    }

    #[test]
    fn etc_proc_sys_refused() {
        for p in ["/etc/passwd", "/proc/cmdline", "/sys/kernel"] {
            let (v, r) = scan_path(p, &[]);
            assert_eq!(v, PathVerdict::Refuse, "{p}");
            assert_eq!(r.unwrap().id, "PATH-010");
        }
    }

    #[test]
    fn root_var_log_refused() {
        let (v, r) = scan_path("/root/.ssh/id_rsa", &[]);
        assert_eq!(v, PathVerdict::Refuse);
        assert_eq!(r.unwrap().id, "PATH-011");
    }

    #[test]
    fn nul_byte_refused() {
        let (v, r) = scan_path("/tmp/file\0extra", &[]);
        assert_eq!(v, PathVerdict::Refuse);
        assert_eq!(r.unwrap().id, "PATH-030");
    }

    #[test]
    fn absolute_outside_allowlist_warns() {
        let (v, r) = scan_path("/tmp/scratch", &[PathBuf::from("/home/user/project")]);
        assert_eq!(v, PathVerdict::Warn);
        assert_eq!(r.unwrap().id, "PATH-020");
    }

    #[test]
    fn absolute_within_allowlist_allowed() {
        let (v, _) = scan_path(
            "/home/user/project/src/main.rs",
            &[PathBuf::from("/home/user/project")],
        );
        assert_eq!(v, PathVerdict::Allow);
    }

    #[tokio::test]
    async fn tool_returns_verdict_for_traversal() {
        let t = PathSecurityTool::new();
        let out = t
            .invoke_raw(serde_json::json!({"path": "../etc/passwd"}))
            .await
            .unwrap();
        assert_eq!(out["verdict"], "refuse");
        assert_eq!(out["rule_id"], "PATH-001");
    }

    #[tokio::test]
    async fn tool_uses_operator_allow_roots() {
        let t = PathSecurityTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "path": "/srv/data/x",
                "allow_roots": ["/srv/data"]
            }))
            .await
            .unwrap();
        assert_eq!(out["verdict"], "allow");
    }
}
