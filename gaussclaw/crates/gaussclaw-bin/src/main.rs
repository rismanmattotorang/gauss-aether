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
    PluginsCmd, ProxyArgs, ReceiptCmd, SkillCmd, SnapshotCmd, ToolsCmd, WebArgs,
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
        // Sprint 5 §3: one persistent in-memory cron scheduler per
        // server lifecycle. Trinity-backed persistence wires in the
        // Sprint 5 §3 follow-on.
        let cron_store: std::sync::Arc<dyn gauss_cron::JobStore> =
            std::sync::Arc::new(gauss_cron::InMemoryJobStore::new());
        let cron = std::sync::Arc::new(gauss_cron::Scheduler::new(
            cron_store,
            gauss_cron::SystemClock,
        ));
        // Build the AgentLoop that drives /api/chat/ws (Sprint 11).
        // The vendor codec is selected from `cfg.provider.name` and
        // wired to a real `reqwest`-backed `HttpBackend` so vendor
        // calls hit the wire. When the configured vendor is unknown or
        // empty, `pick_provider` falls back to the EchoProvider — the
        // backend is harmless there because it's never invoked.
        let kernel = gaussclaw_agent::KernelHandle::permissive();
        // API key (env-sourced) + live reqwest transport, shared with
        // the one-shot `chat` path via `build_provider_choice`.
        let (_model, choice) = build_provider_choice(&cfg);
        let (provider, picked) = gaussclaw_providers::pick_provider(&choice);
        tracing::info!(
            target: "gaussclaw_bin::serve",
            "vendor codec selected: {} (live HTTP transport)",
            picked.as_str()
        );
        let audit = gaussclaw_agent::AuditTrace::new();
        let policy =
            gaussclaw_agent::TurnPolicy::new(kernel.clone(), provider).with_audit(audit.clone());
        let compactor: std::sync::Arc<dyn gaussclaw_agent::Compactor> =
            std::sync::Arc::new(gaussclaw_agent::WindowedCompactor::defaults());
        let agent = std::sync::Arc::new(
            gaussclaw_agent::AgentLoop::new(policy)
                .with_compactor(compactor)
                .with_audit(audit.clone()),
        );

        let state = gaussclaw_web::ServerState::new(cfg, source)
            .with_store(store)
            .with_cron(cron)
            .with_plugin_roots(gaussclaw_plugins::default_discovery_roots())
            .with_agent(agent);
        gaussclaw_web::serve(addr, state).await
    })
}

/// Drives TUI user turns through a [`TurnPolicy`] on a background
/// thread, keeping a running transcript so the terminal holds a real
/// multi-turn conversation. The TUI render loop stays responsive and
/// cancellable while a turn is in flight.
struct AgentTurnDispatcher {
    kernel: gaussclaw_agent::KernelHandle,
    provider: std::sync::Arc<dyn gaussclaw_agent::ProviderHandle>,
    model: String,
    /// Conversation so far, replayed into every prompt for context.
    transcript: std::sync::Arc<std::sync::Mutex<Vec<gaussclaw_agent::Message>>>,
}

impl gaussclaw_tui::TurnDispatcher for AgentTurnDispatcher {
    fn dispatch(&self, prompt: String, tx: std::sync::mpsc::Sender<gaussclaw_tui::TurnOutcome>) {
        use gaussclaw_tui::TurnOutcome;
        let kernel = self.kernel.clone();
        let provider = self.provider.clone();
        let model = self.model.clone();
        let transcript = self.transcript.clone();
        // Run off the render thread on a single-thread runtime so the UI
        // never blocks. The outcome goes back over `tx` exactly once.
        std::thread::spawn(move || {
            let messages = {
                let mut hist = transcript
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                hist.push(gaussclaw_agent::Message::new("user", prompt));
                hist.clone()
            };
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(TurnOutcome::Error(format!("runtime build: {e}")));
                    return;
                }
            };
            let policy = gaussclaw_agent::TurnPolicy::new(kernel, provider);
            let prompt = gaussclaw_agent::Prompt::new(model, messages);
            match rt.block_on(policy.run(prompt, gauss_core::TaintLabel::User)) {
                Ok(completion) => {
                    transcript
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(gaussclaw_agent::Message::new(
                            "assistant",
                            completion.text.clone(),
                        ));
                    let _ = tx.send(TurnOutcome::Reply(completion.text));
                }
                Err(e) => {
                    let _ = tx.send(TurnOutcome::Error(format!("{e:?}")));
                }
            }
        });
    }
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
    // Select the vendor codec + live transport (same path as `chat` /
    // `serve`) and drive the terminal through it. With no vendor
    // configured this resolves to the EchoProvider, so the TUI still
    // gives a working round-trip rather than a dead stub.
    let (model, choice) = build_provider_choice(cfg);
    let (provider, _picked) = gaussclaw_providers::pick_provider(&choice);
    let dispatcher: std::sync::Arc<dyn gaussclaw_tui::TurnDispatcher> =
        std::sync::Arc::new(AgentTurnDispatcher {
            kernel: gaussclaw_agent::KernelHandle::permissive(),
            provider,
            model,
            transcript: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        });
    gaussclaw_tui::run_with_dispatcher(status, Some(dispatcher))
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
        Command::Snapshot(sub) => run_snapshot(sub),
        Command::Plugins(sub) => run_plugins(sub),
        Command::Skill(sub) => run_skill(sub),
        Command::Proxy(args) => run_proxy(args),
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

// ─── provider wiring ─────────────────────────────────────────────────────────

/// Build a [`gaussclaw_providers::ProviderChoice`] from config: the
/// configured vendor name, an env-sourced API key, and the live
/// reqwest HTTP transport. Returns the bare model id the codec should
/// send upstream alongside the choice.
///
/// Shared by the one-shot `chat` command and the `serve` agent-loop
/// wiring so both reach the wire identically. An unset API-key env var
/// passes an empty key through to the codec (which surfaces as a 401
/// from the upstream). If the reqwest client can't be built, the
/// backend is left unset and `pick_provider` falls back to the
/// fail-closed `UnconfiguredBackend`.
fn build_provider_choice(
    cfg: &gaussclaw_config::Config,
) -> (String, gaussclaw_providers::ProviderChoice) {
    let model = if cfg.provider.model.is_empty() {
        "echo".to_string()
    } else {
        cfg.provider.model.clone()
    };
    let env_key = match cfg.provider.name.to_ascii_lowercase().as_str() {
        "anthropic" => std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        "openai" => std::env::var("OPENAI_API_KEY").unwrap_or_default(),
        _ => String::new(),
    };
    let mut choice = gaussclaw_providers::ProviderChoice::new(cfg.provider.name.clone())
        .with_api_key(env_key);
    match gaussclaw_http::ReqwestProviderBackend::new() {
        Ok(backend) => choice = choice.with_backend(std::sync::Arc::new(backend)),
        Err(e) => tracing::error!(
            target: "gaussclaw_bin",
            "failed to build HTTP backend, vendor calls will fail closed: {e}"
        ),
    }
    (model, choice)
}

// ─── chat ──────────────────────────────────────────────────────────────────

fn run_chat(args: ChatArgs, cfg: &gaussclaw_config::Config) -> anyhow::Result<()> {
    use gauss_core::TaintLabel;
    use gaussclaw_agent::{KernelHandle, Message, Prompt, TurnPolicy};
    let Some(message) = args.message else {
        eprintln!(
            "gaussclaw chat: interactive mode is the TUI (`gaussclaw` with no args).\n  \
             Use `-m TEXT` for a one-shot turn against the configured provider."
        );
        return Ok(());
    };
    // Select the vendor codec from config and attach the live HTTP
    // transport. With no vendor configured, `pick_provider` falls back
    // to the EchoProvider; the model id we send the codec is the bare
    // `provider.model` (the vendor doesn't understand the `name/model`
    // display form).
    let (model, choice) = build_provider_choice(cfg);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let completion = rt.block_on(async move {
        let (provider, _picked) = gaussclaw_providers::pick_provider(&choice);
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

#[allow(clippy::too_many_lines)]
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
            CronCmd::Edit { id, label, schedule } => {
                if label.is_none() && schedule.is_none() {
                    eprintln!("error: pass --label and/or --schedule");
                    std::process::exit(1);
                }
                let parsed_schedule = match schedule {
                    Some(s) => Some(
                        gauss_cron::parse_schedule(&s)
                            .map_err(|e| anyhow::anyhow!("schedule grammar: {e}"))?,
                    ),
                    None => None,
                };
                // The standalone CLI's in-memory store is fresh per
                // invocation, so `edit` only succeeds against a job
                // added in the same invocation. The web dashboard's
                // `/api/cron/{id}` endpoint is the persistent edit
                // path until the Trinity-backed store wires in
                // (Sprint 5 §3 follow-on).
                let edited = sched
                    .edit(gauss_cron::JobId::new(id), label, parsed_schedule)
                    .await
                    .map_err(|e| anyhow::anyhow!("scheduler edit: {e}"))?;
                println!(
                    "ok: cron job edited\n  id:           {}\n  label:        {}\n  schedule:     {}\n  next_fire_at: {:?}",
                    edited.id.0, edited.label, edited.schedule, edited.next_fire_at
                );
            }
            CronCmd::Run { id } => {
                let outcome = sched
                    .run_now(gauss_cron::JobId::new(id), gauss_core::CapToken::TOP, |_j| None)
                    .await
                    .map_err(|e| anyhow::anyhow!("scheduler run_now: {e}"))?;
                match outcome {
                    gauss_cron::FireOutcome::Fired { id, receipt_id } => {
                        println!("ok: fired job {} (receipt {:?})", id.0, receipt_id);
                    }
                    gauss_cron::FireOutcome::Refused { id, reason } => {
                        eprintln!("refused: job {} ({reason})", id.0);
                        std::process::exit(1);
                    }
                    _ => {
                        eprintln!("error: unknown fire outcome");
                        std::process::exit(1);
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })
}

// ─── Sprint 5 §8: snapshot / rollback ──────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn run_snapshot(sub: SnapshotCmd) -> anyhow::Result<()> {
    // Each CLI invocation builds a fresh in-memory backend, like cron.
    // The shipping binary's persistent snapshot store lands once
    // `gaussclaw-store` grows a `checkpoints` table (Sprint 5 §8.2).
    // The CLI is still useful for verifying snapshot/rollback grammar
    // and exercising the cap-gate end-to-end against fresh state.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        use std::path::PathBuf;
        use std::sync::Arc;
        let backend: Box<dyn gauss_checkpoint::CheckpointBackend> =
            Box::new(gauss_checkpoint::MemoryBackend::new());
        let mgr = Arc::new(gauss_checkpoint::CheckpointManager::new(backend));
        let grant = gauss_core::CapToken::CHECKPOINT_WRITE
            | gauss_core::CapToken::CHECKPOINT_ROLLBACK;
        match sub {
            SnapshotCmd::Save { label, paths, root } => {
                let root_path = root
                    .map_or_else(|| std::env::current_dir().unwrap_or_default(), PathBuf::from);
                let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
                if path_bufs.is_empty() {
                    eprintln!("error: pass --path PATH (repeatable) listing files to capture");
                    std::process::exit(1);
                }
                let (snap, receipt) = mgr
                    .snapshot(grant, &root_path, &label, &path_bufs)
                    .await
                    .map_err(|e| anyhow::anyhow!("snapshot: {e}"))?;
                println!(
                    "ok: snapshot saved\n  id:         {}\n  label:      {}\n  files:      {}\n  bytes:      {}\n  timestamp:  {}",
                    snap.id, snap.label, receipt.file_count, receipt.size_bytes, receipt.timestamp
                );
            }
            SnapshotCmd::List => {
                let all = mgr.list().await.map_err(|e| anyhow::anyhow!("list: {e}"))?;
                if all.is_empty() {
                    println!("(no snapshots in this process)");
                } else {
                    println!(
                        "{:<64}  {:<32}  {:<6}  {:<10}",
                        "id", "label", "files", "bytes"
                    );
                    for s in &all {
                        println!(
                            "{:<64}  {:<32}  {:<6}  {:<10}",
                            s.id, s.label, s.file_count(), s.size_bytes()
                        );
                    }
                }
            }
            SnapshotCmd::Status { id } => {
                let all = mgr.list().await.map_err(|e| anyhow::anyhow!("list: {e}"))?;
                match all.into_iter().find(|s| s.id.0 == id) {
                    Some(s) => println!("{}", serde_json::to_string_pretty(&s)?),
                    None => {
                        eprintln!("unknown snapshot: {id}");
                        std::process::exit(1);
                    }
                }
            }
            SnapshotCmd::Restore { id, root } => {
                let root_path = root
                    .map_or_else(|| std::env::current_dir().unwrap_or_default(), PathBuf::from);
                let receipt = mgr
                    .rollback(grant, &gauss_checkpoint::CheckpointId::new(id), &root_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("rollback: {e}"))?;
                println!(
                    "ok: rolled back\n  id:    {}\n  files: {}",
                    receipt.id, receipt.file_count
                );
            }
            SnapshotCmd::Remove { id } => {
                mgr.remove(&gauss_checkpoint::CheckpointId::new(id.clone()))
                    .await
                    .map_err(|e| anyhow::anyhow!("remove: {e}"))?;
                println!("ok: removed {id}");
            }
        }
        Ok::<(), anyhow::Error>(())
    })
}

// ─── Sprint 7 §2: plugins ──────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn run_plugins(sub: PluginsCmd) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        use std::path::PathBuf;
        fn parse_kind(s: &str) -> anyhow::Result<gaussclaw_plugins::PluginKind> {
            match s {
                "standalone" => Ok(gaussclaw_plugins::PluginKind::Standalone),
                "backend" => Ok(gaussclaw_plugins::PluginKind::Backend),
                "exclusive" => Ok(gaussclaw_plugins::PluginKind::Exclusive),
                "platform" => Ok(gaussclaw_plugins::PluginKind::Platform),
                "model_provider" | "model-provider" => {
                    Ok(gaussclaw_plugins::PluginKind::ModelProvider)
                }
                other => Err(anyhow::anyhow!(
                    "unknown plugin kind {other:?} (try standalone/backend/exclusive/platform/model_provider)"
                )),
            }
        }
        async fn discover_all(roots: &[String]) -> anyhow::Result<Vec<gaussclaw_plugins::LoadedPlugin>> {
            let mut all: Vec<gaussclaw_plugins::LoadedPlugin> = Vec::new();
            let mut failures: Vec<(PathBuf, String)> = Vec::new();
            let roots_iter: Vec<PathBuf> = if roots.is_empty() {
                gaussclaw_plugins::default_discovery_roots()
            } else {
                roots.iter().map(PathBuf::from).collect()
            };
            for r in &roots_iter {
                let report = gaussclaw_plugins::PluginLoader::discover_in(r)
                    .await
                    .map_err(|e| anyhow::anyhow!("discover {}: {e}", r.display()))?;
                all.extend(report.found);
                failures.extend(report.failures);
            }
            for (path, reason) in &failures {
                eprintln!("warn: skipped {} ({reason})", path.display());
            }
            Ok(all)
        }
        match sub {
            PluginsCmd::List { root } => {
                let all = discover_all(&root).await?;
                if all.is_empty() {
                    println!("(no plugins discovered)");
                } else {
                    println!(
                        "{:<14}  {:<24}  {:<10}  {:<7}",
                        "kind", "name", "version", "enabled"
                    );
                    for p in &all {
                        println!(
                            "{:<14}  {:<24}  {:<10}  {:<7}",
                            p.manifest.kind.as_str(),
                            p.manifest.name,
                            p.manifest.version,
                            p.enabled
                        );
                    }
                }
            }
            PluginsCmd::Inspect { kind, name, root } => {
                let want = parse_kind(&kind)?;
                let all = discover_all(&root).await?;
                match all
                    .into_iter()
                    .find(|p| p.manifest.kind == want && p.manifest.name == name)
                {
                    Some(p) => {
                        println!(
                            "name:        {}\nkind:        {}\nversion:     {}\ndescription: {}\nentry:       {}\ncaps:        {:?}\ntags:        {:?}\nprovenance:  {}\nmanifest:    {}",
                            p.manifest.name,
                            p.manifest.kind.as_str(),
                            p.manifest.version,
                            p.manifest.description,
                            p.manifest.entry,
                            p.manifest.caps,
                            p.manifest.tags,
                            p.provenance,
                            p.manifest_path
                                .as_ref()
                                .map_or_else(String::new, |x| x.display().to_string()),
                        );
                    }
                    None => {
                        eprintln!("unknown plugin: {kind}/{name}");
                        std::process::exit(1);
                    }
                }
            }
            PluginsCmd::Enable { kind, name } => {
                let _ = parse_kind(&kind)?;
                eprintln!(
                    "note: plugin enable/disable persistence lands in Sprint 7 §3 (the dashboard PluginsPage). For now the CLI just acknowledges the intent ({kind}/{name})."
                );
            }
            PluginsCmd::Disable { kind, name } => {
                let _ = parse_kind(&kind)?;
                eprintln!(
                    "note: plugin enable/disable persistence lands in Sprint 7 §3 (the dashboard PluginsPage). For now the CLI just acknowledges the intent ({kind}/{name})."
                );
            }
            PluginsCmd::Install { path } => {
                let path = PathBuf::from(path);
                let manifest_path = if path.is_dir() {
                    path.join("plugin.toml")
                } else {
                    path
                };
                let toml_src = tokio::fs::read_to_string(&manifest_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("read {}: {e}", manifest_path.display()))?;
                let manifest = gaussclaw_plugins::PluginManifest::from_toml(&toml_src)
                    .map_err(|e| anyhow::anyhow!("parse manifest: {e}"))?;
                let provenance = manifest
                    .provenance_digest()
                    .unwrap_or_default();
                println!(
                    "ok: plugin manifest validated\n  name:       {}\n  kind:       {}\n  version:    {}\n  caps:       {:?}\n  provenance: {}",
                    manifest.name,
                    manifest.kind.as_str(),
                    manifest.version,
                    manifest.caps,
                    provenance,
                );
                eprintln!(
                    "note: install-to-user-root persistence lands in Sprint 7 §7 (skill installer). For now `install` only validates the manifest."
                );
            }
        }
        Ok::<(), anyhow::Error>(())
    })
}

// ─── Sprint 7 §7: skill installer ──────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn run_skill(sub: SkillCmd) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        use std::path::PathBuf;

        fn default_root() -> anyhow::Result<PathBuf> {
            directories::ProjectDirs::from("io", "gauss-aether", "gaussclaw")
                .map(|p| p.data_dir().join("skills"))
                .ok_or_else(|| anyhow::anyhow!("no XDG data dir available; pass --root PATH"))
        }

        async fn read_manifest(path: &str) -> anyhow::Result<(PathBuf, String, gaussclaw_skill::SkillManifest, String)> {
            let p = PathBuf::from(path);
            let manifest_path = if p.is_dir() { p.join("skill.toml") } else { p };
            let toml_src = tokio::fs::read_to_string(&manifest_path)
                .await
                .map_err(|e| anyhow::anyhow!("read {}: {e}", manifest_path.display()))?;
            let manifest = gaussclaw_skill::SkillManifest::from_toml(&toml_src)
                .map_err(|e| anyhow::anyhow!("parse: {e}"))?;
            let provenance = blake3::hash(toml_src.as_bytes()).to_hex().to_string();
            Ok((manifest_path, toml_src, manifest, provenance))
        }

        match sub {
            SkillCmd::Preview { path } => {
                let (_, _, manifest, provenance) = read_manifest(&path).await?;
                let cap_token = manifest
                    .cap_required()
                    .map_err(|e| anyhow::anyhow!("resolve caps: {e}"))?;
                println!(
                    "ok: skill manifest validated\n  name:        {}\n  caps:        {:?}\n  cap_bits:    0x{:016x}\n  taint:       {}\n  reversible:  {}\n  persistent:  {}\n  provenance:  {}",
                    manifest.name,
                    manifest.caps,
                    cap_token.bits(),
                    manifest.taint,
                    manifest.reversible,
                    manifest.persistent,
                    provenance,
                );
            }
            SkillCmd::Install { path, root, force } => {
                let (_, toml_src, manifest, provenance) = read_manifest(&path).await?;
                let root = root
                    .map(PathBuf::from)
                    .map_or_else(default_root, Ok)?;
                let dir = root.join(&manifest.name);
                if tokio::fs::try_exists(&dir).await? {
                    if !force {
                        return Err(anyhow::anyhow!(
                            "skill {} already installed at {}; pass --force to overwrite",
                            manifest.name,
                            dir.display()
                        ));
                    }
                    tokio::fs::remove_dir_all(&dir).await?;
                }
                tokio::fs::create_dir_all(&dir).await?;
                tokio::fs::write(dir.join("skill.toml"), toml_src.as_bytes()).await?;
                let receipt = serde_json::json!({
                    "kind":        "skill_install_receipt",
                    "name":        manifest.name,
                    "caps":        manifest.caps,
                    "taint":       manifest.taint,
                    "reversible":  manifest.reversible,
                    "provenance":  provenance,
                    "installed_at": now_unix(),
                });
                let receipt_bytes = serde_json::to_vec_pretty(&receipt)?;
                tokio::fs::write(dir.join("receipt.json"), &receipt_bytes).await?;
                let receipt_digest = blake3::hash(&receipt_bytes).to_hex().to_string();
                println!(
                    "ok: skill installed\n  name:        {}\n  dir:         {}\n  provenance:  {}\n  receipt:     {}",
                    manifest.name,
                    dir.display(),
                    provenance,
                    receipt_digest,
                );
            }
            SkillCmd::List { root } => {
                let root = root
                    .map(PathBuf::from)
                    .map_or_else(default_root, Ok)?;
                if !tokio::fs::try_exists(&root).await? {
                    println!("(no skills installed at {})", root.display());
                    return Ok::<(), anyhow::Error>(());
                }
                let mut rd = tokio::fs::read_dir(&root).await?;
                let mut count = 0u64;
                println!("{:<28}  {:<10}  {:<10}  {:<10}", "name", "taint", "rev", "persist");
                while let Some(entry) = rd.next_entry().await? {
                    let p = entry.path();
                    let skill = p.join("skill.toml");
                    if !tokio::fs::try_exists(&skill).await? {
                        continue;
                    }
                    let src = tokio::fs::read_to_string(&skill).await?;
                    if let Ok(m) = gaussclaw_skill::SkillManifest::from_toml(&src) {
                        count = count.saturating_add(1);
                        println!(
                            "{:<28}  {:<10}  {:<10}  {:<10}",
                            m.name, m.taint, m.reversible, m.persistent
                        );
                    }
                }
                if count == 0 {
                    println!("(no skills installed at {})", root.display());
                }
            }
            SkillCmd::Remove { name, root } => {
                let root = root
                    .map(PathBuf::from)
                    .map_or_else(default_root, Ok)?;
                let dir = root.join(&name);
                if !tokio::fs::try_exists(&dir).await? {
                    return Err(anyhow::anyhow!(
                        "no skill {name} installed under {}",
                        root.display()
                    ));
                }
                tokio::fs::remove_dir_all(&dir).await?;
                println!("ok: removed {}", dir.display());
            }
        }
        Ok::<(), anyhow::Error>(())
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

// ─── Sprint 7 §6: proxy ────────────────────────────────────────────────────

fn run_proxy(args: ProxyArgs) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let url = format!("http://{addr}/");
    eprintln!("gaussclaw proxy: serving on {url} (OpenAI-compat /v1/chat/completions)");
    if args.mock {
        eprintln!("note: using MockUpstream — completions are deterministic echoes.");
    } else {
        eprintln!(
            "note: real upstream wiring (gaussclaw-providers) lands in Sprint 8; for now \
             every proxy run is mock-backed."
        );
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let upstream: std::sync::Arc<dyn gaussclaw_proxy::UpstreamCaller> =
            std::sync::Arc::new(gaussclaw_proxy::MockUpstream::new());
        let state =
            gaussclaw_proxy::ProxyState::new(gaussclaw_proxy::ProxyConfig::default(), upstream)
                .map_err(|e| anyhow::anyhow!("proxy init: {e}"))?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        tracing::info!(%addr, "gaussclaw-proxy listening");
        axum::serve(listener, gaussclaw_proxy::router(state)).await?;
        Ok::<(), anyhow::Error>(())
    })
}
