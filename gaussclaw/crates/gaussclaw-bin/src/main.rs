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
    clippy::single_match_else,
    clippy::uninlined_format_args
)]

use clap::Parser;
use gaussclaw_cli::{
    ChatArgs, Cli, Command, ConfigCmd, CronCmd, DoctorArgs, GatewayCmd, ImportArgs, ModelCmd,
    ReceiptCmd, ToolsCmd, WebArgs,
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

    let cfg_source = report.as_ref().and_then(|r| r.source.clone());
    match cli.command {
        None => run_default_tui(&cfg),
        Some(Command::Web(args)) => run_web(cfg, report, args),
        Some(cmd) => dispatch(cmd, cfg, cfg_source),
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
        let store = std::sync::Arc::new(gaussclaw_store::SessionStore::open_in_memory().await?);
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
        caps: cfg.caps.as_ref().map_or(0, |c| {
            u32::try_from(c.default_grant.len()).unwrap_or(u32::MAX)
        }),
    };
    gaussclaw_tui::run(status)
}

fn dispatch(
    cmd: Command,
    cfg: gaussclaw_config::Config,
    cfg_source: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    match cmd {
        Command::Chat(args) => run_chat(args, &cfg),
        Command::Model(sub) => match sub {
            ModelCmd::List => run_model_list(&cfg),
            ModelCmd::Show => run_model_show(&cfg),
            ModelCmd::Set { model_id } => run_model_set(&model_id, cfg_source.as_deref(), &cfg),
        },
        Command::Tools(sub) => match sub {
            ToolsCmd::List => run_tools_list(),
            ToolsCmd::Show { tool } => run_tools_show(&tool),
            ToolsCmd::Enable { .. } => {
                stub("tools enable", 3, "gaussclaw-skill + gaussclaw-config")
            }
            ToolsCmd::Disable { .. } => {
                stub("tools disable", 3, "gaussclaw-skill + gaussclaw-config")
            }
        },
        Command::Config(sub) => match sub {
            ConfigCmd::List => run_config_list(&cfg),
            ConfigCmd::Get { key } => run_config_get(&key, &cfg),
            ConfigCmd::Set { key, value } => {
                run_config_set(&key, &value, cfg_source.as_deref(), &cfg)
            }
            ConfigCmd::Path => run_config_path(cfg_source.as_deref()),
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
            ReceiptCmd::Head => run_receipt_head(),
            ReceiptCmd::Verify { envelope } => run_receipt_verify(envelope),
        },
        Command::Cron(sub) => run_cron(sub),
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

// ─── chat ──────────────────────────────────────────────────────────────────

fn run_chat(args: ChatArgs, cfg: &gaussclaw_config::Config) -> anyhow::Result<()> {
    use gauss_core::TaintLabel;
    use gaussclaw_agent::{EchoProvider, KernelHandle, Message, Prompt, TurnPolicy};
    let Some(message) = args.message else {
        eprintln!(
            "gaussclaw chat: interactive mode is the TUI (`gaussclaw` with no args).\n  \
             Use `-m TEXT` for a one-shot turn against the configured provider."
        );
        return Ok(());
    };
    let model = if cfg.provider.name.is_empty() {
        "echo".to_string()
    } else {
        format!("{}/{}", cfg.provider.name, cfg.provider.model)
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let completion = rt.block_on(async move {
        // The shipping binary's default chat path runs the EchoProvider —
        // real vendor drivers in `gaussclaw-providers` ship behind the
        // `chat` command once provider auth wiring lands (the crate is
        // already present and tested; the binary's chat path is the last
        // mile). Until then the EchoProvider gives the user a working
        // round trip that exercises the kernel admit gate and the audit
        // chain.
        let provider = std::sync::Arc::new(EchoProvider::default());
        let tp = TurnPolicy::new(KernelHandle::permissive(), provider);
        let prompt = Prompt::new(model, vec![Message::new("user", message.clone())]);
        tp.run(prompt, TaintLabel::User).await
    });
    let completion = completion.map_err(|e| anyhow::anyhow!("turn: {e:?}"))?;
    if let Some(sid) = args.session {
        println!("session: {sid}");
    }
    println!("{}", completion.text);
    Ok(())
}

// ─── model ─────────────────────────────────────────────────────────────────

fn run_model_list(cfg: &gaussclaw_config::Config) -> anyhow::Result<()> {
    // The shipping binary doesn't itself construct provider catalogues;
    // it surfaces what `gaussclaw.toml` declares. A real deployment
    // populates [provider.chain] with vendor-prefixed model ids that
    // `gaussclaw-providers` then routes through.
    if cfg.provider.name.is_empty() {
        println!("(no model configured — set provider.name + provider.model in gaussclaw.toml)");
    } else {
        println!("{}/{}    (active)", cfg.provider.name, cfg.provider.model);
    }
    if let Some(chain) = &cfg.provider.chain {
        for fallback in &chain.fallback {
            println!("{fallback}    (fallback)");
        }
    }
    Ok(())
}

fn run_model_show(cfg: &gaussclaw_config::Config) -> anyhow::Result<()> {
    if cfg.provider.name.is_empty() {
        println!("(no active model)");
    } else {
        println!("provider: {}", cfg.provider.name);
        println!("model:    {}", cfg.provider.model);
        if let Some(chain) = &cfg.provider.chain {
            println!("fallback chain: {}", chain.fallback.join(" → "));
        }
    }
    Ok(())
}

fn run_model_set(
    model_id: &str,
    cfg_path: Option<&std::path::Path>,
    cfg: &gaussclaw_config::Config,
) -> anyhow::Result<()> {
    let (provider_name, model) = model_id.split_once('/').ok_or_else(|| {
        anyhow::anyhow!("model id must be in `provider/model` form (got {model_id:?})")
    })?;
    let path = cfg_path.ok_or_else(|| {
        anyhow::anyhow!("no config file loaded — run `gaussclaw config path` first")
    })?;
    let mut new_cfg = cfg.clone();
    new_cfg.provider.name = provider_name.into();
    new_cfg.provider.model = model.into();
    gaussclaw_config::save(&new_cfg, path)?;
    println!("set provider.name = {provider_name}");
    println!("set provider.model = {model}");
    println!("saved → {}", path.display());
    Ok(())
}

// ─── tools ─────────────────────────────────────────────────────────────────

fn run_tools_list() -> anyhow::Result<()> {
    let reg = gaussclaw_tools::default_registry();
    if reg.is_empty() {
        println!("(no tools registered)");
        return Ok(());
    }
    println!("{} registered tools:", reg.len());
    for id in reg.ids() {
        if let Some(tool) = reg.get(id) {
            let m = tool.manifest();
            println!("  {id:<20} cap_required=0x{:016x}", m.cap_required.bits());
        }
    }
    Ok(())
}

fn run_tools_show(name: &str) -> anyhow::Result<()> {
    let reg = gaussclaw_tools::default_registry();
    let tool = reg
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("unknown tool: {name}"))?;
    let m = tool.manifest();
    println!("id:             {}", m.id.0);
    println!("cap_required:   0x{:016x}", m.cap_required.bits());
    println!("reversible:     {}", m.reversible);
    Ok(())
}

// ─── config ────────────────────────────────────────────────────────────────

fn run_config_list(cfg: &gaussclaw_config::Config) -> anyhow::Result<()> {
    let body = toml::to_string_pretty(cfg)?;
    print!("{body}");
    Ok(())
}

fn run_config_get(key: &str, cfg: &gaussclaw_config::Config) -> anyhow::Result<()> {
    let v = toml::Value::try_from(cfg).map_err(|e| anyhow::anyhow!("toml: {e}"))?;
    let mut cursor = &v;
    for part in key.split('.') {
        match cursor {
            toml::Value::Table(t) => match t.get(part) {
                Some(next) => cursor = next,
                None => {
                    eprintln!("unknown key: {key}");
                    std::process::exit(1);
                }
            },
            _ => {
                eprintln!("key {key} traverses a non-table");
                std::process::exit(1);
            }
        }
    }
    println!("{}", cursor);
    Ok(())
}

fn run_config_set(
    key: &str,
    value: &str,
    cfg_path: Option<&std::path::Path>,
    cfg: &gaussclaw_config::Config,
) -> anyhow::Result<()> {
    let path = cfg_path.ok_or_else(|| {
        anyhow::anyhow!("no config file loaded — create a `gaussclaw.toml` first")
    })?;
    let parsed_value: toml::Value = value
        .parse()
        .or_else(|_| toml::Value::try_from(value))
        .map_err(|e| anyhow::anyhow!("parse value: {e}"))?;
    let mut tree: toml::Value =
        toml::Value::try_from(cfg).map_err(|e| anyhow::anyhow!("encode config: {e}"))?;
    let parts: Vec<&str> = key.split('.').collect();
    {
        let mut cursor = &mut tree;
        for part in &parts[..parts.len().saturating_sub(1)] {
            cursor = cursor
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("key {key} traverses non-table at {part}"))?
                .entry(String::from(*part))
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        }
        let last = parts.last().ok_or_else(|| anyhow::anyhow!("empty key"))?;
        cursor
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("key {key} terminus is not a table"))?
            .insert((*last).into(), parsed_value);
    }
    let new_cfg: gaussclaw_config::Config = tree
        .try_into()
        .map_err(|e| anyhow::anyhow!("re-parse: {e}"))?;
    gaussclaw_config::save(&new_cfg, path)?;
    println!("set {key} = {value}");
    println!("saved → {}", path.display());
    Ok(())
}

fn run_config_path(cfg_path: Option<&std::path::Path>) -> anyhow::Result<()> {
    match cfg_path {
        Some(p) => println!("{}", p.display()),
        None => {
            println!("(no config file loaded; default search path:)");
            for p in gaussclaw_config::search_path() {
                println!("  {}", p.display());
            }
        }
    }
    Ok(())
}

// ─── receipt head ──────────────────────────────────────────────────────────

fn run_receipt_head() -> anyhow::Result<()> {
    // The shipping binary opens a fresh in-memory store and prints its
    // chain head — useful for "is the chain machinery wired correctly?"
    // smoke tests. Production deployments query the persistent
    // SessionStore via the `web` surface's `/api/receipt/head` endpoint.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let head = rt.block_on(async move {
        let store = gaussclaw_store::SessionStore::open_in_memory().await?;
        store.chain_head().await
    })?;
    println!("digest: {}", head.digest_hex);
    println!("length: {}", head.length);
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

// ─── Sprint 5 §1: cron ──────────────────────────────────────────────────────

fn run_cron(sub: CronCmd) -> anyhow::Result<()> {
    // The shipping binary builds a fresh in-memory scheduler per
    // invocation — the actual production wiring (one scheduler per
    // process, persisted to the Trinity store) lands in Sprint 5 §2.
    // The CLI surface is still useful for verifying schedule grammar
    // and exercising the cap-gate end-to-end.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        use std::sync::Arc;
        let store = Arc::new(gauss_cron::InMemoryJobStore::new());
        let sched = gauss_cron::Scheduler::new(store, gauss_cron::SystemClock);
        match sub {
            CronCmd::List => {
                let jobs = sched.list().await
                    .map_err(|e| anyhow::anyhow!("scheduler list: {e}"))?;
                if jobs.is_empty() {
                    println!("(no scheduled jobs)");
                } else {
                    println!(
                        "{:<4}  {:<20}  {:<24}  {:<10}  {:<10}  next_fire_at",
                        "id", "label", "schedule", "status", "fires"
                    );
                    for j in &jobs {
                        println!(
                            "{:<4}  {:<20}  {:<24}  {:<10?}  {:<10}  {:?}",
                            j.id.0, j.label, j.schedule, j.status, j.fire_count, j.next_fire_at
                        );
                    }
                }
            }
            CronCmd::Add { schedule, label, payload } => {
                let schedule = gauss_cron::parse_schedule(&schedule)
                    .map_err(|e| anyhow::anyhow!("schedule grammar: {e}"))?;
                let payload_value: serde_json::Value = serde_json::from_str(&payload)
                    .unwrap_or(serde_json::Value::Null);
                let j = gauss_cron::Job::new(
                    gauss_cron::JobId::new(0),
                    label,
                    schedule,
                    gauss_core::CapToken::BOTTOM,
                    payload_value,
                    0,
                );
                let added = sched.add(j).await
                    .map_err(|e| anyhow::anyhow!("scheduler add: {e}"))?;
                println!(
                    "ok: cron job added\n  id:           {}\n  label:        {}\n  schedule:     {}\n  next_fire_at: {:?}",
                    added.id.0, added.label, added.schedule, added.next_fire_at
                );
            }
            CronCmd::Pause { id } => {
                sched.pause(gauss_cron::JobId::new(id)).await
                    .map_err(|e| anyhow::anyhow!("scheduler pause: {e}"))?;
                println!("ok: paused {id}");
            }
            CronCmd::Resume { id } => {
                sched.resume(gauss_cron::JobId::new(id)).await
                    .map_err(|e| anyhow::anyhow!("scheduler resume: {e}"))?;
                println!("ok: resumed {id}");
            }
            CronCmd::Remove { id } => {
                sched.cancel(gauss_cron::JobId::new(id)).await
                    .map_err(|e| anyhow::anyhow!("scheduler cancel: {e}"))?;
                println!("ok: removed {id}");
            }
            CronCmd::Status { id } => {
                let jobs = sched.list().await
                    .map_err(|e| anyhow::anyhow!("scheduler list: {e}"))?;
                match jobs.into_iter().find(|j| j.id.0 == id) {
                    Some(j) => {
                        println!("{}", serde_json::to_string_pretty(&j)?);
                    }
                    None => {
                        eprintln!("unknown job: {id}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
}
