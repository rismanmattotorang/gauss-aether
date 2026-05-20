//! `gaussclaw-plugins` — Hermes-parity plugin loader.
//!
//! Sprint 7 §1 of `/ROADMAP.md`. Hermes ships a 5-kind plugin system
//! (`standalone` / `backend` / `exclusive` / `platform` /
//! `model-provider`) under `hermes_cli/plugins.py` (~1 450 LOC). Every
//! plugin loads with the full operator credential set; there's no
//! cap declaration anywhere in the manifest.
//!
//! GaussClaw's variant ships:
//!
//! - A typed [`PluginManifest`] parsed from `plugin.toml`. Every
//!   manifest declares its `caps = [...]` — the kernel admit gate
//!   restricts the plugin to its declared cap set. **A plugin
//!   cannot acquire a cap it didn't declare.**
//! - A `PluginKind` enum that mirrors Hermes's 5 kinds.
//! - A [`PluginLoader::discover_in`] that walks a discovery root
//!   for `plugin.toml` files; production wires 4 roots (bundled /
//!   user / project / workspace-member).
//! - A [`PluginRegistry`] that holds loaded plugins, indexed by
//!   `(kind, name)`. Registration is cap-gated by the live grant.
//!
//! ## Hermes-superiority axes
//!
//! - **Cap declaration is data.** Every plugin's manifest declares
//!   its required caps. The kernel admit gate refuses load if the
//!   session's grant doesn't satisfy the manifest. Hermes plugins
//!   inherit the full process credentials.
//! - **Provenance digest.** Every loaded plugin carries a
//!   BLAKE3 of its manifest bytes. The audit chain records the
//!   digest at load time; tampered plugins surface immediately.
//! - **Path-traversal guard.** Discovery refuses symlink chains and
//!   `..` references at parse time.

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::missing_errors_doc,
    clippy::missing_docs_in_private_items,
    clippy::too_long_first_doc_paragraph,
    clippy::significant_drop_tightening,
    clippy::redundant_clone
)]
#![allow(rustdoc::broken_intra_doc_links)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gauss_core::CapToken;
use gaussclaw_skill::parse_cap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One of Hermes's 5 plugin kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginKind {
    /// A free-standing capability extension (no exclusive coupling).
    Standalone,
    /// A backend that satisfies a typed contract (e.g. a new SessionExecutor).
    Backend,
    /// An exclusive replacement of a first-party subsystem — only one
    /// `exclusive` plugin can be active per id at a time.
    Exclusive,
    /// A channel / messaging-platform adapter.
    Platform,
    /// An LLM provider plugin.
    ModelProvider,
}

impl PluginKind {
    /// Stable string tag — `standalone`, `backend`, `exclusive`,
    /// `platform`, `model_provider`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::Backend => "backend",
            Self::Exclusive => "exclusive",
            Self::Platform => "platform",
            Self::ModelProvider => "model_provider",
        }
    }
}

/// Lifecycle stage a `HookDeclaration` attaches to.
///
/// Mirrors `gauss_hooks::{PreToolHook, PostToolHook}` — at startup
/// the plugin loader registers each declared hook with the live
/// `HookBus` at the corresponding stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookLifecycle {
    /// Fired before tool dispatch. May `Warn` or `Deny`.
    PreTool,
    /// Fired after tool dispatch. Advisory only.
    PostTool,
}

/// One hook a plugin promises to install when loaded. Data-only —
/// the runtime resolves `id` to a concrete `PreToolHook` / `PostToolHook`
/// implementation at registration time. Plugins that haven't been
/// loaded yet still surface their declarations through `plugin list`
/// so operators can see what they're about to admit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct HookDeclaration {
    /// Hook id (`[a-z0-9_-]+`). Plugins MUST resolve this id to a
    /// concrete callback when they're loaded; the loader fails
    /// closed on a missing implementation.
    pub id: String,
    /// Lifecycle stage — `pre_tool` or `post_tool`.
    pub lifecycle: HookLifecycle,
    /// Optional dispatch priority (0 = earliest). Mirrors the
    /// `HookBus::register_*` priority parameter. Defaults to 100.
    #[serde(default = "default_hook_priority")]
    pub priority: u8,
    /// Optional list of tool ids the hook applies to. Empty (the
    /// default) means "every tool" — the bus consults the hook for
    /// every dispatch.
    #[serde(default)]
    pub target_tools: Vec<String>,
    /// Human-readable description shown in `plugin list`.
    #[serde(default)]
    pub description: String,
}

const fn default_hook_priority() -> u8 {
    100
}

/// Parsed `plugin.toml` (one per plugin).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PluginManifest {
    /// Plugin slug (`[a-z0-9_-]+`).
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Plugin kind.
    pub kind: PluginKind,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Capability strings (e.g. `"network:http_get"`). Same grammar
    /// as `SkillManifest::caps`.
    #[serde(default)]
    pub caps: Vec<String>,
    /// Entry-point hint — for `standalone`/`backend` plugins, the
    /// workspace-member name. For WASM plugins, the path to the
    /// `.wasm` blob. Free-form string interpreted by the host.
    #[serde(default)]
    pub entry: String,
    /// Tags for filtering / dashboard rendering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Hooks the plugin declares it will install (OpenHarness-inspired).
    /// Each declaration carries an id + lifecycle + priority + an
    /// optional `target_tools` filter. The plugin loader resolves
    /// the ids to concrete `PreToolHook` / `PostToolHook` impls at
    /// startup and registers them with the live `HookBus`.
    #[serde(default)]
    pub hooks: Vec<HookDeclaration>,
}

impl PluginManifest {
    /// Parse from a TOML string.
    ///
    /// # Errors
    /// Returns [`PluginError::Toml`] when the TOML doesn't parse.
    pub fn from_toml(s: &str) -> Result<Self, PluginError> {
        let m: Self = toml::from_str(s).map_err(PluginError::Toml)?;
        m.validate()?;
        Ok(m)
    }

    /// Resolve declared caps into a single `CapToken` bit-OR.
    ///
    /// # Errors
    /// Returns [`PluginError::UnknownCap`] if a cap string isn't in
    /// the canonical map (mirrors `gaussclaw-skill::parse_cap`).
    pub fn declared_caps(&self) -> Result<CapToken, PluginError> {
        let mut acc: u64 = 0;
        for cap in &self.caps {
            acc |= parse_cap(cap)
                .map_err(|e| PluginError::UnknownCap(format!("{cap}: {e}")))?
                .bits();
        }
        Ok(CapToken::from_bits(acc))
    }

    /// Compute the BLAKE3 of the manifest's canonical TOML re-serialise.
    /// The digest is the plugin's stable provenance id.
    ///
    /// # Errors
    /// Returns [`PluginError::Toml`] if re-serialisation fails (a
    /// build-time bug — the manifest already round-trips through
    /// `from_toml`).
    pub fn provenance_digest(&self) -> Result<String, PluginError> {
        let canonical = toml::to_string(self)
            .map_err(|e| PluginError::Backend(format!("re-serialise: {e}")))?;
        Ok(blake3::hash(canonical.as_bytes()).to_hex().to_string())
    }

    fn validate(&self) -> Result<(), PluginError> {
        if self.name.is_empty() {
            return Err(PluginError::InvalidManifest("name empty".into()));
        }
        for c in self.name.chars() {
            if !c.is_ascii_alphanumeric() && c != '_' && c != '-' {
                return Err(PluginError::InvalidManifest(format!(
                    "name contains forbidden char {c:?}"
                )));
            }
        }
        if self.version.is_empty() {
            return Err(PluginError::InvalidManifest("version empty".into()));
        }
        // Hook declarations must have unique ids and valid id grammar.
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for h in &self.hooks {
            if h.id.is_empty() {
                return Err(PluginError::InvalidManifest("hook id empty".into()));
            }
            for c in h.id.chars() {
                if !c.is_ascii_alphanumeric() && c != '_' && c != '-' {
                    return Err(PluginError::InvalidManifest(format!(
                        "hook id `{}` contains forbidden char {c:?}",
                        h.id
                    )));
                }
            }
            if !seen.insert(h.id.as_str()) {
                return Err(PluginError::InvalidManifest(format!(
                    "duplicate hook id `{}`",
                    h.id
                )));
            }
        }
        Ok(())
    }

    /// Iterate hooks of the given lifecycle. The plugin loader calls
    /// this twice — once per lifecycle — and registers each entry
    /// with the live `HookBus`.
    pub fn hooks_for(&self, lifecycle: HookLifecycle) -> impl Iterator<Item = &HookDeclaration> {
        self.hooks.iter().filter(move |h| h.lifecycle == lifecycle)
    }

    /// Convenience: report whether the plugin declares any hook for
    /// the given tool id. The match honours `HookDeclaration::target_tools`
    /// — an empty list matches every tool.
    #[must_use]
    pub fn applies_to_tool(&self, lifecycle: HookLifecycle, tool: &str) -> bool {
        self.hooks_for(lifecycle).any(|h| {
            h.target_tools.is_empty() || h.target_tools.iter().any(|t| t == tool)
        })
    }
}

/// Crate-wide error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PluginError {
    /// TOML parse failure.
    #[error("toml: {0}")]
    Toml(toml::de::Error),
    /// Manifest schema check failed.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    /// Cap string didn't resolve.
    #[error("unknown cap: {0}")]
    UnknownCap(String),
    /// I/O failure.
    #[error("io: {0}")]
    Io(String),
    /// Kernel admit gate refused load.
    #[error("admit refused: plugin {name} requires cap 0x{required:016x}, grant 0x{grant:016x}")]
    AdmitRefused {
        /// Plugin name.
        name: String,
        /// Bits required.
        required: u64,
        /// Bits the grant exposes.
        grant: u64,
    },
    /// A plugin with the same `(kind, name)` is already registered.
    #[error("plugin already registered: {kind}/{name}")]
    AlreadyRegistered {
        /// Kind tag.
        kind: &'static str,
        /// Name.
        name: String,
    },
    /// Discovery refused a manifest path (e.g. traversal).
    #[error("refused manifest path: {0}")]
    RefusedPath(String),
    /// Backend-side failure.
    #[error("backend: {0}")]
    Backend(String),
}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Result alias.
pub type PluginResult<T> = Result<T, PluginError>;

/// One loaded plugin — the parsed manifest + its provenance digest +
/// the kind-discriminated `enabled` flag.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// Parsed manifest.
    pub manifest: PluginManifest,
    /// BLAKE3 of the manifest's canonical TOML re-serialise.
    pub provenance: String,
    /// Whether this plugin is currently enabled.
    pub enabled: bool,
    /// Path to the manifest file (for diagnostics + dashboard).
    pub manifest_path: Option<PathBuf>,
}

/// Discovery — walks a directory and parses every `plugin.toml`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PluginLoader;

impl PluginLoader {
    /// Discover every plugin under `root`. Each subdirectory's
    /// `plugin.toml` (if present) is parsed.
    ///
    /// Discovery refuses symlinks and `..`-relative paths at parse
    /// time. A malformed `plugin.toml` surfaces as one
    /// [`DiscoveryReport::failures`] entry; other plugins continue
    /// loading.
    pub async fn discover_in(root: &Path) -> PluginResult<DiscoveryReport> {
        let mut found: Vec<LoadedPlugin> = Vec::new();
        let mut failures: Vec<(PathBuf, String)> = Vec::new();
        if !root.exists() {
            return Ok(DiscoveryReport { found, failures });
        }
        let mut rd = tokio::fs::read_dir(root).await?;
        while let Some(entry) = rd.next_entry().await? {
            let path = entry.path();
            let meta = entry.metadata().await?;
            // We accept regular directories only. Symlinks +
            // non-dirs are silently skipped (the worker pool
            // mounted under root might inject scratch files).
            if !meta.is_dir() || meta.file_type().is_symlink() {
                continue;
            }
            let manifest_path = path.join("plugin.toml");
            if !tokio::fs::try_exists(&manifest_path).await? {
                continue;
            }
            // Refuse manifests outside `root` (defence against
            // symlinked subdirs).
            if let Ok(canon) = tokio::fs::canonicalize(&manifest_path).await {
                if !canon.starts_with(root)
                    && !canon.starts_with(
                        tokio::fs::canonicalize(root)
                            .await
                            .unwrap_or_else(|_| root.into()),
                    )
                {
                    failures.push((manifest_path.clone(), "path traversal refused".into()));
                    continue;
                }
            }
            match tokio::fs::read_to_string(&manifest_path).await {
                Ok(s) => match PluginManifest::from_toml(&s) {
                    Ok(m) => {
                        let provenance = m.provenance_digest().unwrap_or_else(|_| String::new());
                        found.push(LoadedPlugin {
                            manifest: m,
                            provenance,
                            enabled: true,
                            manifest_path: Some(manifest_path),
                        });
                    }
                    Err(e) => failures.push((manifest_path, format!("{e}"))),
                },
                Err(e) => failures.push((manifest_path, format!("{e}"))),
            }
        }
        // Sort for determinism.
        found.sort_by(|a, b| {
            a.manifest
                .kind
                .as_str()
                .cmp(b.manifest.kind.as_str())
                .then_with(|| a.manifest.name.cmp(&b.manifest.name))
        });
        Ok(DiscoveryReport { found, failures })
    }
}

/// One discovery pass.
#[derive(Debug, Default)]
pub struct DiscoveryReport {
    /// Successfully loaded plugins.
    pub found: Vec<LoadedPlugin>,
    /// Per-file failure reasons. The dashboard surfaces these so
    /// operators can fix manifests in-place.
    pub failures: Vec<(PathBuf, String)>,
}

/// Plugin registry. Cap-gates registration against the live grant.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    inner: Mutex<BTreeMap<(&'static str, String), LoadedPlugin>>,
}

impl PluginRegistry {
    /// Build a fresh empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of registered plugins.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("poisoned").len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Register a plugin against the live grant. The plugin's
    /// declared caps MUST be a subset of `grant` — otherwise the
    /// registry refuses.
    pub fn register(&self, plugin: LoadedPlugin, grant: CapToken) -> PluginResult<()> {
        let required = plugin.manifest.declared_caps()?;
        if !grant.contains(required) {
            return Err(PluginError::AdmitRefused {
                name: plugin.manifest.name.clone(),
                required: required.bits(),
                grant: grant.bits(),
            });
        }
        let key = (plugin.manifest.kind.as_str(), plugin.manifest.name.clone());
        let mut g = self.inner.lock().expect("poisoned");
        if g.contains_key(&key) {
            return Err(PluginError::AlreadyRegistered {
                kind: key.0,
                name: key.1,
            });
        }
        g.insert(key, plugin);
        Ok(())
    }

    /// Look up a plugin by `(kind, name)`.
    #[must_use]
    pub fn get(&self, kind: PluginKind, name: &str) -> Option<LoadedPlugin> {
        self.inner
            .lock()
            .expect("poisoned")
            .get(&(kind.as_str(), name.to_string()))
            .cloned()
    }

    /// List every registered plugin.
    #[must_use]
    pub fn list(&self) -> Vec<LoadedPlugin> {
        self.inner
            .lock()
            .expect("poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Toggle enabled state.
    pub fn set_enabled(&self, kind: PluginKind, name: &str, enabled: bool) -> PluginResult<()> {
        let mut g = self.inner.lock().expect("poisoned");
        let key = (kind.as_str(), name.to_string());
        let plugin = g.get_mut(&key).ok_or_else(|| {
            PluginError::Backend(format!("unknown plugin {}/{name}", kind.as_str()))
        })?;
        plugin.enabled = enabled;
        Ok(())
    }

    /// Drop a plugin from the registry.
    pub fn unregister(&self, kind: PluginKind, name: &str) -> PluginResult<()> {
        let mut g = self.inner.lock().expect("poisoned");
        let key = (kind.as_str(), name.to_string());
        g.remove(&key).ok_or_else(|| {
            PluginError::Backend(format!("unknown plugin {}/{name}", kind.as_str()))
        })?;
        Ok(())
    }
}

/// Canonical discovery roots. Production deployments concatenate the
/// results of these into one [`PluginLoader::discover_in`] sweep per
/// root.
#[must_use]
pub fn default_discovery_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(proj) = directories::ProjectDirs::from("io", "gauss-aether", "gaussclaw") {
        roots.push(proj.data_dir().join("plugins"));
    }
    // Project-local opt-in root.
    if let Ok(cwd) = std::env::current_dir() {
        let project_root = cwd.join(".gaussclaw").join("plugins");
        if project_root.exists() {
            roots.push(project_root);
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_toml(name: &str, kind: &str, caps: &[&str]) -> String {
        let caps_str = caps
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"
name        = "{name}"
version     = "0.1.0"
kind        = "{kind}"
description = "test plugin"
caps        = [{caps_str}]
entry       = "{name}"
tags        = []
"#
        )
    }

    #[test]
    fn manifest_round_trips() {
        let m =
            PluginManifest::from_toml(&sample_toml("alpha", "standalone", &["network:http_get"]))
                .unwrap();
        assert_eq!(m.name, "alpha");
        assert_eq!(m.kind, PluginKind::Standalone);
        assert_eq!(m.caps, vec!["network:http_get"]);
    }

    #[test]
    fn manifest_rejects_unknown_field() {
        let bad = r#"
name = "x"
version = "0.1.0"
kind = "standalone"
bogus_field = true
"#;
        assert!(PluginManifest::from_toml(bad).is_err());
    }

    #[test]
    fn manifest_validate_rejects_traversal_name() {
        let bad = r#"
name = "../escape"
version = "0.1.0"
kind = "standalone"
"#;
        assert!(PluginManifest::from_toml(bad).is_err());
    }

    #[test]
    fn manifest_validate_rejects_empty_name_or_version() {
        assert!(PluginManifest::from_toml(
            r#"name = ""
version = "0.1.0"
kind = "standalone"
"#
        )
        .is_err());
        assert!(PluginManifest::from_toml(
            r#"name = "x"
version = ""
kind = "standalone"
"#
        )
        .is_err());
    }

    #[test]
    fn declared_caps_resolves_known_strings() {
        let m = PluginManifest::from_toml(&sample_toml(
            "a",
            "standalone",
            &["network:http_get", "fs:read"],
        ))
        .unwrap();
        let token = m.declared_caps().unwrap();
        assert!(token.contains(CapToken::NETWORK_GET));
        assert!(token.contains(CapToken::FILESYSTEM_READ));
    }

    #[test]
    fn declared_caps_rejects_unknown_cap() {
        let m =
            PluginManifest::from_toml(&sample_toml("a", "standalone", &["unknown:cap"])).unwrap();
        assert!(matches!(m.declared_caps(), Err(PluginError::UnknownCap(_))));
    }

    #[test]
    fn provenance_digest_is_stable() {
        let m1 = PluginManifest::from_toml(&sample_toml("a", "standalone", &[])).unwrap();
        let m2 = PluginManifest::from_toml(&sample_toml("a", "standalone", &[])).unwrap();
        assert_eq!(
            m1.provenance_digest().unwrap(),
            m2.provenance_digest().unwrap()
        );
        let m3 = PluginManifest::from_toml(&sample_toml("b", "standalone", &[])).unwrap();
        assert_ne!(
            m1.provenance_digest().unwrap(),
            m3.provenance_digest().unwrap()
        );
    }

    #[test]
    fn plugin_kind_string_tags_are_stable() {
        assert_eq!(PluginKind::Standalone.as_str(), "standalone");
        assert_eq!(PluginKind::Backend.as_str(), "backend");
        assert_eq!(PluginKind::Exclusive.as_str(), "exclusive");
        assert_eq!(PluginKind::Platform.as_str(), "platform");
        assert_eq!(PluginKind::ModelProvider.as_str(), "model_provider");
    }

    #[tokio::test]
    async fn discover_walks_subdirectories() {
        let dir = tempdir().unwrap();
        // plug-a
        let a = dir.path().join("plug-a");
        tokio::fs::create_dir(&a).await.unwrap();
        tokio::fs::write(
            a.join("plugin.toml"),
            sample_toml("alpha", "standalone", &[]),
        )
        .await
        .unwrap();
        // plug-b
        let b = dir.path().join("plug-b");
        tokio::fs::create_dir(&b).await.unwrap();
        tokio::fs::write(b.join("plugin.toml"), sample_toml("beta", "backend", &[]))
            .await
            .unwrap();

        let report = PluginLoader::discover_in(dir.path()).await.unwrap();
        assert_eq!(report.found.len(), 2);
        assert!(report.failures.is_empty());
        let names: Vec<&str> = report
            .found
            .iter()
            .map(|p| p.manifest.name.as_str())
            .collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[tokio::test]
    async fn discover_collects_failures_but_continues() {
        let dir = tempdir().unwrap();
        let good = dir.path().join("good");
        tokio::fs::create_dir(&good).await.unwrap();
        tokio::fs::write(
            good.join("plugin.toml"),
            sample_toml("g", "standalone", &[]),
        )
        .await
        .unwrap();
        let bad = dir.path().join("bad");
        tokio::fs::create_dir(&bad).await.unwrap();
        tokio::fs::write(bad.join("plugin.toml"), "not toml = is = invalid")
            .await
            .unwrap();
        let report = PluginLoader::discover_in(dir.path()).await.unwrap();
        assert_eq!(report.found.len(), 1);
        assert_eq!(report.failures.len(), 1);
    }

    #[tokio::test]
    async fn discover_skips_directories_without_manifest() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("empty");
        tokio::fs::create_dir(&p).await.unwrap();
        let report = PluginLoader::discover_in(dir.path()).await.unwrap();
        assert!(report.found.is_empty());
        assert!(report.failures.is_empty());
    }

    #[tokio::test]
    async fn discover_returns_empty_for_missing_root() {
        let report = PluginLoader::discover_in(Path::new("/does/not/exist"))
            .await
            .unwrap();
        assert!(report.found.is_empty());
        assert!(report.failures.is_empty());
    }

    #[test]
    fn registry_registers_when_grant_satisfies_caps() {
        let manifest =
            PluginManifest::from_toml(&sample_toml("a", "standalone", &["network:http_get"]))
                .unwrap();
        let provenance = manifest.provenance_digest().unwrap();
        let reg = PluginRegistry::new();
        reg.register(
            LoadedPlugin {
                manifest,
                provenance,
                enabled: true,
                manifest_path: None,
            },
            CapToken::NETWORK_GET,
        )
        .unwrap();
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_refuses_when_grant_missing_caps() {
        let manifest =
            PluginManifest::from_toml(&sample_toml("a", "standalone", &["network:http_get"]))
                .unwrap();
        let provenance = manifest.provenance_digest().unwrap();
        let reg = PluginRegistry::new();
        let err = reg
            .register(
                LoadedPlugin {
                    manifest,
                    provenance,
                    enabled: true,
                    manifest_path: None,
                },
                CapToken::BOTTOM,
            )
            .unwrap_err();
        assert!(matches!(err, PluginError::AdmitRefused { .. }));
        assert!(reg.is_empty());
    }

    #[test]
    fn registry_refuses_duplicate_kind_name() {
        let m = PluginManifest::from_toml(&sample_toml("dup", "standalone", &[])).unwrap();
        let provenance = m.provenance_digest().unwrap();
        let reg = PluginRegistry::new();
        reg.register(
            LoadedPlugin {
                manifest: m.clone(),
                provenance: provenance.clone(),
                enabled: true,
                manifest_path: None,
            },
            CapToken::TOP,
        )
        .unwrap();
        let err = reg
            .register(
                LoadedPlugin {
                    manifest: m,
                    provenance,
                    enabled: true,
                    manifest_path: None,
                },
                CapToken::TOP,
            )
            .unwrap_err();
        assert!(matches!(err, PluginError::AlreadyRegistered { .. }));
    }

    #[test]
    fn registry_get_and_set_enabled_round_trip() {
        let m = PluginManifest::from_toml(&sample_toml("a", "platform", &[])).unwrap();
        let provenance = m.provenance_digest().unwrap();
        let reg = PluginRegistry::new();
        reg.register(
            LoadedPlugin {
                manifest: m,
                provenance,
                enabled: true,
                manifest_path: None,
            },
            CapToken::TOP,
        )
        .unwrap();
        assert!(reg.get(PluginKind::Platform, "a").unwrap().enabled);
        reg.set_enabled(PluginKind::Platform, "a", false).unwrap();
        assert!(!reg.get(PluginKind::Platform, "a").unwrap().enabled);
    }

    #[test]
    fn registry_unregister_drops_plugin() {
        let m = PluginManifest::from_toml(&sample_toml("a", "backend", &[])).unwrap();
        let provenance = m.provenance_digest().unwrap();
        let reg = PluginRegistry::new();
        reg.register(
            LoadedPlugin {
                manifest: m,
                provenance,
                enabled: true,
                manifest_path: None,
            },
            CapToken::TOP,
        )
        .unwrap();
        reg.unregister(PluginKind::Backend, "a").unwrap();
        assert!(reg.is_empty());
    }

    // ── OpenHarness-inspired hook declaration tests ──────────────────────

    const HOOK_MANIFEST: &str = r#"
name        = "guard"
version     = "0.1.0"
kind        = "standalone"
description = "blocks dangerous shells"
caps        = []

[[hooks]]
id          = "shell-guard"
lifecycle   = "pre_tool"
priority    = 10
target_tools = ["shell"]
description  = "refuses rm -rf /"

[[hooks]]
id          = "audit-log"
lifecycle   = "post_tool"
description = "writes every result to /var/log"
"#;

    #[test]
    fn hooks_round_trip_through_toml() {
        let m = PluginManifest::from_toml(HOOK_MANIFEST).expect("parse");
        assert_eq!(m.hooks.len(), 2);
        assert_eq!(m.hooks[0].id, "shell-guard");
        assert!(matches!(m.hooks[0].lifecycle, HookLifecycle::PreTool));
        assert_eq!(m.hooks[0].priority, 10);
        assert_eq!(m.hooks[0].target_tools, vec!["shell".to_string()]);
        assert!(matches!(m.hooks[1].lifecycle, HookLifecycle::PostTool));
        // priority defaulted to 100.
        assert_eq!(m.hooks[1].priority, 100);
    }

    #[test]
    fn empty_hook_id_is_rejected() {
        let raw = r#"
name = "x"
version = "0.1.0"
kind = "standalone"

[[hooks]]
id = ""
lifecycle = "pre_tool"
"#;
        let err = PluginManifest::from_toml(raw).unwrap_err();
        assert!(matches!(err, PluginError::InvalidManifest(_)));
    }

    #[test]
    fn duplicate_hook_id_is_rejected() {
        let raw = r#"
name = "x"
version = "0.1.0"
kind = "standalone"

[[hooks]]
id = "h"
lifecycle = "pre_tool"

[[hooks]]
id = "h"
lifecycle = "post_tool"
"#;
        let err = PluginManifest::from_toml(raw).unwrap_err();
        match err {
            PluginError::InvalidManifest(msg) => assert!(msg.contains("duplicate")),
            other => panic!("expected InvalidManifest, got {other:?}"),
        }
    }

    #[test]
    fn forbidden_chars_in_hook_id_rejected() {
        let raw = r#"
name = "x"
version = "0.1.0"
kind = "standalone"

[[hooks]]
id = "bad id"
lifecycle = "pre_tool"
"#;
        let err = PluginManifest::from_toml(raw).unwrap_err();
        assert!(matches!(err, PluginError::InvalidManifest(_)));
    }

    #[test]
    fn hooks_for_filters_by_lifecycle() {
        let m = PluginManifest::from_toml(HOOK_MANIFEST).unwrap();
        let pre: Vec<&str> = m
            .hooks_for(HookLifecycle::PreTool)
            .map(|h| h.id.as_str())
            .collect();
        let post: Vec<&str> = m
            .hooks_for(HookLifecycle::PostTool)
            .map(|h| h.id.as_str())
            .collect();
        assert_eq!(pre, vec!["shell-guard"]);
        assert_eq!(post, vec!["audit-log"]);
    }

    #[test]
    fn applies_to_tool_honours_target_filter() {
        let m = PluginManifest::from_toml(HOOK_MANIFEST).unwrap();
        // shell-guard targets only "shell".
        assert!(m.applies_to_tool(HookLifecycle::PreTool, "shell"));
        assert!(!m.applies_to_tool(HookLifecycle::PreTool, "echo"));
        // audit-log has an empty target list → matches every tool.
        assert!(m.applies_to_tool(HookLifecycle::PostTool, "shell"));
        assert!(m.applies_to_tool(HookLifecycle::PostTool, "echo"));
    }

    #[test]
    fn manifest_without_hooks_parses_clean() {
        let raw = r#"
name = "x"
version = "0.1.0"
kind = "standalone"
"#;
        let m = PluginManifest::from_toml(raw).unwrap();
        assert!(m.hooks.is_empty());
        assert!(!m.applies_to_tool(HookLifecycle::PreTool, "any"));
    }

    #[test]
    fn unknown_field_under_hooks_is_rejected() {
        let raw = r#"
name = "x"
version = "0.1.0"
kind = "standalone"

[[hooks]]
id = "h"
lifecycle = "pre_tool"
rogue = true
"#;
        let err = PluginManifest::from_toml(raw).unwrap_err();
        assert!(matches!(err, PluginError::Toml(_)));
    }
}
