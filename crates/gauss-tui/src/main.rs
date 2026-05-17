//! `gauss-tui` — Ratatui admin console for Gauss-Aether.
//!
//! Run `cargo run -p gauss-tui` to launch. The TUI binds against a
//! live in-process Gauss-Aether engine (kernel + memory + SAG +
//! health + cluster + scorecard); every panel reads (and a few mutate)
//! the real state.

// UI code has different style constraints from kernel code — many
// pedantic lints (arithmetic-side-effects on cursor advance, too-many-
// lines on tab-dispatch matches, missing-docs on internal helpers, etc.)
// would obscure rather than improve the layout. The workspace's
// `unsafe_code = "forbid"` lint still applies.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::missing_docs_in_private_items,
    clippy::too_many_lines,
    clippy::module_name_repetitions,
    clippy::similar_names,
    clippy::unused_async,
    clippy::missing_errors_doc,
    clippy::struct_field_names,
    clippy::needless_pass_by_value,
    clippy::if_not_else,
    clippy::redundant_closure_for_method_calls,
    clippy::doc_markdown,
    clippy::option_if_let_else,
    clippy::match_same_arms,
    clippy::single_match_else,
    clippy::collapsible_else_if,
    clippy::ignored_unit_patterns,
    clippy::needless_continue,
    clippy::manual_let_else,
    clippy::unreadable_literal,
    clippy::large_stack_arrays,
    clippy::wildcard_imports,
    clippy::too_long_first_doc_paragraph,
    clippy::manual_unwrap_or_default,
    clippy::redundant_else,
    clippy::useless_let_if_seq,
    clippy::option_as_ref_deref,
    clippy::single_match,
    clippy::needless_borrows_for_generic_args,
    clippy::map_unwrap_or,
    clippy::future_not_send,
    dead_code,
    unused_imports,
    unreachable_pub,
    missing_docs
)]

mod app;
mod ui;

use std::io;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use gauss_sag::ApprovalDecision;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::time::Instant;

use crate::app::{App, Tab};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Build the app state (boots the live engine + drain task).
    let mut app = App::boot().await?;
    app.refresh().await;

    // 2. Set up the terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let res = run_event_loop(&mut term, &mut app).await;

    // 3. Restore the terminal.
    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    term.show_cursor()?;

    res
}

async fn run_event_loop<B: ratatui::backend::Backend>(
    term: &mut Terminal<B>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let tick = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    loop {
        // Render.
        term.draw(|f| ui::draw(f, app))?;

        // Poll keyboard.
        let elapsed: Duration = last_tick.elapsed();
        let timeout = tick
            .checked_sub(elapsed)
            .unwrap_or(Duration::from_millis(10));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.show_help {
                        // While the help overlay is up, every key closes
                        // it (this includes `?` toggling and `Esc`).
                        app.show_help = false;
                        continue;
                    }
                    handle_key(app, key.code, key.modifiers).await;
                }
            }
        }

        if last_tick.elapsed() >= Duration::from_millis(250) {
            app.refresh().await;
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    // Global: quit + help + tab cycle.
    match code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            return;
        }
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
            return;
        }
        KeyCode::Char('?') => {
            app.show_help = !app.show_help;
            return;
        }
        KeyCode::Tab => {
            app.tab = app.tab.next();
            return;
        }
        KeyCode::BackTab => {
            app.tab = app.tab.prev();
            return;
        }
        KeyCode::Char(c @ '0'..='9') => {
            if let Some(t) = Tab::all().iter().find(|t| t.shortcut() == c) {
                app.tab = *t;
                return;
            }
        }
        _ => {}
    }

    // Per-tab keys.
    match app.tab {
        Tab::Dashboard => match code {
            KeyCode::Char('t') => app.run_demo_turn().await,
            KeyCode::Char('r') => app.refresh().await,
            KeyCode::Char('a') => app.seed_demo_approval().await,
            KeyCode::Char('/') => app.cycle_taint(),
            _ => {}
        },
        Tab::Turns => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                if !app.turn_history.is_empty() {
                    app.turn_cursor = (app.turn_cursor + 1).min(app.turn_history.len() - 1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.turn_cursor = app.turn_cursor.saturating_sub(1);
            }
            KeyCode::Char('r') => app.refresh().await,
            _ => {}
        },
        Tab::Memory => {
            if matches!(code, KeyCode::Char('r')) {
                app.refresh().await;
            }
        }
        Tab::Sandbox => {}
        Tab::Sag => match code {
            KeyCode::Down | KeyCode::Char('j') => {
                let len = app.pending.lock().await.len();
                if len > 0 {
                    app.pending_cursor = (app.pending_cursor + 1).min(len - 1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.pending_cursor = app.pending_cursor.saturating_sub(1);
            }
            KeyCode::Char('a') => {
                app.decide_pending(ApprovalDecision::Approved {
                    approver: "tui-operator".into(),
                })
                .await;
                app.flash = Some(("approved".into(), "ok"));
            }
            KeyCode::Char('d') => {
                app.decide_pending(ApprovalDecision::Denied {
                    approver: "tui-operator".into(),
                    reason: Some("denied via TUI".into()),
                })
                .await;
                app.flash = Some(("denied".into(), "warn"));
            }
            KeyCode::Char('s') => app.seed_demo_approval().await,
            _ => {}
        },
        Tab::Health => match code {
            KeyCode::Char('r') => app.refresh().await,
            KeyCode::Char('g') => {
                // Demo: force grant to 0 by writing it into the health
                // subject. The kernel itself can't downgrade to 0 (BOTTOM)
                // without breaking other admit checks; this is a
                // visualisation aid so operators can see what a failing
                // invariant looks like.
                app.health_subject.grant = 0;
                app.last_health = app.health.evaluate(&app.health_subject);
                app.flash = Some(("forced grant=0 (demo)".into(), "warn"));
            }
            _ => {}
        },
        Tab::Cluster => match code {
            KeyCode::Char('n') => {
                let name = format!("gauss-{}.demo", app.ring.node_count() + 1);
                app.add_cluster_node(&name);
                app.flash = Some((format!("added {name}"), "ok"));
                app.route_cluster();
            }
            KeyCode::Char('x') => {
                let nodes = app.ring.nodes();
                if let Some(last) = nodes.last() {
                    let n = last.0.clone();
                    app.remove_cluster_node(&n);
                    app.flash = Some((format!("removed {n}"), "warn"));
                    app.route_cluster();
                }
            }
            KeyCode::Char('r') => {
                app.route_cluster();
            }
            _ => {}
        },
        Tab::Audit => {
            if matches!(code, KeyCode::Char('r')) {
                app.refresh().await;
            }
        }
        Tab::Scorecard => match code {
            KeyCode::Left | KeyCode::Char('h') => {
                app.scorecard_focus = if app.scorecard_focus == 0 {
                    3
                } else {
                    app.scorecard_focus - 1
                };
            }
            KeyCode::Right | KeyCode::Char('l') => {
                app.scorecard_focus = (app.scorecard_focus + 1) % 4;
            }
            _ => {}
        },
        Tab::Logs => match code {
            KeyCode::Char('c') => {
                app.logs.clear();
                app.flash = Some(("logs cleared".into(), "ok"));
            }
            _ => {}
        },
    }
}
