//! The shipping `gaussclaw` binary.
//!
//! Phase 1 wires the clap subcommand surface from `gaussclaw-cli` to a
//! stub dispatcher. Each subcommand prints which phase will replace its
//! body. See `GAUSSCLAW_ROADMAP.md` Phase 1 Task 2 ("CLI subcommand parity").

#![allow(
    clippy::needless_pass_by_value,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::doc_markdown,
    clippy::option_if_let_else,
    clippy::unnecessary_wraps,
)]

use clap::Parser;
use gaussclaw_cli::{
    Cli, Command, ConfigCmd, GatewayCmd, ModelCmd, ReceiptCmd, ToolsCmd,
};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => run_default_tui(),
        Some(cmd) => dispatch(cmd),
    }
}

fn run_default_tui() -> anyhow::Result<()> {
    println!(
        "gaussclaw {} — TUI launcher\n\
         \n\
         The Ratatui + crossterm TUI lands later in Phase 1 (Task 3).\n\
         Use `gaussclaw --help` to see the implemented subcommand surface.\n",
        env!("CARGO_PKG_VERSION"),
    );
    Ok(())
}

fn dispatch(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Chat(_args) => stub("chat", 1, "gaussclaw-agent + gaussclaw-providers"),
        Command::Model(sub) => match sub {
            ModelCmd::List => stub("model list", 4, "gaussclaw-providers + gaussclaw-providers-meta"),
            ModelCmd::Show => stub("model show", 4, "gaussclaw-providers"),
            ModelCmd::Set { .. } => stub("model set", 4, "gaussclaw-providers"),
        },
        Command::Tools(sub) => match sub {
            ToolsCmd::List => stub("tools list", 3, "gaussclaw-tools + gaussclaw-skill"),
            ToolsCmd::Show { .. } => stub("tools show", 3, "gaussclaw-skill"),
            ToolsCmd::Enable { .. } => stub("tools enable", 3, "gaussclaw-skill + gaussclaw-config"),
            ToolsCmd::Disable { .. } => stub("tools disable", 3, "gaussclaw-skill + gaussclaw-config"),
        },
        Command::Config(sub) => match sub {
            ConfigCmd::List => stub("config list", 1, "gaussclaw-config"),
            ConfigCmd::Get { .. } => stub("config get", 1, "gaussclaw-config"),
            ConfigCmd::Set { .. } => stub("config set", 1, "gaussclaw-config"),
            ConfigCmd::Path => stub("config path", 1, "gaussclaw-config"),
        },
        Command::Gateway(sub) => match sub {
            GatewayCmd::Start => stub("gateway start", 1, "gaussclaw-channels + gauss-gateway"),
            GatewayCmd::Stop => stub("gateway stop", 1, "gaussclaw-channels"),
            GatewayCmd::Status => stub("gateway status", 1, "gaussclaw-channels"),
        },
        Command::Setup(_) => stub("setup", 1, "gaussclaw-config + gaussclaw-migrate"),
        Command::Update(_) => stub("update", 5, "gaussclaw-desktop updater + gauss-attest"),
        Command::Doctor(_) => stub("doctor", 1, "gauss-health (SDHE invariants)"),
        Command::Import(args) => stub_import(args.hermes_config),
        Command::Receipt(sub) => match sub {
            ReceiptCmd::Head => stub("receipt head", 2, "gauss-audit + gaussclaw-store"),
            ReceiptCmd::Verify { .. } => stub("receipt verify", 5, "gaussclaw-export envelope verifier"),
        },
    }
}

fn stub(name: &str, phase: u8, crates: &str) -> anyhow::Result<()> {
    eprintln!(
        "gaussclaw {name}: not yet implemented.\n\
         \n  Lands in Phase {phase} of GAUSSCLAW_ROADMAP.md.\n  \
         Implementing crates: {crates}\n"
    );
    Ok(())
}

fn stub_import(path: std::path::PathBuf) -> anyhow::Result<()> {
    eprintln!(
        "gaussclaw import: not yet implemented.\n\
         \n  Would read Hermes config from: {}\n  \
         Lands in Phase 1 Task 8 of GAUSSCLAW_ROADMAP.md.\n  \
         Implementing crate: gaussclaw-migrate\n",
        path.display()
    );
    Ok(())
}
