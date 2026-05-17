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
    Cli, Command, ConfigCmd, DoctorArgs, GatewayCmd, ImportArgs, ModelCmd, ReceiptCmd, ToolsCmd,
    WebArgs,
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
    let addr: std::net::SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let url = format!("http://{addr}/");
    eprintln!("gaussclaw web: serving on {url}");
    if args.open {
        eprintln!("note: --open is wired in slice 5 (desktop deep links)");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        // Phase 2 wiring: build a single Arc<SessionStore> that backs
        // /api/sessions, /api/receipt/head, and the chat WebSocket.
        // In-memory backend for the demo binary; production deployments
        // swap to a persistent SurrealMemory via SessionStore::with_memory.
        let store = std::sync::Arc::new(
            gaussclaw_store::SessionStore::open_in_memory().await?,
        );
        let state = gaussclaw_web::ServerState::new(cfg, source).with_store(store);
        gaussclaw_web::serve(addr, state).await
    })
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
        Command::Doctor(args) => run_doctor(args),
        Command::Import(args) => run_import(args),
        Command::Receipt(sub) => match sub {
            ReceiptCmd::Head => stub("receipt head", 2, "gauss-audit + gaussclaw-store"),
            ReceiptCmd::Verify { envelope } => run_receipt_verify(envelope),
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

// ─── Phase 5: doctor / import / receipt verify ─────────────────────────────

fn run_doctor(args: DoctorArgs) -> anyhow::Result<()> {
    // Build the SDHE with the SPECS-default invariant set and evaluate
    // against a minimal subject. Production deployments swap the subject
    // for one that owns Arc<dyn Kernel> + Arc<SessionStore> + signer.
    use gauss_health::{HealthEngine, HealthReport};
    let engine = HealthEngine::with_specs_defaults();
    let subject = DefaultSubject;
    let report: HealthReport = engine.evaluate(&subject);
    if args.json {
        let body = serde_json::to_string_pretty(&report)?;
        println!("{body}");
    } else {
        let pass = report
            .invariants
            .iter()
            .filter(|o| matches!(o.verdict, gauss_health::Verdict::Ok))
            .count();
        let warn = report
            .invariants
            .iter()
            .filter(|o| matches!(o.verdict, gauss_health::Verdict::Warning))
            .count();
        let fail = report
            .invariants
            .iter()
            .filter(|o| matches!(o.verdict, gauss_health::Verdict::Failing))
            .count();
        println!(
            "gaussclaw doctor: {} invariants — {pass} ok, {warn} warning, {fail} failing",
            report.invariants.len()
        );
        for o in &report.invariants {
            let marker = match o.verdict {
                gauss_health::Verdict::Ok => "ok",
                gauss_health::Verdict::Warning => "warn",
                gauss_health::Verdict::Failing => "fail",
                _ => "?",
            };
            println!("  [{marker}] {} — {}", o.id, o.description);
            if let Some(d) = &o.detail {
                println!("      {d}");
            }
        }
        if report.has_failure() {
            eprintln!("\nat least one invariant is FAILING; exiting with code 1");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Minimal HealthSubject impl that reports a clean baseline so the
/// SPECS-default invariants exercise their `Ok` branch on a fresh
/// install. Real deployments swap this for a subject backed by the
/// runtime kernel / store / signer.
struct DefaultSubject;

#[async_trait::async_trait]
impl gauss_health::HealthSubject for DefaultSubject {
    async fn chain_head(&self) -> Option<(u64, [u8; 32])> {
        Some((0, [0u8; 32]))
    }
    fn current_grant(&self) -> u64 {
        u64::MAX // permissive default
    }
    fn live_worker_count(&self) -> u32 {
        0
    }
    fn signer_present(&self) -> bool {
        false
    }
    fn sag_present(&self) -> bool {
        false
    }
    fn sandbox_present(&self) -> bool {
        true
    }
    async fn memory_non_empty(&self) -> bool {
        false
    }
}

fn run_import(args: ImportArgs) -> anyhow::Result<()> {
    let (body, report) = gaussclaw_migrate::migrate_file_to_string(&args.hermes_config)?;
    println!("{body}");
    eprintln!("\n── migration report ──");
    eprintln!(
        "  surfaces flipped to shim: {}\n  tools flipped to shim:    {}\n  defaults added:           {}",
        report.surfaces_to_shim, report.tools_to_shim, report.defaults_added
    );
    if !report.checklist.is_empty() {
        eprintln!("\n── opt-in checklist ──");
        for item in &report.checklist {
            eprintln!("  [{}] {}: {}", item.phase, item.area, item.action);
        }
    }
    Ok(())
}

fn run_receipt_verify(envelope_path: std::path::PathBuf) -> anyhow::Result<()> {
    let body = std::fs::read(&envelope_path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", envelope_path.display()))?;
    let envelope: gaussclaw_export::Envelope =
        serde_json::from_slice(&body).map_err(|e| anyhow::anyhow!("parse envelope: {e}"))?;
    // Verify under the envelope's embedded publisher key. Strict
    // production deployments pin a known key via --pin-key (not wired
    // here in the demo binary).
    gaussclaw_export::verify_envelope(&envelope, None, None)
        .map_err(|e| anyhow::anyhow!("verify_envelope: {e}"))?;
    println!(
        "ok: envelope verifies\n  publisher_pk: {}\n  chain_head:   {}\n  index:        {}\n  taint:        {:?}",
        hex::encode(envelope.receipt.public_key),
        hex::encode(envelope.chain_head),
        envelope.receipt.index,
        envelope.receipt.taint
    );
    Ok(())
}
