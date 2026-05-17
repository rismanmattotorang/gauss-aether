//! `gaussclaw-tui` — Ratatui + crossterm interactive shell.
//!
//! Phase 1 Task 3 of `GAUSSCLAW_ROADMAP.md`. Replaces the upstream
//! Hermes React + Ink TUI with a Rust-native terminal application.
//!
//! ## Layout
//!
//! ```text
//!  ┌───────────────────────────── GaussClaw v0.0.1 ──────────────────────────┐
//!  │ session=…  model=…  turn=…  chain=…  taint=⊥  caps=…                    │ ← status bar
//!  ├──────────────────────────────────────────────────────────────────────────┤
//!  │ history pane (scrollable)                                                │
//!  │   • user / assistant / system entries                                    │
//!  │                                                                          │
//!  ├──────────────────────────────────────────────────────────────────────────┤
//!  │ > input area (multiline; Shift+Enter newline)                            │
//!  └──────── Enter submit · Ctrl+C quit · Ctrl+L clear · /help help ──────────┘
//! ```
//!
//! This crate ships the [`App`] state machine, the render pipeline, and a
//! [`run`] entry point that wraps the terminal-setup/teardown ceremony.
//! Snapshot tests in `tests/snapshots.rs` lock the rendered output via
//! `ratatui::backend::TestBackend` + `insta`, satisfying the TUI snapshot
//! class of `gaussclaw-conformance` (Phase 1 conformance gate #4).

#![allow(clippy::doc_markdown)]

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use ratatui::Terminal;
use tui_textarea::TextArea;

// ─── status bar info ────────────────────────────────────────────────────────

/// Status-bar payload — what the top status row renders.
///
/// The GaussClaw-specific fields are `chain_head` and `taint_floor`: they
/// reflect kernel state, which the upstream Hermes TUI cannot display.
#[derive(Debug, Clone)]
pub struct StatusInfo {
    /// Active session id (short form).
    pub session: String,
    /// Active provider/model string.
    pub model: String,
    /// Turn counter for the current session.
    pub turn: u64,
    /// First eight hex chars of the receipt-chain head digest.
    pub chain_head: String,
    /// Current taint floor for the session (`⊥` / `user` / `web` / `adversarial`).
    pub taint_floor: String,
    /// Count of granted capabilities.
    pub caps: u32,
}

impl Default for StatusInfo {
    fn default() -> Self {
        Self {
            session: "new".into(),
            model: "(unset)".into(),
            turn: 0,
            chain_head: "00000000".into(),
            taint_floor: "⊥".into(),
            caps: 0,
        }
    }
}

// ─── history ────────────────────────────────────────────────────────────────

/// A single line in the conversation pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// User-submitted prompt.
    User(String),
    /// Assistant (model) response.
    Assistant(String),
    /// System-emitted notice (slash command output, errors).
    System(String),
}

// ─── outcome of a key event ─────────────────────────────────────────────────

/// What `App::on_key` tells the event loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tick {
    /// Re-render and keep going.
    Continue,
    /// Re-render once and exit cleanly.
    Quit,
}

// ─── the app ────────────────────────────────────────────────────────────────

/// The TUI state machine.
pub struct App<'a> {
    history: Vec<Entry>,
    input: TextArea<'a>,
    status: StatusInfo,
    /// Vertical scroll offset (0 = bottom-pinned).
    scroll_offset: u16,
}

impl App<'_> {
    /// Build a fresh app with the given status bar payload.
    #[must_use]
    pub fn new(status: StatusInfo) -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(Style::default());
        input.set_placeholder_text(
            "Type a message. Enter to submit · Shift+Enter to insert newline · /help for commands.",
        );
        Self {
            history: vec![Entry::System(
                "Welcome to GaussClaw. The TUI skeleton lands in this slice; \
                 real agent dispatch arrives later in Phase 1. Try `/help`."
                    .into(),
            )],
            input,
            status,
            scroll_offset: 0,
        }
    }

    /// Push an entry onto the history pane.
    pub fn push(&mut self, entry: Entry) {
        self.history.push(entry);
    }

    /// Replace the status bar payload (called when kernel state changes).
    pub fn set_status(&mut self, status: StatusInfo) {
        self.status = status;
    }

    /// Read-only access to the history (for tests).
    #[must_use]
    pub fn history(&self) -> &[Entry] {
        &self.history
    }

    /// Process one key event. Returns whether the loop should continue.
    pub fn on_key(&mut self, key: KeyEvent) -> Tick {
        if key.kind != KeyEventKind::Press {
            return Tick::Continue;
        }

        // Global keybindings first.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c' | 'd') => return Tick::Quit,
                KeyCode::Char('l') => {
                    self.history.clear();
                    self.history.push(Entry::System("Session cleared.".into()));
                    return Tick::Continue;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.submit_current();
                return Tick::Continue;
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                return Tick::Continue;
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                return Tick::Continue;
            }
            _ => {}
        }

        // Otherwise hand off to the textarea (multiline editor).
        self.input.input(key);
        Tick::Continue
    }

    /// Submit the current input as a user turn. Splits slash commands out
    /// into [`Self::dispatch_slash`].
    fn submit_current(&mut self) {
        let body = self.input.lines().join("\n").trim_end().to_string();
        if body.is_empty() {
            return;
        }
        // Clear the editor.
        self.input = TextArea::default();
        self.input.set_cursor_line_style(Style::default());
        self.input.set_placeholder_text(
            "Type a message. Enter to submit · Shift+Enter to insert newline · /help for commands.",
        );

        if let Some(cmd) = body.strip_prefix('/') {
            self.dispatch_slash(cmd);
            return;
        }

        // Echo the user turn, then a stub assistant turn.
        self.history.push(Entry::User(body));
        self.history.push(Entry::Assistant(
            "(stub) gaussclaw-tui does not yet dispatch to a provider; \
             real agent execution lands later in Phase 1 (Task 9, three-plane routing)."
                .into(),
        ));
        self.status.turn = self.status.turn.saturating_add(1);
    }

    /// Dispatch a slash command. Phase 1 ships `/help`, `/quit`, `/clear`;
    /// the rest stub with the phase that lands them.
    fn dispatch_slash(&mut self, cmd: &str) {
        let (head, rest) = cmd
            .split_once(char::is_whitespace)
            .map_or((cmd, ""), |(h, r)| (h.trim(), r.trim()));
        let body = match head {
            "help" => HELP_TEXT.into(),
            "quit" | "exit" => {
                self.history
                    .push(Entry::System("Bye. (Use Ctrl+C any time.)".into()));
                return;
            }
            "clear" | "new" => {
                self.history.clear();
                self.history.push(Entry::System("Session cleared.".into()));
                return;
            }
            "receipt" | "taint" | "caps" | "sandbox" => {
                format!(
                    "/{head}: not yet implemented (Phase 2 / 3 deliverable; see GAUSSCLAW_ROADMAP.md)."
                )
            }
            "model" | "tools" | "config" | "logs" | "statusbar" | "queue" | "undo" | "retry"
            | "copy" | "paste" | "details" | "compact" | "resume" => {
                format!("/{head} {rest}: not yet implemented (later Phase 1 slice).")
            }
            _ => format!("Unknown command: /{head}. Try /help."),
        };
        self.history.push(Entry::System(body));
    }

    /// Render the whole UI.
    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(1), // status bar
                Constraint::Min(3),    // history
                Constraint::Length(5), // input
            ])
            .split(area);

        self.render_title(frame, chunks[0]);
        self.render_status(frame, chunks[1]);
        self.render_history(frame, chunks[2]);
        self.render_input(frame, chunks[3]);
    }

    #[allow(clippy::unused_self)]
    fn render_title(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " GaussClaw ",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Black)
                    .bg(Color::Cyan),
            ),
            Span::styled(
                format!(" v{} ", env!("CARGO_PKG_VERSION")),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(" — Hermes port on Gauss-Aether "),
        ]));
        frame.render_widget(title, area);
    }

    fn render_status(&self, frame: &mut Frame<'_>, area: Rect) {
        let s = &self.status;
        let row = Paragraph::new(Line::from(vec![
            Span::styled(" session=", Style::default().fg(Color::DarkGray)),
            Span::styled(s.session.clone(), Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled("model=", Style::default().fg(Color::DarkGray)),
            Span::styled(s.model.clone(), Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled("turn=", Style::default().fg(Color::DarkGray)),
            Span::styled(s.turn.to_string(), Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled("chain=", Style::default().fg(Color::DarkGray)),
            Span::styled(
                s.chain_head.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            ),
            Span::raw("  "),
            Span::styled("taint=", Style::default().fg(Color::DarkGray)),
            Span::styled(
                s.taint_floor.clone(),
                Style::default().fg(taint_colour(&s.taint_floor)),
            ),
            Span::raw("  "),
            Span::styled("caps=", Style::default().fg(Color::DarkGray)),
            Span::styled(s.caps.to_string(), Style::default().fg(Color::White)),
        ]));
        frame.render_widget(row, area);
    }

    fn render_history(&self, frame: &mut Frame<'_>, area: Rect) {
        let lines: Vec<Line<'_>> = self
            .history
            .iter()
            .flat_map(|e| match e {
                Entry::User(body) => prefixed_lines("›", body, Color::Green),
                Entry::Assistant(body) => prefixed_lines("◀", body, Color::Cyan),
                Entry::System(body) => prefixed_lines("·", body, Color::DarkGray),
            })
            .collect();

        let block = Block::default()
            .borders(Borders::TOP | Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray));
        let widget = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_offset, 0));
        frame.render_widget(widget, area);
    }

    fn render_input(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" input ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(&self.input, inner);
    }
}

fn taint_colour(t: &str) -> Color {
    match t {
        "⊥" | "trusted" => Color::Green,
        "user" => Color::Cyan,
        "web" => Color::Yellow,
        "adversarial" => Color::Red,
        _ => Color::White,
    }
}

fn prefixed_lines(prefix: &str, body: &str, colour: Color) -> Vec<Line<'static>> {
    body.split('\n')
        .map(|line| {
            Line::from(vec![
                Span::styled(
                    format!(" {prefix} "),
                    Style::default().fg(colour).add_modifier(Modifier::BOLD),
                ),
                Span::raw(line.to_string()),
            ])
        })
        .collect()
}

/// In-app help body for `/help`.
const HELP_TEXT: &str = "\
Slash commands:
  /help          show this help
  /quit, /exit   exit the TUI (or Ctrl+C / Ctrl+D)
  /clear, /new   wipe the visible history (also Ctrl+L)
  /receipt       Phase 2: show receipt-chain head
  /taint         Phase 3: show current taint floor + per-token labels
  /caps          Phase 3: show current capability set
  /sandbox       Phase 3: per-tool sandbox layer status

Keybindings:
  Enter          submit current input
  Shift+Enter    insert newline
  Ctrl+C, Ctrl+D quit
  Ctrl+L         clear history
  PageUp/Down    scroll history
";

// ─── public render hook used by the conformance snapshot tests ──────────────

/// Render the app once into a buffer of the given size. Used by snapshot
/// tests via `ratatui::backend::TestBackend`; the production loop goes
/// through [`run`] instead.
#[must_use]
pub fn snapshot_render(app: &App<'_>, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| app.render(frame)).expect("draw");
    terminal.backend().buffer().clone()
}

/// Pretty-print a Ratatui buffer to a `String` (one cell per char, ignoring
/// styling). Stable across terminals — suitable for `insta` snapshots.
#[must_use]
pub fn buffer_text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

// ─── run loop ───────────────────────────────────────────────────────────────

/// Run the TUI synchronously until the user quits.
///
/// Owns the terminal setup and teardown: enters the alternate screen,
/// enables raw mode + bracketed paste, and restores them on exit (even
/// on panic, via an internal Drop guard).
pub fn run(initial: StatusInfo) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(initial);

    run_event_loop(&mut terminal, &mut app)?;
    Ok(())
}

fn run_event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App<'_>) -> Result<()> {
    loop {
        terminal.draw(|f| app.render(f))?;
        if crossterm::event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = crossterm::event::read()? {
                if app.on_key(key) == Tick::Quit {
                    terminal.draw(|f| app.render(f))?;
                    return Ok(());
                }
            }
        }
    }
}

/// RAII restorer for the terminal mode set by [`run`]. Runs on drop, so the
/// terminal returns to normal even if the loop panics.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste);
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    fn type_str(app: &mut App<'_>, s: &str) {
        for c in s.chars() {
            app.on_key(key(c));
        }
    }

    #[test]
    fn welcome_entry_present() {
        let app = App::new(StatusInfo::default());
        assert_eq!(app.history().len(), 1);
        matches::<&Entry, _>(&app.history()[0], |e| matches!(e, Entry::System(_)));
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = App::new(StatusInfo::default());
        assert_eq!(app.on_key(ctrl('c')), Tick::Quit);
    }

    #[test]
    fn ctrl_l_clears_history() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "hi");
        app.on_key(enter());
        assert!(app.history().len() > 1);
        assert_eq!(app.on_key(ctrl('l')), Tick::Continue);
        // Cleared then a fresh system notice pushed.
        assert_eq!(app.history().len(), 1);
        matches::<&Entry, _>(&app.history()[0], |e| matches!(e, Entry::System(_)));
    }

    #[test]
    fn enter_submits_user_turn_and_stub_assistant() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "hello");
        app.on_key(enter());
        let h = app.history();
        // welcome + user + assistant
        assert_eq!(h.len(), 3);
        assert_eq!(h[1], Entry::User("hello".into()));
        matches::<&Entry, _>(&h[2], |e| matches!(e, Entry::Assistant(_)));
        // Turn counter advanced.
        assert_eq!(app.status.turn, 1);
    }

    #[test]
    fn empty_enter_is_a_noop() {
        let mut app = App::new(StatusInfo::default());
        let before = app.history().len();
        app.on_key(enter());
        assert_eq!(app.history().len(), before);
    }

    #[test]
    fn slash_help_is_recognised() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "/help");
        app.on_key(enter());
        let last = app.history().last().expect("entry");
        match last {
            Entry::System(body) => assert!(body.contains("Slash commands")),
            other => panic!("expected System, got {other:?}"),
        }
    }

    #[test]
    fn slash_clear_wipes_history() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "warmup");
        app.on_key(enter());
        assert!(app.history().len() > 1);
        type_str(&mut app, "/clear");
        app.on_key(enter());
        assert_eq!(app.history().len(), 1);
    }

    #[test]
    fn unknown_slash_command_emits_hint() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "/nope");
        app.on_key(enter());
        let last = app.history().last().expect("entry");
        match last {
            Entry::System(body) => assert!(body.contains("Unknown command")),
            other => panic!("expected System, got {other:?}"),
        }
    }

    /// Helper for matches-with-message asserts.
    fn matches<T, F: FnOnce(T) -> bool>(v: T, f: F) {
        assert!(f(v), "match failed");
    }

    #[test]
    fn welcome_snapshot_is_stable() {
        let app = App::new(StatusInfo {
            session: "s1".into(),
            model: "anthropic/claude-3.5-sonnet".into(),
            turn: 0,
            chain_head: "deadbeef".into(),
            taint_floor: "⊥".into(),
            caps: 3,
        });
        let buf = snapshot_render(&app, 80, 12);
        insta::assert_snapshot!("welcome_screen", buffer_text(&buf));
    }

    #[test]
    fn one_turn_snapshot_is_stable() {
        let mut app = App::new(StatusInfo {
            session: "s1".into(),
            model: "anthropic/claude-3.5-sonnet".into(),
            turn: 0,
            chain_head: "cafef00d".into(),
            taint_floor: "user".into(),
            caps: 5,
        });
        type_str(&mut app, "hello world");
        app.on_key(enter());
        let buf = snapshot_render(&app, 80, 14);
        insta::assert_snapshot!("after_one_turn", buffer_text(&buf));
    }
}
