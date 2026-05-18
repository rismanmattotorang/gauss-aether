//! CLI parity tests — assert that the `gaussclaw` subcommand surface
//! covers every Hermes subcommand 1:1 and locks the `--help` output as
//! an `insta` snapshot so accidental drift fails CI.
//!
//! Snapshots live in `crates/gaussclaw-conformance/src/snapshots/`. Run
//! `cargo insta accept -p gaussclaw-conformance` after an intentional CLI
//! change.

#![allow(clippy::doc_markdown)]

/// The set of Hermes subcommands GaussClaw must preserve.
///
/// Sourced from upstream `hermes --help` at the version pinned in
/// `docs/HERMES_ADAPTER_MATRIX.md`. Any addition or removal in upstream
/// Hermes is reflected here in lock-step.
pub const HERMES_SUBCOMMANDS: &[&str] = &[
    "model", "tools", "config", "gateway", "setup", "update", "doctor",
    // `cron` is Hermes parity — Hermes's `hermes cron` ships scheduled
    // job management (per the parity matrix in `/ROADMAP.md`). The
    // GaussClaw implementation lands as Sprint 5 §1.
    "cron",
    // `web` is a GaussClaw extension — see SUBCOMMANDS for the parity flag.
];

#[cfg(test)]
mod tests {
    use super::HERMES_SUBCOMMANDS;
    use clap::CommandFactory;
    use gaussclaw_cli::{Cli, SUBCOMMANDS};

    /// Render the top-level help to a string, normalising the width so the
    /// snapshot is stable across terminals.
    fn render_help() -> String {
        let mut cmd = Cli::command().term_width(100);
        let mut out = Vec::new();
        cmd.write_help(&mut out).expect("write_help");
        String::from_utf8(out).expect("utf-8")
    }

    /// Render the help of a single subcommand.
    fn render_subcommand_help(name: &str) -> String {
        let cli = Cli::command().term_width(100);
        let sub = cli
            .find_subcommand(name)
            .unwrap_or_else(|| panic!("subcommand {name} not found"))
            .clone();
        let mut sub = sub.term_width(100);
        let mut out = Vec::new();
        sub.write_long_help(&mut out).expect("write_long_help");
        String::from_utf8(out).expect("utf-8")
    }

    /// Every Hermes subcommand must appear in the GaussClaw surface.
    /// GaussClaw extensions are permitted; missing Hermes commands are not.
    #[test]
    fn hermes_subcommands_are_all_covered() {
        let claw: std::collections::BTreeSet<&str> = SUBCOMMANDS.iter().map(|(n, _)| *n).collect();
        let missing: Vec<&&str> = HERMES_SUBCOMMANDS
            .iter()
            .filter(|h| !claw.contains(*h))
            .collect();
        assert!(
            missing.is_empty(),
            "Hermes subcommands not yet ported: {missing:?}"
        );
    }

    /// Every entry flagged as Hermes parity in `SUBCOMMANDS` must actually
    /// be in the upstream Hermes list. Prevents accidental over-claim.
    #[test]
    fn parity_flags_are_truthful() {
        let hermes: std::collections::BTreeSet<&str> = HERMES_SUBCOMMANDS.iter().copied().collect();
        for (name, claims_parity) in SUBCOMMANDS {
            if *claims_parity {
                assert!(
                    hermes.contains(name),
                    "`{name}` claims Hermes parity but is not in HERMES_SUBCOMMANDS"
                );
            } else {
                assert!(
                    !hermes.contains(name),
                    "`{name}` is in HERMES_SUBCOMMANDS but is flagged as a GaussClaw extension"
                );
            }
        }
    }

    /// Lock the top-level help text. Changes require `cargo insta accept`.
    #[test]
    fn top_level_help_snapshot() {
        insta::assert_snapshot!("gaussclaw_help", render_help());
    }

    /// Lock the help text for every subcommand.
    #[test]
    fn subcommand_help_snapshots() {
        for (name, _) in SUBCOMMANDS {
            let body = render_subcommand_help(name);
            insta::assert_snapshot!(format!("subcommand_{name}_help"), body);
        }
    }
}
