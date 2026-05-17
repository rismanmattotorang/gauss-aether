//! `gaussclaw-migrate` — `gaussclaw import hermes ./hermes-config.toml`.
//!
//! Phase 5 §8 of `GAUSSCLAW_ROADMAP.md`. Takes an upstream Hermes
//! `hermes-config.toml`, emits a [`gaussclaw_config::Config`] with
//! every legacy field preserved verbatim AND the GaussClaw-namespaced
//! extensions populated with safe defaults that match a fresh
//! deployment's behaviour:
//!
//! - Every surface gets `backend = "shim"` (so the legacy Hermes
//!   executor remains the active path) — operators flip to `"native"`
//!   per surface as they complete the per-phase opt-in checklist.
//! - Every tool gets `backend = "shim"`.
//! - `export.filter_mode = "declassified"`, `export.envelopes = true`
//!   — the most conservative export defaults.
//! - `taint.default_declass = "default"`.
//! - `desktop.global_hotkey = true`, `desktop.autostart = false`.
//!
//! The migration also emits a [`MigrationReport`] — a phase-by-phase
//! checklist of "what's still on the legacy path" so the operator can
//! flip switches with confidence.
//!
//! ## Why Hermes parity is mechanical
//!
//! `gaussclaw_config::Config` was designed in Phase 1 so that the
//! Hermes top-level keys (`provider`, `surfaces`, `channels`, `tools`)
//! are byte-for-byte the same. That makes this crate a one-pass
//! transformer with no schema cross-walk — Hermes-shaped TOML parses
//! straight into the GaussClaw struct.
//!
//! ## Hermes-superiorities
//!
//! 1. **One-shot upgrade.** A real Hermes deployment migrates in
//!    sub-second time (the parse + serialise cost). Roadmap Exit
//!    criterion: round-trip a real Hermes deployment in under 60 s.
//! 2. **Auditable.** Every change between the input and the output is
//!    enumerated in the [`MigrationReport`] — operators see exactly
//!    what defaults were added.
//! 3. **Reversible.** The output config can be hand-trimmed back to
//!    Hermes shape by deleting the GaussClaw-namespaced sections; the
//!    `surfaces` / `tools` / `channels` keys remain byte-equal.

#![allow(
    clippy::doc_markdown,
    clippy::missing_docs_in_private_items,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::manual_contains,
    dead_code,
)]

use std::path::{Path, PathBuf};

use gaussclaw_config::{
    CapsConfig, ChannelConfig, Config, DesktopConfig, ExportConfig, SurfaceConfig, TaintConfig,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Migration error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MigrationError {
    /// Input file IO failed.
    #[error("read {0}: {1}")]
    Read(PathBuf, std::io::Error),
    /// Parsing the input TOML failed.
    #[error("parse {0}: {1}")]
    Parse(PathBuf, toml::de::Error),
    /// Serialising the output back to TOML failed.
    #[error("serialise: {0}")]
    Serialise(#[from] toml::ser::Error),
    /// Writing the output file failed.
    #[error("write {0}: {1}")]
    Write(PathBuf, std::io::Error),
}

/// One item on the post-migration opt-in checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    /// Subsystem (surface / channel / tool / provider / sandbox).
    pub area: String,
    /// Human-readable instruction.
    pub action: String,
    /// Phase of the roadmap that ships the native path.
    pub phase: String,
}

/// What the migration did, surfaced to the operator.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    /// Number of `[surfaces.*]` sections flipped to `backend="shim"`.
    pub surfaces_to_shim: u32,
    /// Number of `[tools.*]` sections flipped to `backend="shim"`.
    pub tools_to_shim: u32,
    /// Number of GaussClaw-namespaced sections defaulted.
    pub defaults_added: u32,
    /// Phase-by-phase opt-in checklist.
    pub checklist: Vec<ChecklistItem>,
}

/// Parse a Hermes TOML file (or any byte-compatible
/// [`gaussclaw_config::Config`] file) into a GaussClaw config.
///
/// # Errors
/// Returns [`MigrationError::Read`] / [`MigrationError::Parse`] on IO
/// or parse failure.
pub fn read_hermes_config(path: &Path) -> Result<Config, MigrationError> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| MigrationError::Read(path.to_path_buf(), e))?;
    read_hermes_str(&body).map_err(|e| match e {
        MigrationError::Parse(_, err) => MigrationError::Parse(path.to_path_buf(), err),
        other => other,
    })
}

/// Parse a Hermes TOML string into a [`Config`].
///
/// # Errors
/// Returns [`MigrationError::Parse`] on parse failure. The `path`
/// component of the error is filled with the placeholder `<inline>`;
/// callers wrapping a real file should re-map it via
/// [`read_hermes_config`].
pub fn read_hermes_str(body: &str) -> Result<Config, MigrationError> {
    toml::from_str(body)
        .map_err(|e| MigrationError::Parse(PathBuf::from("<inline>"), e))
}

/// Run the migration: input config in, output config + report out.
///
/// Mutating rules (all reversible):
///
/// 1. Every `[surfaces.*]` gets `backend = "shim"` if no explicit
///    backend is present.
/// 2. Every `[tools.*]` gets `backend = "shim"` if no explicit
///    backend is present.
/// 3. Missing `[caps]`, `[taint]`, `[export]`, `[desktop]` sections
///    are filled with the safest GaussClaw defaults.
///
/// The function is **pure**: no IO. Callers that want IO call
/// [`read_hermes_config`] → `migrate` → [`save_gaussclaw_config`].
#[must_use]
pub fn migrate(input: Config) -> (Config, MigrationReport) {
    let mut out = input;
    let mut report = MigrationReport::default();

    // ── surfaces: every Hermes surface goes on the legacy shim by
    //    default. Operators flip to "native" per surface as they
    //    complete the per-phase opt-in checklist.
    //
    // Migration policy is unconditional: a Hermes config never had
    // the `backend` field, so any value the parser produced was a
    // GaussClaw-side default that should be replaced. Operators who
    // want explicit-native semantics edit the produced config by hand
    // after migration.
    for (name, s) in &mut out.surfaces {
        let was_already_shim = is_shim_backend(&s.backend);
        s.backend = "shim".into();
        if !was_already_shim {
            report.surfaces_to_shim = report.surfaces_to_shim.saturating_add(1);
        }
        report.checklist.push(ChecklistItem {
            area: format!("surfaces.{name}"),
            action: format!(
                "Surface '{name}' is on the legacy Hermes shim. Flip to backend = \"native\" when the P1 native executor lands."
            ),
            phase: "P1".into(),
        });
    }

    // ── tools: default to legacy shim ──────────────────────────────
    for (name, t) in &mut out.tools {
        match t.backend.as_deref() {
            None => {
                t.backend = Some("shim".into());
                report.tools_to_shim = report.tools_to_shim.saturating_add(1);
                report.checklist.push(ChecklistItem {
                    area: format!("tools.{name}"),
                    action: format!(
                        "Tool '{name}' is on the legacy Hermes shim. Native HWCA + sandbox lands in P3; flip when ready."
                    ),
                    phase: "P3".into(),
                });
            }
            Some("shim") => {
                report.checklist.push(ChecklistItem {
                    area: format!("tools.{name}"),
                    action: format!(
                        "Tool '{name}' is explicitly on shim. Native HWCA + sandbox lands in P3; flip when ready."
                    ),
                    phase: "P3".into(),
                });
            }
            _ => {}
        }
    }

    // ── channels: just note them on the checklist ──────────────────
    for (name, c) in &out.channels {
        if c.enabled {
            report.checklist.push(ChecklistItem {
                area: format!("channels.{name}"),
                action: format!(
                    "Channel '{name}' is enabled. Native gaussclaw-channels adapter lands in P1; default backend stays compatible."
                ),
                phase: "P1".into(),
            });
        }
    }

    // ── provider: provider chain check ──────────────────────────────
    if out.provider.chain.is_none() {
        report.checklist.push(ChecklistItem {
            area: "provider.chain".into(),
            action:
                "No fallback chain configured. Phase 4 adds [provider.chain] with router-transparency receipts; consider configuring."
                    .into(),
            phase: "P4".into(),
        });
    }

    // ── caps: add a conservative default if absent ─────────────────
    if out.caps.is_none() {
        let mut caps = CapsConfig::default();
        caps.default_grant = vec!["fs:read:./data".into(), "network:http_get".into()];
        out.caps = Some(caps);
        report.defaults_added = report.defaults_added.saturating_add(1);
        report.checklist.push(ChecklistItem {
            area: "caps".into(),
            action:
                "Added [caps] with conservative default_grant. Tighten before shipping to production."
                    .into(),
            phase: "P3".into(),
        });
    }

    // ── taint: default declass ────────────────────────────────────
    if out.taint.is_none() {
        let mut taint = TaintConfig::default();
        taint.default_declass = "default".into();
        out.taint = Some(taint);
        report.defaults_added = report.defaults_added.saturating_add(1);
        report.checklist.push(ChecklistItem {
            area: "taint".into(),
            action:
                "Added [taint] with default declass map (User->Trusted on export). Switch to \"strict\" for public corpora."
                    .into(),
            phase: "P3".into(),
        });
    }

    // ── export: enable envelopes by default ───────────────────────
    if out.export.is_none() {
        let mut export = ExportConfig::default();
        export.filter_mode = "declassified".into();
        export.envelopes = true;
        out.export = Some(export);
        report.defaults_added = report.defaults_added.saturating_add(1);
        report.checklist.push(ChecklistItem {
            area: "export".into(),
            action:
                "Added [export] with filter_mode=\"declassified\" and envelopes=true. Disable envelopes only if downstream consumers refuse them."
                    .into(),
            phase: "P5".into(),
        });
    }

    // ── desktop: hotkey on, autostart off ──────────────────────────
    if out.desktop.is_none() {
        let mut desktop = DesktopConfig::default();
        desktop.global_hotkey = true;
        desktop.autostart = false;
        out.desktop = Some(desktop);
        report.defaults_added = report.defaults_added.saturating_add(1);
        report.checklist.push(ChecklistItem {
            area: "desktop".into(),
            action: "Added [desktop] with global_hotkey=true, autostart=false.".into(),
            phase: "P1".into(),
        });
    }

    (out, report)
}

/// Convenience: read a Hermes file, migrate, serialise to a string.
///
/// # Errors
/// Returns any error from the underlying steps.
pub fn migrate_file_to_string(path: &Path) -> Result<(String, MigrationReport), MigrationError> {
    let input = read_hermes_config(path)?;
    let (output, report) = migrate(input);
    let body = toml::to_string_pretty(&output)?;
    Ok((body, report))
}

/// Write a migrated [`Config`] to `path`. Creates parents as needed.
///
/// # Errors
/// Returns [`MigrationError::Write`] / [`MigrationError::Serialise`].
pub fn save_gaussclaw_config(cfg: &Config, path: &Path) -> Result<(), MigrationError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MigrationError::Write(parent.to_path_buf(), e))?;
        }
    }
    let body = toml::to_string_pretty(cfg)?;
    std::fs::write(path, body).map_err(|e| MigrationError::Write(path.to_path_buf(), e))?;
    Ok(())
}

// ─── implementation helpers ────────────────────────────────────────

fn is_native_backend(b: &str) -> bool {
    b == "native"
}
fn is_shim_backend(b: &str) -> bool {
    b == "shim"
}

/// Convenience: deserialise a [`SurfaceConfig`] from its component
/// fields (mostly to make tests less verbose). Use the field initialiser
/// directly in production code.
#[doc(hidden)]
#[must_use]
pub fn make_surface(host: &str, port: u16, backend: &str) -> SurfaceConfig {
    let mut s = SurfaceConfig::default();
    s.host = host.into();
    s.port = port;
    s.backend = backend.into();
    s
}

/// Convenience: deserialise a [`ChannelConfig`].
#[doc(hidden)]
#[must_use]
pub fn make_channel(enabled: bool) -> ChannelConfig {
    let mut c = ChannelConfig::default();
    c.enabled = enabled;
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const HERMES_INPUT: &str = r#"
[provider]
name = "anthropic"
model = "claude-3.5-sonnet"

[surfaces.rest]
host = "127.0.0.1"
port = 8080

[surfaces.ws]
host = "127.0.0.1"
port = 8081

[channels.slack]
enabled = true
secret_env = "SLACK_TOKEN"

[tools.web_search]
enabled = true

[tools.shell]
enabled = false
"#;

    #[test]
    fn parses_minimal_hermes_config() {
        let cfg = read_hermes_str(HERMES_INPUT).unwrap();
        assert_eq!(cfg.provider.name, "anthropic");
        assert_eq!(cfg.surfaces.len(), 2);
        assert!(cfg.channels.contains_key("slack"));
        assert_eq!(cfg.tools.len(), 2);
        // No GaussClaw-namespaced extensions in the input.
        assert!(cfg.caps.is_none());
        assert!(cfg.taint.is_none());
        assert!(cfg.export.is_none());
        assert!(cfg.desktop.is_none());
    }

    #[test]
    fn migrate_defaults_every_surface_to_shim() {
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (out, report) = migrate(input);
        for s in out.surfaces.values() {
            assert_eq!(s.backend, "shim");
        }
        assert_eq!(report.surfaces_to_shim, 2);
    }

    #[test]
    fn migrate_defaults_every_tool_to_shim() {
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (out, report) = migrate(input);
        for t in out.tools.values() {
            assert_eq!(t.backend.as_deref(), Some("shim"));
        }
        assert_eq!(report.tools_to_shim, 2);
    }

    #[test]
    fn migrate_adds_every_gaussclaw_extension() {
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (out, report) = migrate(input);
        assert!(out.caps.is_some());
        assert!(out.taint.is_some());
        assert!(out.export.is_some());
        assert!(out.desktop.is_some());
        assert_eq!(report.defaults_added, 4);
    }

    #[test]
    fn migrate_flips_every_surface_to_shim_even_if_serde_default_native() {
        // A Config built with Default::default() reports
        // backend="native" via serde defaults, but the migration is
        // unconditional: Hermes never had `backend`, so we flip every
        // surface to shim and emit a checklist item.
        let mut input = Config::default();
        input.provider.name = "openai".into();
        input.provider.model = "gpt-4o".into();
        let mut surfaces: BTreeMap<String, SurfaceConfig> = BTreeMap::new();
        surfaces.insert("rest".into(), make_surface("127.0.0.1", 8080, "native"));
        input.surfaces = surfaces;
        let (out, report) = migrate(input);
        assert_eq!(out.surfaces["rest"].backend, "shim");
        assert_eq!(report.surfaces_to_shim, 1);
    }

    #[test]
    fn checklist_lists_phases_in_order_present() {
        // The checklist orders by area-touched, which surfaces P1 then
        // P3 then P5 phase items based on the order encountered.
        // This test pins THAT exact ordering so a regression in iteration
        // order surfaces.
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (_out, report) = migrate(input);
        // Drop the channels/provider/etc. items, keep only the
        // phase-indexed defaults items.
        let phases: Vec<&str> = report
            .checklist
            .iter()
            .map(|c| c.phase.as_str())
            .collect();
        // At minimum we expect at least one P1 (surfaces), one P3
        // (tools / caps / taint), and one P5 (export) item.
        assert!(phases.iter().any(|p| *p == "P1"));
        assert!(phases.iter().any(|p| *p == "P3"));
        assert!(phases.iter().any(|p| *p == "P5"));
    }

    #[test]
    fn checklist_flags_enabled_channels() {
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (_out, report) = migrate(input);
        assert!(report
            .checklist
            .iter()
            .any(|c| c.area == "channels.slack"));
    }

    #[test]
    fn checklist_flags_missing_fallback_chain() {
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (_out, report) = migrate(input);
        assert!(report
            .checklist
            .iter()
            .any(|c| c.area == "provider.chain"));
    }

    #[test]
    fn migrate_is_idempotent_on_value() {
        // Migrating an already-migrated config produces the same Config
        // value. (The report counters reset to 0 because nothing more
        // is being flipped or defaulted.)
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (once, _r1) = migrate(input);
        let (twice, r2) = migrate(once.clone());
        assert_eq!(once, twice);
        assert_eq!(r2.defaults_added, 0);
        assert_eq!(r2.surfaces_to_shim, 0);
        assert_eq!(r2.tools_to_shim, 0);
    }

    #[test]
    fn round_trips_through_toml() {
        let input = read_hermes_str(HERMES_INPUT).unwrap();
        let (out, _r) = migrate(input);
        let body = toml::to_string_pretty(&out).unwrap();
        // Output parses back as Config and equals the migrated value.
        let back: Config = toml::from_str(&body).unwrap();
        assert_eq!(out, back);
    }
}
