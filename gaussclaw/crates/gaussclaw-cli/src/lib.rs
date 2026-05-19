//! `gaussclaw-cli` — clap v4 subcommand surface for the `gaussclaw` binary.
//!
//! Provides 1:1 Hermes CLI parity (`model`, `tools`, `config`, `gateway`,
//! `setup`, `update`, `doctor`) plus port-specific extensions (`chat`,
//! `import`, `receipt`). See Phase 1 Task 2 of `GAUSSCLAW_ROADMAP.md`.
//!
//! The crate is intentionally sync: parsing happens here, the async
//! runtime lives in `gaussclaw-bin`. Each subcommand currently delegates
//! to a stub handler that prints the phase that will fill it in; the
//! parity fixture (`gaussclaw --help`) is locked under
//! `gaussclaw-conformance::cli_parity`.

#![allow(clippy::module_name_repetitions, clippy::doc_markdown)]

use clap::{Parser, Subcommand};

/// Top-level CLI.
///
/// Running `gaussclaw` with no subcommand launches the TUI (Phase 1
/// deliverable). All other entry points are subcommands below.
#[derive(Debug, Parser)]
#[command(
    name = "gaussclaw",
    bin_name = "gaussclaw",
    version,
    about = "GaussClaw — Hermes-compatible agent on the Gauss-Aether runtime.",
    long_about = "GaussClaw is a Rust port of the Hermes agent atop the \
                  Gauss-Aether axiomatic kernel.\n\n\
                  Run with no subcommand to launch the TUI. See the \
                  per-subcommand --help for argument details. The full \
                  development plan lives in GAUSSCLAW_ROADMAP.md.",
    propagate_version = true,
    arg_required_else_help = false
)]
pub struct Cli {
    /// Path to an alternate `gaussclaw.toml`. If omitted, the loader walks
    /// the standard search path (cwd → `$XDG_CONFIG_HOME/gaussclaw/` →
    /// platform default).
    #[arg(short = 'c', long = "config", global = true, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,

    /// Increase log verbosity. May be repeated (-v, -vv, -vvv).
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Quiet mode: suppress non-error output.
    #[arg(short = 'q', long = "quiet", global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// The subcommand to dispatch. `None` means launch the TUI.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// One-shot chat REPL without launching the full TUI (GaussClaw extension).
    Chat(ChatArgs),

    /// Select or inspect the active LLM model. (Hermes parity: `hermes model`.)
    #[command(subcommand)]
    Model(ModelCmd),

    /// Enable, disable, or list tools. (Hermes parity: `hermes tools`.)
    #[command(subcommand)]
    Tools(ToolsCmd),

    /// Read or write configuration values. (Hermes parity: `hermes config`.)
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Start, stop, or inspect the messaging gateway. (Hermes parity: `hermes gateway`.)
    #[command(subcommand)]
    Gateway(GatewayCmd),

    /// Interactive first-run setup wizard. (Hermes parity: `hermes setup`.)
    Setup(SetupArgs),

    /// Update GaussClaw to the latest signed release. (Hermes parity: `hermes update`.)
    Update(UpdateArgs),

    /// Run the Self-Diagnostic Health Engine invariants. (Hermes parity: `hermes doctor`.)
    Doctor(DoctorArgs),

    /// Import a Hermes config and migrate it to GaussClaw (GaussClaw extension).
    Import(ImportArgs),

    /// Inspect the receipt chain head or verify an envelope (GaussClaw extension).
    #[command(subcommand)]
    Receipt(ReceiptCmd),

    /// Manage scheduled cron jobs (Sprint 5 §1).
    #[command(subcommand)]
    Cron(CronCmd),

    /// Capture or restore working-directory snapshots (Sprint 5 §8).
    #[command(subcommand)]
    Snapshot(SnapshotCmd),

    /// Launch the web dashboard (Axum + retained React frontend) (GaussClaw extension).
    Web(WebArgs),
}

// ─── cron ───────────────────────────────────────────────────────────────────

/// `cron` subcommand variants.
#[derive(Debug, clap::Subcommand)]
pub enum CronCmd {
    /// List every scheduled job (id, label, schedule, status,
    /// next-fire-at, last-fired-at, fire count).
    List,
    /// Add a new job. Schedule grammar: a duration (`30m`, `2h15m`),
    /// a 5-field cron expression (`*/15 * * * *`), or an RFC 3339
    /// timestamp (`2026-05-20T14:30:00Z`).
    Add {
        /// Schedule grammar.
        #[arg(value_name = "SCHEDULE")]
        schedule: String,
        /// Free-text label that surfaces in `cron list` output.
        #[arg(short = 'l', long = "label", default_value = "(unlabeled)")]
        label: String,
        /// Inline JSON payload the job's fire callback receives.
        #[arg(long = "payload", default_value = "null")]
        payload: String,
    },
    /// Pause a job. `next_fire_at` is preserved; resuming a paused
    /// job fires immediately if its fire-time was reached during
    /// the pause window.
    Pause {
        /// Job id (from `cron list`).
        #[arg(value_name = "ID")]
        id: u64,
    },
    /// Resume a paused job.
    Resume {
        /// Job id (from `cron list`).
        #[arg(value_name = "ID")]
        id: u64,
    },
    /// Remove a job from the scheduler.
    Remove {
        /// Job id (from `cron list`).
        #[arg(value_name = "ID")]
        id: u64,
    },
    /// Show one job's full record (status, payload, fire history).
    Status {
        /// Job id (from `cron list`).
        #[arg(value_name = "ID")]
        id: u64,
    },
    /// Edit a job's label and/or schedule. Pass `--label` to rename, or
    /// `--schedule` to change cadence. Changing the schedule recomputes
    /// `next_fire_at` from the new grammar against the current clock.
    Edit {
        /// Job id (from `cron list`).
        #[arg(value_name = "ID")]
        id: u64,
        /// New label.
        #[arg(short = 'l', long = "label")]
        label: Option<String>,
        /// New schedule (same grammar as `cron add`).
        #[arg(short = 's', long = "schedule")]
        schedule: Option<String>,
    },
    /// Fire a job immediately, bypassing its scheduled time. The
    /// cap-gate is still applied: a job whose `payload_caps` aren't in
    /// the live kernel grant is refused (and marked `Failed`).
    Run {
        /// Job id (from `cron list`).
        #[arg(value_name = "ID")]
        id: u64,
    },
}

// ─── snapshot ───────────────────────────────────────────────────────────────

/// `snapshot` subcommand variants. Sprint 5 §8.
///
/// Hermes ships `checkpoint_manager`; GaussClaw separates the two
/// caps (`cap:checkpoint:write` vs `cap:checkpoint:rollback`) so a
/// write-only session can still capture state.
#[derive(Debug, clap::Subcommand)]
pub enum SnapshotCmd {
    /// Capture the live working-directory state under a label.
    Save {
        /// Free-text label that surfaces in `snapshot list`.
        #[arg(short = 'l', long = "label", default_value = "(unlabeled)")]
        label: String,
        /// Paths to capture (relative to `--root`). Repeat to add more.
        #[arg(short = 'p', long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Root directory; defaults to the current working directory.
        #[arg(long = "root", value_name = "DIR")]
        root: Option<String>,
    },
    /// List every retained snapshot (id, label, file count, size).
    List,
    /// Print one snapshot's full record.
    Status {
        /// Snapshot id (from `snapshot list`).
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Restore the working directory from a captured snapshot.
    /// Requires `cap:checkpoint:rollback`.
    Restore {
        /// Snapshot id (from `snapshot list`).
        #[arg(value_name = "ID")]
        id: String,
        /// Root directory; defaults to the current working directory.
        #[arg(long = "root", value_name = "DIR")]
        root: Option<String>,
    },
    /// Drop a snapshot from the store.
    Remove {
        /// Snapshot id (from `snapshot list`).
        #[arg(value_name = "ID")]
        id: String,
    },
}

// ─── web ────────────────────────────────────────────────────────────────────

/// `web` arguments.
#[derive(Debug, Parser)]
pub struct WebArgs {
    /// Bind host. Defaults to `127.0.0.1`.
    #[arg(long = "host", default_value = "127.0.0.1")]
    pub host: String,

    /// Bind port. `0` lets the OS pick a free port.
    #[arg(short = 'p', long = "port", default_value_t = 8642)]
    pub port: u16,

    /// Open the dashboard URL in the default browser after the server is up.
    #[arg(long = "open")]
    pub open: bool,
}

// ─── chat ───────────────────────────────────────────────────────────────────

/// One-shot chat REPL arguments.
#[derive(Debug, Parser)]
pub struct ChatArgs {
    /// Send a single message and exit (non-interactive mode).
    #[arg(short = 'm', long = "message", value_name = "TEXT")]
    pub message: Option<String>,

    /// Use a specific session ID instead of starting a new one.
    #[arg(short = 's', long = "session", value_name = "ID")]
    pub session: Option<String>,
}

// ─── model ──────────────────────────────────────────────────────────────────

/// `model` subcommand variants.
#[derive(Debug, Subcommand)]
pub enum ModelCmd {
    /// List every model the configured providers expose.
    List,
    /// Print the currently-selected model.
    Show,
    /// Set the active model (e.g. `anthropic/claude-3.5-sonnet`).
    Set {
        /// Fully-qualified model id `<provider>/<model>`.
        #[arg(value_name = "MODEL_ID")]
        model_id: String,
    },
}

// ─── tools ──────────────────────────────────────────────────────────────────

/// `tools` subcommand variants.
#[derive(Debug, Subcommand)]
pub enum ToolsCmd {
    /// List every registered tool and its Skill Manifest summary.
    List,
    /// Show the full Skill Manifest for one tool.
    Show {
        /// Tool name as declared in its Skill Manifest.
        #[arg(value_name = "TOOL")]
        tool: String,
    },
    /// Enable a tool for the current session and persist the choice.
    Enable {
        /// Tool name.
        #[arg(value_name = "TOOL")]
        tool: String,
    },
    /// Disable a tool for the current session and persist the choice.
    Disable {
        /// Tool name.
        #[arg(value_name = "TOOL")]
        tool: String,
    },
}

// ─── config ─────────────────────────────────────────────────────────────────

/// `config` subcommand variants.
#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Print every configured key and its current value.
    List,
    /// Read a single configuration key.
    Get {
        /// Dotted key path (e.g. `provider.name`).
        #[arg(value_name = "KEY")]
        key: String,
    },
    /// Write a single configuration key.
    Set {
        /// Dotted key path (e.g. `provider.model`).
        #[arg(value_name = "KEY")]
        key: String,
        /// New value (parsed as TOML; quote strings).
        #[arg(value_name = "VALUE")]
        value: String,
    },
    /// Print the path of the active `gaussclaw.toml`.
    Path,
}

// ─── gateway ────────────────────────────────────────────────────────────────

/// `gateway` subcommand variants.
#[derive(Debug, Subcommand)]
pub enum GatewayCmd {
    /// Start the messaging gateway in the foreground.
    Start,
    /// Stop the running gateway daemon.
    Stop,
    /// Show whether the gateway is running and which channels are attached.
    Status,
}

// ─── setup ──────────────────────────────────────────────────────────────────

/// `setup` arguments.
#[derive(Debug, Parser)]
pub struct SetupArgs {
    /// Skip interactive prompts and accept all defaults.
    #[arg(long = "non-interactive")]
    pub non_interactive: bool,
}

// ─── update ─────────────────────────────────────────────────────────────────

/// `update` arguments.
#[derive(Debug, Parser)]
pub struct UpdateArgs {
    /// Release channel.
    #[arg(long = "channel", value_name = "CHANNEL", default_value = "stable")]
    pub channel: String,

    /// Show what would be installed without performing the upgrade.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

// ─── doctor ─────────────────────────────────────────────────────────────────

/// `doctor` arguments.
#[derive(Debug, Parser)]
pub struct DoctorArgs {
    /// Emit machine-readable JSON instead of the human-readable summary.
    #[arg(long = "json")]
    pub json: bool,
}

// ─── import ─────────────────────────────────────────────────────────────────

/// `import` arguments.
#[derive(Debug, Parser)]
pub struct ImportArgs {
    /// Path to the Hermes config file.
    #[arg(value_name = "HERMES_CONFIG")]
    pub hermes_config: std::path::PathBuf,

    /// Write the converted `gaussclaw.toml` to this path instead of the default.
    #[arg(short = 'o', long = "output", value_name = "PATH")]
    pub output: Option<std::path::PathBuf>,
}

// ─── receipt ────────────────────────────────────────────────────────────────

/// `receipt` subcommand variants.
#[derive(Debug, Subcommand)]
pub enum ReceiptCmd {
    /// Print the current receipt-chain head and its TSA-anchor proof.
    Head,
    /// Verify a Cryptographic Trajectory Envelope file.
    Verify {
        /// Path to a `.env` envelope file.
        #[arg(value_name = "ENVELOPE")]
        envelope: std::path::PathBuf,
    },
}

/// Stable string id every subcommand returns; used by the conformance suite
/// to assert that the dispatch table covers exactly the Hermes-parity set.
#[must_use]
pub const fn dispatch_id(cmd: &Command) -> &'static str {
    match cmd {
        Command::Chat(_) => "chat",
        Command::Model(_) => "model",
        Command::Tools(_) => "tools",
        Command::Config(_) => "config",
        Command::Gateway(_) => "gateway",
        Command::Setup(_) => "setup",
        Command::Update(_) => "update",
        Command::Doctor(_) => "doctor",
        Command::Import(_) => "import",
        Command::Receipt(_) => "receipt",
        Command::Cron(_) => "cron",
        Command::Snapshot(_) => "snapshot",
        Command::Web(_) => "web",
    }
}

/// Canonical list of subcommand ids — kept in sync with `Command` by a
/// conformance test. The `(id, hermes_parity)` tuple records whether the
/// upstream Hermes CLI ships the same subcommand.
pub const SUBCOMMANDS: &[(&str, bool)] = &[
    ("chat", false),
    ("model", true),
    ("tools", true),
    ("config", true),
    ("gateway", true),
    ("setup", true),
    ("update", true),
    ("doctor", true),
    ("import", false),
    ("receipt", false),
    ("cron", true),
    ("snapshot", false),
    ("web", false),
];

#[cfg(test)]
mod tests {
    use super::{dispatch_id, Cli, Command, SUBCOMMANDS};
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_parses_with_no_args() {
        let cli = Cli::try_parse_from(["gaussclaw"]).expect("no-arg parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn every_subcommand_in_the_table_parses() {
        let leaf: &[(&str, &[&str])] = &[
            ("chat", &["gaussclaw", "chat"]),
            ("model", &["gaussclaw", "model", "list"]),
            ("tools", &["gaussclaw", "tools", "list"]),
            ("config", &["gaussclaw", "config", "list"]),
            ("gateway", &["gaussclaw", "gateway", "status"]),
            ("setup", &["gaussclaw", "setup"]),
            ("update", &["gaussclaw", "update"]),
            ("doctor", &["gaussclaw", "doctor"]),
            ("import", &["gaussclaw", "import", "/tmp/cfg.toml"]),
            ("receipt", &["gaussclaw", "receipt", "head"]),
            ("cron", &["gaussclaw", "cron", "list"]),
            ("snapshot", &["gaussclaw", "snapshot", "list"]),
            ("web", &["gaussclaw", "web"]),
        ];
        for (id, argv) in leaf {
            let parsed = Cli::try_parse_from(argv.iter().copied())
                .unwrap_or_else(|e| panic!("subcommand {id} failed to parse: {e}"));
            let cmd: Command = parsed.command.unwrap_or_else(|| panic!("{id}: empty"));
            assert_eq!(dispatch_id(&cmd), *id, "dispatch_id drift for {id}");
        }
    }

    #[test]
    fn subcommand_table_matches_command_factory() {
        let cmd = Cli::command();
        let clap_names: std::collections::BTreeSet<&str> =
            cmd.get_subcommands().map(clap::Command::get_name).collect();
        let table_names: std::collections::BTreeSet<&str> =
            SUBCOMMANDS.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            clap_names, table_names,
            "SUBCOMMANDS drifted from the clap-derived surface"
        );
    }
}
