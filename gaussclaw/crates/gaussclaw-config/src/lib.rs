//! `gaussclaw-config` — Hermes-compatible TOML configuration loader.
//!
//! Phase 1 Task 6 of `GAUSSCLAW_ROADMAP.md`. Provides:
//!
//! - The [`Config`] root struct, whose top-level keys (`provider`,
//!   `surfaces`, `channels`, `tools`) are byte-for-byte compatible with
//!   the upstream Hermes config schema (Binding Constraint #4).
//! - GaussClaw-only namespaced extensions (`caps`, `taint`, `export`,
//!   `desktop`) — all optional, defaults preserve Hermes behaviour.
//! - A figment-based [`load`] entry point with a deterministic search
//!   path: `--config <PATH>` > `$GAUSSCLAW_CONFIG` env > `$XDG_CONFIG_HOME/
//!   gaussclaw/config.toml` > platform default > workspace `./gaussclaw.toml`.
//! - A round-trip [`save`] helper.
//!
//! Capability-gated writes land in Phase 3 (the `caps:config:write`
//! Skill Manifest); this module just provides the schema and IO.

#![allow(clippy::doc_markdown)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── errors ─────────────────────────────────────────────────────────────────

/// Loading and saving errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The named file was not present on disk.
    #[error("config file not found: {0}")]
    NotFound(PathBuf),

    /// The file parsed but did not match the schema.
    #[error("config parse: {0}")]
    Parse(Box<figment::Error>),

    /// IO failure (read, write, mkdir).
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),

    /// Serialisation back to TOML failed.
    #[error("config serialise: {0}")]
    Serialise(#[from] toml::ser::Error),
}

impl From<figment::Error> for ConfigError {
    fn from(e: figment::Error) -> Self {
        Self::Parse(Box::new(e))
    }
}

// ─── root config ────────────────────────────────────────────────────────────

/// The root `gaussclaw.toml` (or upstream `hermes-config.toml`) schema.
///
/// Top-level keys mirror Hermes 1:1. New keys (`caps`, `taint`, `export`,
/// `desktop`) are optional and namespaced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct Config {
    /// Active LLM provider plus model id.
    pub provider: ProviderConfig,

    /// Surface-by-name configuration: `[surfaces.rest]`, `[surfaces.ws]`, ….
    pub surfaces: BTreeMap<String, SurfaceConfig>,

    /// Channel-by-name configuration: `[channels.slack]`, `[channels.discord]`, ….
    pub channels: BTreeMap<String, ChannelConfig>,

    /// Tool-by-name configuration: `[tools.web_search]`, `[tools.shell]`, ….
    pub tools: BTreeMap<String, ToolConfig>,

    // ─── GaussClaw extensions (all optional) ────────────────────────────────
    /// Capability gates (Phase 3).
    pub caps: Option<CapsConfig>,

    /// Taint policy and declassification map (Phase 3).
    pub taint: Option<TaintConfig>,

    /// Trajectory-export options (Phase 5).
    pub export: Option<ExportConfig>,

    /// Desktop / Tauri shell options (Phase 1 / 5).
    pub desktop: Option<DesktopConfig>,

    /// Per-session terminal / executor selection (Sprint 6 §2).
    /// Defaults to `local`.
    #[serde(default)]
    pub terminal: TerminalConfig,

    /// Backend storage location (Sprint 4). Absent / empty path →
    /// ephemeral in-memory store (lost on restart).
    #[serde(default)]
    pub storage: StorageConfig,
}

/// `[storage]` section — Sprint 4.
///
/// Selects where session data, the lineage graph, and the receipt
/// chain are persisted. With an empty `path` (the default) the server
/// uses an ephemeral embedded in-memory store — fine for demos and
/// tests, but everything is lost on restart. Set `path` to a directory
/// to enable the persistent embedded SurrealKV backend (requires the
/// binary to be built with the `kv-surrealkv` feature, which is on by
/// default for `gaussclaw`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct StorageConfig {
    /// Filesystem path to the persistent store. Empty → in-memory.
    pub path: String,
}

/// `[terminal]` section — Sprint 6 §2.
///
/// Selects which `gauss-exec` backend the session dispatches into.
/// The kernel admit gate refuses dispatch into an executor whose
/// `cap:executor:<backend>` isn't in the session grant — this knob
/// is operator intent, not a privilege grant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct TerminalConfig {
    /// Backend tag: `local` | `docker` | `ssh` | `modal`. Defaults to `local`.
    pub backend: TerminalBackend,
}

/// `terminal.backend` value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TerminalBackend {
    /// In-process execution (default).
    #[default]
    Local,
    /// Docker container.
    Docker,
    /// Remote host over SSH.
    Ssh,
    /// Modal sandbox.
    Modal,
}

impl TerminalBackend {
    /// Stable string tag — `local` / `docker` / `ssh` / `modal`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Docker => "docker",
            Self::Ssh => "ssh",
            Self::Modal => "modal",
        }
    }
}

/// Provider plane root.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ProviderConfig {
    /// Provider id, e.g. `anthropic`, `openrouter`, `notdiamond`.
    pub name: String,

    /// Model id, e.g. `claude-3.5-sonnet`, `anthropic/claude-3.5-sonnet`.
    pub model: String,

    /// Optional fallback chain (`chain.fallback = ["openrouter/...", ...]`).
    pub chain: Option<FallbackChainConfig>,
}

/// Provider fallback chain (Phase 4).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct FallbackChainConfig {
    /// Ordered fallback targets; first reachable wins.
    pub fallback: Vec<String>,
}

/// Surface adapter config (REST, WS, OAI-compat, …).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SurfaceConfig {
    /// Bind host (default `127.0.0.1`).
    #[serde(default = "default_host")]
    pub host: String,

    /// Bind port (`0` means "let the OS pick").
    pub port: u16,

    /// Executor backend. `"native"` (Rust), `"shim"` (legacy Hermes Python).
    #[serde(default = "default_backend")]
    pub backend: String,
}

/// Channel adapter config (Slack, Discord, Telegram, …).
///
/// Secret storage is delegated to `gauss-attest`; this struct only carries
/// references (env var names, secret-store ids).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ChannelConfig {
    /// Whether the adapter is enabled at startup.
    pub enabled: bool,

    /// Environment variable carrying the channel auth secret.
    pub secret_env: Option<String>,

    /// Free-form per-adapter options. The owning adapter validates this.
    #[serde(default)]
    pub options: BTreeMap<String, toml::Value>,
}

/// Tool configuration override (final word lives in the Skill Manifest).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolConfig {
    /// Whether this tool is enabled.
    pub enabled: bool,

    /// Force the legacy Python shim path for this tool.
    #[serde(default)]
    pub backend: Option<String>,
}

/// Capability-gate root.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct CapsConfig {
    /// Default capability set granted to a fresh session.
    pub default_grant: Vec<String>,
}

/// Taint policy root.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct TaintConfig {
    /// Declassification policy id. `"default"` (conservative) or `"strict"`.
    #[serde(default = "default_declass")]
    pub default_declass: String,
}

/// Trajectory-export options (Phase 5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ExportConfig {
    /// Taint-aware filter mode: `permissive` | `strict` | `declassified`.
    #[serde(default = "default_filter")]
    pub filter_mode: String,

    /// Whether to emit Cryptographic Trajectory Envelopes alongside SFT.
    #[serde(default = "default_true")]
    pub envelopes: bool,
}

/// Desktop / Tauri shell options.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct DesktopConfig {
    /// Launch a global-hotkey overlay. Default key chord: `Cmd/Ctrl+Shift+H`.
    #[serde(default = "default_true")]
    pub global_hotkey: bool,

    /// Start GaussClaw at login.
    #[serde(default)]
    pub autostart: bool,
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_backend() -> String {
    "native".into()
}
fn default_declass() -> String {
    "default".into()
}
fn default_filter() -> String {
    "declassified".into()
}
const fn default_true() -> bool {
    true
}

// ─── loader ─────────────────────────────────────────────────────────────────

/// Where the loader looked, in order. First hit wins.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LoadReport {
    /// The actual file used.
    pub source: Option<PathBuf>,
    /// Every path the loader probed, in order.
    pub probed: Vec<PathBuf>,
}

/// Load the config from the deterministic search path.
///
/// `override_path = Some(p)` short-circuits the search and fails if `p`
/// is unreadable. Otherwise the search path is:
///
/// 1. `$GAUSSCLAW_CONFIG` (env, absolute path).
/// 2. `$XDG_CONFIG_HOME/gaussclaw/config.toml`
///    (`~/Library/Application Support/gaussclaw/config.toml` on macOS,
///    `%APPDATA%\gaussclaw\config.toml` on Windows).
/// 3. `/etc/gaussclaw/config.toml` (system-wide).
/// 4. `./gaussclaw.toml` (workspace-local; mostly for developers).
///
/// Returns `(config, report)` so callers can log which file was used.
pub fn load(override_path: Option<&Path>) -> Result<(Config, LoadReport), ConfigError> {
    let mut probed = Vec::new();

    let found: Option<PathBuf> = if let Some(p) = override_path {
        probed.push(p.to_path_buf());
        if !p.exists() {
            return Err(ConfigError::NotFound(p.to_path_buf()));
        }
        Some(p.to_path_buf())
    } else {
        search_path().into_iter().find(|candidate| {
            probed.push(candidate.clone());
            candidate.is_file()
        })
    };

    let mut fig = Figment::new();
    if let Some(ref path) = found {
        fig = fig.merge(Toml::file(path));
    }
    // Environment overrides last so secrets and CI overrides win.
    fig = fig.merge(Env::prefixed("GAUSSCLAW_").split("__"));

    let cfg: Config = fig.extract()?;
    Ok((
        cfg,
        LoadReport {
            source: found,
            probed,
        },
    ))
}

/// Compute the search path for `gaussclaw.toml`.
#[must_use]
pub fn search_path() -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Ok(env) = std::env::var("GAUSSCLAW_CONFIG") {
        out.push(PathBuf::from(env));
    }

    if let Some(dirs) = directories::ProjectDirs::from("ai", "gauss", "gaussclaw") {
        out.push(dirs.config_dir().join("config.toml"));
    }

    out.push(PathBuf::from("/etc/gaussclaw/config.toml"));
    out.push(PathBuf::from("./gaussclaw.toml"));
    out
}

/// Serialise a [`Config`] back to TOML and write it to `path`. Creates
/// parent directories as needed.
pub fn save(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(cfg)?;
    std::fs::write(path, body)?;
    Ok(())
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_inert() {
        let c = Config::default();
        assert_eq!(c.provider.name, "");
        assert!(c.surfaces.is_empty());
        assert!(c.caps.is_none());
        assert!(c.taint.is_none());
    }

    #[test]
    fn round_trip_via_toml() {
        let mut cfg = Config::default();
        cfg.provider.name = "anthropic".into();
        cfg.provider.model = "claude-3.5-sonnet".into();
        cfg.surfaces.insert(
            "rest".into(),
            SurfaceConfig {
                host: "127.0.0.1".into(),
                port: 8080,
                backend: "native".into(),
            },
        );
        cfg.caps = Some(CapsConfig {
            default_grant: vec!["fs:read:./data".into(), "network:http_get".into()],
        });
        cfg.export = Some(ExportConfig {
            filter_mode: "declassified".into(),
            envelopes: true,
        });

        let body = toml::to_string_pretty(&cfg).expect("ser");
        let back: Config = toml::from_str(&body).expect("de");
        assert_eq!(cfg, back, "TOML round-trip drift");
    }

    #[test]
    fn hermes_toplevel_keys_load_unchanged() {
        // Mirrors what a Hermes deployment writes today.
        let toml_str = r#"
[provider]
name  = "anthropic"
model = "claude-3.5-sonnet"

[surfaces.rest]
host    = "127.0.0.1"
port    = 8080
backend = "native"

[channels.slack]
enabled    = true
secret_env = "SLACK_BOT_TOKEN"

[tools.web_search]
enabled = true
"#;
        let cfg: Config = toml::from_str(toml_str).expect("hermes-shape parse");
        assert_eq!(cfg.provider.name, "anthropic");
        assert_eq!(cfg.surfaces["rest"].port, 8080);
        assert!(cfg.channels["slack"].enabled);
        assert!(cfg.tools["web_search"].enabled);
        // GaussClaw extensions remain absent — Hermes config is still valid.
        assert!(cfg.caps.is_none());
        assert!(cfg.taint.is_none());
        assert!(cfg.export.is_none());
        assert!(cfg.desktop.is_none());
    }

    #[test]
    fn gaussclaw_extensions_load() {
        let toml_str = r#"
[provider]
name  = "openrouter"
model = "anthropic/claude-3.5-sonnet"

[provider.chain]
fallback = ["anthropic/claude-3.5-sonnet", "openai/gpt-4o"]

[caps]
default_grant = ["fs:read:./data", "network:http_get"]

[taint]
default_declass = "strict"

[export]
filter_mode = "strict"
envelopes   = true

[desktop]
global_hotkey = true
autostart     = false
"#;
        let cfg: Config = toml::from_str(toml_str).expect("ext parse");
        assert_eq!(cfg.provider.chain.as_ref().unwrap().fallback.len(), 2);
        assert_eq!(cfg.caps.as_ref().unwrap().default_grant.len(), 2);
        assert_eq!(cfg.taint.as_ref().unwrap().default_declass, "strict");
        assert_eq!(cfg.export.as_ref().unwrap().filter_mode, "strict");
        assert!(cfg.desktop.as_ref().unwrap().global_hotkey);
    }

    #[test]
    fn terminal_backend_defaults_to_local() {
        let cfg = Config::default();
        assert_eq!(cfg.terminal.backend, TerminalBackend::Local);
        assert_eq!(cfg.terminal.backend.as_str(), "local");
    }

    #[test]
    fn terminal_backend_round_trips_through_toml() {
        for (literal, expected) in [
            ("local", TerminalBackend::Local),
            ("docker", TerminalBackend::Docker),
            ("ssh", TerminalBackend::Ssh),
            ("modal", TerminalBackend::Modal),
        ] {
            let toml_str =
                format!("[provider]\nname=\"x\"\nmodel=\"y\"\n[terminal]\nbackend=\"{literal}\"\n");
            let cfg: Config = toml::from_str(&toml_str).expect("parse");
            assert_eq!(cfg.terminal.backend, expected);
        }
    }

    #[test]
    fn terminal_backend_rejects_unknown_string() {
        let toml_str = r#"
[provider]
name  = "anthropic"
model = "claude-3.5-sonnet"

[terminal]
backend = "wat"
"#;
        let r: Result<Config, _> = toml::from_str(toml_str);
        assert!(r.is_err());
    }

    #[test]
    fn unknown_top_level_keys_are_rejected() {
        let toml_str = r#"
[provider]
name  = "anthropic"
model = "claude-3.5-sonnet"

[unknown_section]
foo = "bar"
"#;
        let r: Result<Config, _> = toml::from_str(toml_str);
        assert!(r.is_err(), "deny_unknown_fields should reject {toml_str:?}");
    }

    #[test]
    fn load_honours_override_path() {
        let tmp = tempfile_path("gaussclaw_load_override");
        std::fs::write(&tmp, "[provider]\nname = \"openai\"\nmodel = \"gpt-4o\"\n").unwrap();
        let (cfg, report) = load(Some(&tmp)).expect("load");
        assert_eq!(cfg.provider.name, "openai");
        assert_eq!(report.source.as_ref(), Some(&tmp));
        assert_eq!(report.probed.len(), 1);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let mut cfg = Config::default();
        cfg.provider.name = "anthropic".into();
        cfg.provider.model = "claude-3.5-sonnet".into();
        let tmp = tempfile_path("gaussclaw_save_roundtrip");
        save(&cfg, &tmp).expect("save");
        let (back, _) = load(Some(&tmp)).expect("load");
        assert_eq!(cfg, back);
        std::fs::remove_file(&tmp).ok();
    }

    fn tempfile_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("{tag}_{}.toml", std::process::id()));
        p
    }
}
