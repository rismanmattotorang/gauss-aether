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
    Cli, Command, ConfigCmd, GatewayCmd, ModelCmd, ReceiptCmd, ToolsCmd, WebArgs,
};
use gaussclaw_tui::StatusInfo;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg_path = cli.config.as_deref();
    let (cfg, report) = match gaussclaw_config::load(cfg_path) {
        Ok((c, r)) => (c, Some(r)),
        Err(_) if cfg_path.is_none() => {
            // No config on disk yet — keep an inert default and continue.
            (gaussclaw_config::Config::default(), None)
        }
        Err(e) => return Err(e.into()),
    };

    if cli.verbose > 0 {
        if let Some(r) = &report {
            if let Some(src) = &r.source {
                eprintln!("config: loaded from {}", src.display());
            } else {
                eprintln!("config: no file found; using defaults");
            }
        }
    }

    match cli.command {
        None => run_default_tui(&cfg),
        Some(Command::Web(args)) => run_web(cfg, report, args),
        Some(cmd) => dispatch(cmd),
    }
}

fn run_web(
    cfg: gaussclaw_config::Config,
    report: Option<gaussclaw_config::LoadReport>,
    args: WebArgs,
) -> anyhow::Result<()> {
    let source = report
        .and_then(|r| r.source)
        .map(|p| p.display().to_string());
    let state = gaussclaw_web::ServerState::new(cfg, source);
    let addr: std::net::SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let url = format!("http://{addr}/");
    eprintln!("gaussclaw web: serving on {url}");
    if args.open {
        eprintln!("note: --open is wired in slice 5 (desktop deep links)");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(gaussclaw_web::serve(addr, state))
}

fn run_default_tui(cfg: &gaussclaw_config::Config) -> anyhow::Result<()> {
    let status = StatusInfo {
        session: "new".into(),
        model: if cfg.provider.name.is_empty() {
            "(unset)".into()
        } else {
            format!("{}/{}", cfg.provider.name, cfg.provider.model)
        },
        turn: 0,
        chain_head: "00000000".into(), // populated in Phase 2 once gaussclaw-store is wired
        taint_floor: "⊥".into(),
        caps: cfg
            .caps
            .as_ref()
            .map_or(0, |c| u32::try_from(c.default_grant.len()).unwrap_or(u32::MAX)),
    };
    gaussclaw_tui::run(status)
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
        Command::Web(_) => unreachable!("`web` is dispatched above in main()"),
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
