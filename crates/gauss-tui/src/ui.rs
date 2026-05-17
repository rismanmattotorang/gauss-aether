//! Rendering layer — one function per tab plus a help overlay.

use gauss_bench::AxisVerdict;
use gauss_health::Verdict;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Padding, Paragraph, Row, Table,
    Tabs, Wrap,
};
use ratatui::Frame;

use crate::app::{App, Tab};

const ACCENT: Color = Color::Cyan;
const ACCENT_DIM: Color = Color::DarkGray;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const ERR: Color = Color::Red;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header / tabs
            Constraint::Min(0),    // body
            Constraint::Length(2), // status / flash
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    match app.tab {
        Tab::Dashboard => draw_dashboard(frame, chunks[1], app),
        Tab::Turns => draw_turns(frame, chunks[1], app),
        Tab::Memory => draw_memory(frame, chunks[1], app),
        Tab::Sandbox => draw_sandbox(frame, chunks[1], app),
        Tab::Sag => draw_sag(frame, chunks[1], app),
        Tab::Health => draw_health(frame, chunks[1], app),
        Tab::Cluster => draw_cluster(frame, chunks[1], app),
        Tab::Audit => draw_audit(frame, chunks[1], app),
        Tab::Scorecard => draw_scorecard(frame, chunks[1], app),
        Tab::Logs => draw_logs(frame, chunks[1], app),
    }
    draw_status(frame, chunks[2], app);

    if app.show_help {
        draw_help_overlay(frame, area);
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let titles: Vec<Line<'static>> = Tab::all()
        .iter()
        .map(|t| {
            Line::from(vec![
                Span::styled(
                    format!(" {}", t.shortcut()),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::raw(t.title()),
                Span::raw(" "),
            ])
        })
        .collect();
    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .title(Line::from(vec![
                    Span::raw("─ "),
                    Span::styled(
                        "Gauss-Aether",
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" · "),
                    Span::styled("1.0", Style::default().fg(Color::Magenta)),
                    Span::raw(" "),
                ])),
        )
        .select(app.tab.index())
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(40)])
        .split(area);

    let bindings = match app.tab {
        Tab::Dashboard => "[t] run turn  [r] refresh  [a] seed approval  [/] taint",
        Tab::Turns => "[r] refresh  [↑↓ j/k] navigate  [Enter] details",
        Tab::Memory => "[r] refresh",
        Tab::Sandbox => "[r] refresh",
        Tab::Sag => "[a] approve  [d] deny  [s] seed demo  [↑↓ j/k] navigate",
        Tab::Health => "[r] re-evaluate  [g] grant 0 (demo fail)",
        Tab::Cluster => "[n] add node  [x] remove highlighted  [r] route",
        Tab::Audit => "[r] refresh",
        Tab::Scorecard => "[← →] cycle predecessor  [r] refresh",
        Tab::Logs => "[c] clear  [↑↓] scroll",
    };
    let general = " · [Tab] next · [Shift+Tab] prev · [?] help · [q] quit";
    let line = Line::from(vec![
        Span::styled(bindings, Style::default().fg(Color::Gray)),
        Span::styled(general, Style::default().fg(ACCENT_DIM)),
    ]);
    let p = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(ACCENT_DIM)),
    );
    frame.render_widget(p, chunks[0]);

    let flash = match &app.flash {
        Some((msg, "ok")) => Line::from(Span::styled(
            format!(" ✓ {msg} "),
            Style::default()
                .fg(Color::Black)
                .bg(OK)
                .add_modifier(Modifier::BOLD),
        )),
        Some((msg, "warn")) => Line::from(Span::styled(
            format!(" ! {msg} "),
            Style::default()
                .fg(Color::Black)
                .bg(WARN)
                .add_modifier(Modifier::BOLD),
        )),
        Some((msg, "err")) => Line::from(Span::styled(
            format!(" ✗ {msg} "),
            Style::default()
                .fg(Color::White)
                .bg(ERR)
                .add_modifier(Modifier::BOLD),
        )),
        _ => Line::from(Span::raw("")),
    };
    let fp = Paragraph::new(flash).alignment(Alignment::Right);
    frame.render_widget(fp, chunks[1]);
}

fn draw_dashboard(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(area);

    // KPI cards.
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(chunks[0]);

    let chain_len = app.health_subject.chain.map(|(l, _)| l).unwrap_or_default();
    let pending_count = app.pending.try_lock().map(|q| q.len()).unwrap_or(0);
    let health_verdict = if app.last_health.has_failure() {
        "FAIL"
    } else if app
        .last_health
        .invariants
        .iter()
        .any(|i| matches!(i.verdict, Verdict::Warning))
    {
        "WARN"
    } else {
        "OK"
    };
    let health_color = match health_verdict {
        "OK" => OK,
        "WARN" => WARN,
        _ => ERR,
    };

    frame.render_widget(
        kpi("chain length", &chain_len.to_string(), ACCENT),
        cards[0],
    );
    frame.render_widget(
        kpi(
            "pending approvals",
            &pending_count.to_string(),
            if pending_count > 0 { WARN } else { ACCENT },
        ),
        cards[1],
    );
    frame.render_widget(kpi("health", health_verdict, health_color), cards[2]);
    frame.render_widget(
        kpi("taint", taint_label(app.default_taint), ACCENT),
        cards[3],
    );

    // Bottom row: recent turns + system info.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[1]);

    let items: Vec<ListItem<'_>> = app
        .turn_history
        .iter()
        .take(20)
        .map(|t| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("#{:<5}", t.id.as_u128()),
                    Style::default().fg(ACCENT),
                ),
                Span::raw(format!(" {} actions ", t.action_count)),
                Span::styled(
                    taint_label(t.taint),
                    Style::default().fg(taint_color(t.taint)),
                ),
                Span::raw(" "),
                Span::styled(
                    short_hex(&t.chain_head_hex),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(section_block("Recent turns").border_style(Style::default().fg(ACCENT_DIM)));
    frame.render_widget(list, cols[0]);

    // System info table.
    let kernel_grant = format!("0x{:016X}", app.health_subject.grant);
    let chain_head_hex = app
        .health_subject
        .chain
        .map(|(_, d)| hex::encode(d))
        .unwrap_or_else(|| "—".into());
    let rows = vec![
        row2("workspace", "22 crates"),
        row2("tests", "299 passing"),
        row2("kernel grant", &kernel_grant),
        row2("chain head", &short_hex(&chain_head_hex)),
        row2("license", "MIT (ADR-0017)"),
        row2("cluster", &format!("{} nodes", app.ring.node_count())),
        row2("attestor", "SoftwareSim (Phase 10)"),
    ];
    let info = Table::new(rows, [Constraint::Length(16), Constraint::Min(0)])
        .block(section_block("System"));
    frame.render_widget(info, cols[1]);
}

fn draw_turns(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let items: Vec<ListItem<'_>> = app
        .turn_history
        .iter()
        .map(|t| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("#{:>5}", t.id.as_u128()),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  L{:<4}", t.chain_length)),
                Span::raw(format!("  {} act  ", t.action_count)),
                Span::styled(
                    taint_label(t.taint),
                    Style::default().fg(taint_color(t.taint)),
                ),
                Span::raw("  "),
                Span::styled(
                    short_hex(&t.chain_head_hex),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let mut state = ListState::default();
    if !app.turn_history.is_empty() {
        state.select(Some(app.turn_cursor.min(app.turn_history.len() - 1)));
    }
    let list = List::new(items)
        .block(section_block(&format!(
            "Turns ({} total)",
            app.turn_history.len()
        )))
        .highlight_style(Style::default().bg(ACCENT).fg(Color::Black))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, cols[0], &mut state);

    // Detail panel.
    let detail = app
        .turn_history
        .get(
            app.turn_cursor
                .min(app.turn_history.len().saturating_sub(1)),
        )
        .map(|t| {
            let mut rows = vec![
                row2("turn id", &t.id.as_u128().to_string()),
                row2("chain length", &t.chain_length.to_string()),
                row2("chain head", &t.chain_head_hex),
                row2("taint", taint_label(t.taint)),
                row2("actions", &t.action_count.to_string()),
                row2("committed at (ms)", &t.committed_at_ms.to_string()),
            ];
            if !t.sag_decisions.is_empty() {
                rows.push(row2("SAG", "—"));
                for d in &t.sag_decisions {
                    rows.push(row2(
                        "  decision",
                        &format!(
                            "{:?} / {} / {}",
                            d.risk,
                            if d.proceeded { "✓" } else { "✗" },
                            d.tool
                        ),
                    ));
                }
            }
            Table::new(rows, [Constraint::Length(20), Constraint::Min(0)])
                .block(section_block("Detail"))
        });

    if let Some(t) = detail {
        frame.render_widget(t, cols[1]);
    } else {
        let p = Paragraph::new("No turns yet — press [t] on the Dashboard to run one.")
            .block(section_block("Detail"))
            .alignment(Alignment::Center);
        frame.render_widget(p, cols[1]);
    }
}

fn draw_memory(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(0)])
        .split(area);

    let head = app
        .health_subject
        .chain
        .map(|(l, d)| (l, hex::encode(d)))
        .unwrap_or((0, String::new()));
    let rows = vec![
        row2("chain length", &head.0.to_string()),
        row2("chain head", &head.1),
        row2(
            "backend",
            "SurrealDB embedded (kv-mem)\n  options: kv-surrealkv, kv-rocksdb",
        ),
        row2(
            "FTS analyzer",
            "lower_alphanum (class tokenizer, lowercase+ascii filters)",
        ),
        row2(
            "HNSW index",
            "DIMENSION 384 TYPE F32 DISTANCE COSINE M 16 EFC 200",
        ),
    ];
    let t = Table::new(rows, [Constraint::Length(20), Constraint::Min(0)])
        .block(section_block("Trinity Memory — Chain & Indices"));
    frame.render_widget(t, chunks[0]);

    let info = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "BM25 keyword recall",
            Style::default().add_modifier(Modifier::BOLD).fg(ACCENT),
        )]),
        Line::from("  SurrealQL `@0@` over the FTS analyzer; score = search::score(0)."),
        Line::from(""),
        Line::from(vec![Span::styled(
            "HNSW vector recall",
            Style::default().add_modifier(Modifier::BOLD).fg(ACCENT),
        )]),
        Line::from("  SurrealQL `<|k|>`; score = 1 - vector::distance::knn() (closer = higher)."),
        Line::from(""),
        Line::from(vec![Span::styled(
            "K-LRU prefix tree",
            Style::default().add_modifier(Modifier::BOLD).fg(ACCENT),
        )]),
        Line::from("  K = 128; capacity = 512; cache caps worst-case rewind at K deltas."),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Hybrid recall",
            Style::default().add_modifier(Modifier::BOLD).fg(ACCENT),
        )]),
        Line::from("  ρ_hyb(α) = α · ρ_fts ∪ (1-α) · ρ_vec, deduplicated by turn_id."),
    ])
    .block(section_block("Recall Surfaces"))
    .wrap(Wrap { trim: true });
    frame.render_widget(info, chunks[1]);
}

fn draw_sandbox(frame: &mut Frame<'_>, area: Rect, _app: &App) {
    let rows = vec![
        Row::new(vec![
            Cell::from("FILESYSTEM_READ"),
            Cell::from("L1"),
            Cell::from("WASM (wasmi 0.46, fuel-metered)"),
        ]),
        Row::new(vec![
            Cell::from("CANVAS_RENDER"),
            Cell::from("L1"),
            Cell::from("WASM"),
        ]),
        Row::new(vec![
            Cell::from("FILESYSTEM_WRITE"),
            Cell::from("L2"),
            Cell::from("WASM + Landlock 5.13+ / Seatbelt"),
        ]),
        Row::new(vec![
            Cell::from("NETWORK_GET"),
            Cell::from("L2"),
            Cell::from("WASM + Landlock / Seatbelt"),
        ]),
        Row::new(vec![
            Cell::from("CANVAS_EMBED"),
            Cell::from("L2"),
            Cell::from("WASM + Landlock / Seatbelt"),
        ]),
        Row::new(vec![
            Cell::from("NETWORK_POST"),
            Cell::from("L3"),
            Cell::from("+ bubblewrap (ns) + seccompiler"),
        ]),
        Row::new(vec![
            Cell::from("SUBPROCESS_SPAWN"),
            Cell::from("L3"),
            Cell::from("+ bubblewrap + seccompiler"),
        ]),
        Row::new(vec![
            Cell::from("CRYPTO_SIGN"),
            Cell::from("L4"),
            Cell::from("+ TEE attestation (Phase 10 simulator; hardware = plugin)"),
        ]),
    ];
    let t = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Length(6),
            Constraint::Min(0),
        ],
    )
    .header(
        Row::new(vec!["Capability", "Class", "Layers required"])
            .style(Style::default().add_modifier(Modifier::BOLD).fg(ACCENT)),
    )
    .block(section_block(
        "Composite Sandbox — `gauss_traits::min_sandbox_for`",
    ));
    frame.render_widget(t, area);
}

fn draw_sag(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let pending: Vec<crate::app::PendingApproval> = app
        .pending
        .try_lock()
        .map(|q| q.iter().cloned().collect())
        .unwrap_or_default();
    let items: Vec<ListItem<'_>> = pending
        .iter()
        .map(|p| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("#{:>5}", p.request.turn_id.as_u128()),
                    Style::default().fg(ACCENT),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:?}", p.request.risk),
                    Style::default().fg(WARN).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::raw(p.request.action.tool.0.clone()),
                Span::raw("  "),
                Span::styled(
                    p.request.reason.clone(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let mut state = ListState::default();
    if !pending.is_empty() {
        state.select(Some(app.pending_cursor.min(pending.len() - 1)));
    }
    let list = List::new(items)
        .block(section_block(&format!(
            "Approval queue ({} pending)",
            pending.len()
        )))
        .highlight_style(Style::default().bg(ACCENT).fg(Color::Black))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, cols[0], &mut state);

    // Right side: decision table + selected request detail.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(cols[1]);

    let table_rows = vec![
        row2("adversarial taint", "→ Deny"),
        row2("CRYPTO_SIGN", "→ RequireApproval"),
        row2("¬rev ∧ (NET_POST ∨ SPAWN)", "→ RequireApproval"),
        row2("¬rev ∨ Web taint", "→ Notify"),
        row2("else", "→ Auto"),
    ];
    let dt = Table::new(table_rows, [Constraint::Length(32), Constraint::Min(0)])
        .block(section_block("Decision table (paper §XI.B)"));
    frame.render_widget(dt, split[0]);

    let detail = pending.get(app.pending_cursor).map(|p| {
        let args_json =
            serde_json::to_string_pretty(&p.request.action.args).unwrap_or_else(|_| "—".into());
        let rows = vec![
            row2("turn", &p.request.turn_id.as_u128().to_string()),
            row2("tool", &p.request.action.tool.0),
            row2(
                "cap_required",
                &format!("0x{:016X}", p.request.action.cap_required.bits()),
            ),
            row2(
                "reversible",
                if p.request.action.reversible {
                    "yes"
                } else {
                    "no"
                },
            ),
            row2("risk", &format!("{:?}", p.request.risk)),
            row2("reason", &p.request.reason),
            row2("args", &args_json),
        ];
        Table::new(rows, [Constraint::Length(14), Constraint::Min(0)])
            .block(section_block("Request detail — [a] approve · [d] deny"))
    });
    if let Some(t) = detail {
        frame.render_widget(t, split[1]);
    } else {
        let p = Paragraph::new(
            "No pending approvals. Press [s] to seed a demo request, or [t] on the\nDashboard to run a turn that may trigger one.",
        )
        .block(section_block("Request detail"))
        .alignment(Alignment::Center);
        frame.render_widget(p, split[1]);
    }
}

fn draw_health(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let ok = app
        .last_health
        .invariants
        .iter()
        .filter(|i| matches!(i.verdict, Verdict::Ok))
        .count();
    let warn = app
        .last_health
        .invariants
        .iter()
        .filter(|i| matches!(i.verdict, Verdict::Warning))
        .count();
    let fail = app
        .last_health
        .invariants
        .iter()
        .filter(|i| matches!(i.verdict, Verdict::Failing))
        .count();
    let total = app.last_health.invariants.len().max(1);
    let pct = (ok * 100) / total;
    let gauge = Gauge::default()
        .block(section_block(&format!(
            "Overall health — {ok}/{} OK · {warn} warn · {fail} fail",
            app.last_health.invariants.len()
        )))
        .gauge_style(if fail > 0 {
            Style::default().fg(ERR)
        } else if warn > 0 {
            Style::default().fg(WARN)
        } else {
            Style::default().fg(OK)
        })
        .percent(u16::try_from(pct).unwrap_or(0))
        .label(format!("{pct} %"));
    frame.render_widget(gauge, chunks[0]);

    let items: Vec<ListItem<'_>> = app
        .last_health
        .invariants
        .iter()
        .map(|o| {
            let (sym, c) = match o.verdict {
                Verdict::Ok => ("✓", OK),
                Verdict::Warning => ("!", WARN),
                Verdict::Failing => ("✗", ERR),
                // `Verdict` is `#[non_exhaustive]`; unknown variants are
                // shown as warnings so the operator notices but the UI
                // doesn't panic.
                _ => ("?", WARN),
            };
            let detail = o
                .detail
                .as_deref()
                .map(|d| format!("  — {d}"))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {sym} "),
                    Style::default().fg(c).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:30}", o.id), Style::default().fg(ACCENT)),
                Span::raw(format!(" {}", o.description)),
                Span::styled(detail, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let list = List::new(items).block(section_block("Invariants (gauss-health)"));
    frame.render_widget(list, chunks[1]);
}

fn draw_cluster(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(0)])
        .split(area);

    let rows = vec![
        row2("nodes", &app.ring.node_count().to_string()),
        row2("vnodes/node", "128 (DEFAULT_VNODES)"),
        row2("hash", "SHA-256 prefix → u64 ring key"),
        row2("session-key under test", &app.cluster_test_key),
        row2("routes to", app.cluster_test_node.as_deref().unwrap_or("—")),
    ];
    let t = Table::new(rows, [Constraint::Length(28), Constraint::Min(0)])
        .block(section_block("ConsistentHashRing (Theorem T6)"));
    frame.render_widget(t, chunks[0]);

    let items: Vec<ListItem<'_>> = app
        .ring
        .nodes()
        .iter()
        .map(|n| {
            ListItem::new(Line::from(vec![
                Span::styled("● ", Style::default().fg(OK)),
                Span::raw(n.0.clone()),
            ]))
        })
        .collect();
    let list = List::new(items).block(section_block("Active nodes — [n] add  [x] remove"));
    frame.render_widget(list, chunks[1]);
}

fn draw_audit(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let head = app
        .health_subject
        .chain
        .map(|(l, d)| (l, hex::encode(d)))
        .unwrap_or((0, String::new()));
    let rows = vec![
        row2("chain primitive", "SHA-256(prev_head ‖ payload) (Phase 2)"),
        row2("signed receipts", "Ed25519 (Phase 5) — ed25519-dalek 2.x"),
        row2(
            "anchor cadence",
            "AnchorPolicy::SPECS_DEFAULT = every 1000 appends",
        ),
        row2(
            "anchor kinds",
            "Rfc3161, OpenTimestamps, Simulator (Phase 5)",
        ),
        row2(
            "verifier API",
            "verify_receipt · verify_chain · verify_anchor_replay",
        ),
        row2("current chain length", &head.0.to_string()),
        row2("current chain head", &head.1),
    ];
    let t = Table::new(rows, [Constraint::Length(22), Constraint::Min(0)])
        .block(section_block("Audit chain (gauss-audit)"));
    frame.render_widget(t, area);
}

fn draw_scorecard(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let pred = &app.predecessor_scorecards[app.scorecard_focus % 4];
    let cmp = app.me_scorecard.compare(pred);
    let rows: Vec<Row<'_>> = cmp
        .iter()
        .map(|r| {
            let (sym, c) = match r.verdict {
                AxisVerdict::Better => ("▲", OK),
                AxisVerdict::Equal => ("=", Color::Gray),
                AxisVerdict::Worse => ("▼", ERR),
                // `AxisVerdict` is `#[non_exhaustive]`; treat unknown
                // variants as warnings.
                _ => ("?", WARN),
            };
            Row::new(vec![
                Cell::from(Span::raw(r.axis.label())),
                Cell::from(Span::raw(format!("{:>6.2}", r.self_value))),
                Cell::from(Span::raw(format!("{:>6.2}", r.other_value))),
                Cell::from(Span::styled(
                    format!(" {sym} "),
                    Style::default().fg(c).add_modifier(Modifier::BOLD),
                )),
            ])
        })
        .collect();
    let t = Table::new(
        rows,
        [
            Constraint::Length(24),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(6),
        ],
    )
    .header(
        Row::new(vec!["Axis", "1.0", &pred.system, "Δ"])
            .style(Style::default().add_modifier(Modifier::BOLD).fg(ACCENT)),
    )
    .block(section_block(&format!(
        "15-axis Pareto-dominance (vs `{}`) — [←→] cycle predecessor",
        pred.system
    )));
    frame.render_widget(t, area);
}

fn draw_logs(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem<'_>> = app
        .logs
        .iter()
        .rev()
        .take(usize::from(area.height).saturating_sub(2))
        .map(|l| {
            let c = match l.level {
                "ERROR" => ERR,
                "WARN" => WARN,
                _ => OK,
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("[{:5}]", l.level),
                    Style::default().fg(c).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::raw(l.message.clone()),
            ]))
        })
        .collect();
    let list = List::new(items).block(section_block(&format!("Logs ({} lines)", app.logs.len())));
    frame.render_widget(list, area);
}

fn draw_help_overlay(frame: &mut Frame<'_>, area: Rect) {
    let popup = centered_rect(72, 70, area);
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(vec![Span::styled(
            "Gauss-Aether TUI — keybindings",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Global",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Tab / Shift+Tab          cycle tabs"),
        Line::from("  1 2 3 4 5 6 7 8 9 0      jump to tab by number"),
        Line::from("  ?                        toggle this overlay"),
        Line::from("  q / Ctrl-C / Esc-Esc     quit"),
        Line::from("  r                        refresh current tab's polled state"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Dashboard",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  t                        run one demo turn through the engine"),
        Line::from("  a                        seed a demo approval request"),
        Line::from("  /                        cycle the default taint band"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Turns / SAG / Logs",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  ↑ ↓ / j k                navigate the highlighted list"),
        Line::from("  a (in SAG)               approve the highlighted pending request"),
        Line::from("  d (in SAG)               deny the highlighted pending request"),
        Line::from("  s (in SAG)               seed a demo approval request"),
        Line::from("  c (in Logs)              clear the log buffer"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Cluster",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  n                        add an auto-named cluster node"),
        Line::from("  x                        remove the most-recently-added node"),
        Line::from("  r                        route the test session key"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Health",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  r                        re-evaluate the seven invariants"),
        Line::from("  g                        force kernel.grant = 0 (demo failing invariant)"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Scorecard",
            Style::default().fg(WARN).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  ← →                      cycle through the four predecessors"),
    ];
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .padding(Padding::new(2, 2, 1, 1))
                .border_style(Style::default().fg(ACCENT))
                .title(" Help "),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(p, popup);
}

// ---------- helpers -----------------------------------------------------

fn section_block(title: impl AsRef<str>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_set(symbols::border::ROUNDED)
        .border_style(Style::default().fg(ACCENT_DIM))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                title.as_ref().to_owned(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
        .padding(Padding::new(1, 1, 0, 0))
}

fn kpi(label: impl AsRef<str>, value: impl AsRef<str>, color: Color) -> Paragraph<'static> {
    let block = section_block(label);
    let body = Line::from(vec![Span::styled(
        value.as_ref().to_owned(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )]);
    Paragraph::new(vec![Line::raw(""), body])
        .alignment(Alignment::Center)
        .block(block)
}

fn row2(k: impl AsRef<str>, v: impl AsRef<str>) -> Row<'static> {
    Row::new(vec![
        Cell::from(Span::styled(
            k.as_ref().to_owned(),
            Style::default().fg(ACCENT_DIM),
        )),
        Cell::from(Span::raw(v.as_ref().to_owned())),
    ])
}

const fn taint_label(t: gauss_core::TaintLabel) -> &'static str {
    use gauss_core::TaintLabel as L;
    match t {
        L::Trusted => "Trusted",
        L::User => "User",
        L::Web => "Web",
        L::Adversarial => "Adversarial",
    }
}

const fn taint_color(t: gauss_core::TaintLabel) -> Color {
    use gauss_core::TaintLabel as L;
    match t {
        L::Trusted => OK,
        L::User => ACCENT,
        L::Web => WARN,
        L::Adversarial => ERR,
    }
}

fn short_hex(hex: &str) -> String {
    if hex.len() <= 12 {
        hex.to_owned()
    } else {
        format!("{}…{}", &hex[..6], &hex[hex.len() - 4..])
    }
}

fn centered_rect(pct_x: u16, pct_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// Imports are all used at point-of-use; no stub forcers needed.
