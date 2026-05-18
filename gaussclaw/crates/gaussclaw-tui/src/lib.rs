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

#![allow(
    clippy::doc_markdown,
    clippy::unnested_or_patterns,
    clippy::arithmetic_side_effects,
    clippy::manual_let_else,
    clippy::manual_repeat_n,
    clippy::manual_string_new,
    clippy::manual_str_repeat,
    clippy::question_mark,
    clippy::redundant_closure_for_method_calls,
    clippy::option_if_let_else,
    clippy::assigning_clones
)]

mod history;
mod overlay;

pub use history::HistoryStore;
pub use overlay::{Overlay, OverlayResult};

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
    /// Persistent input history (`Up`/`Down` recall).
    input_history: HistoryStore,
    /// Last assistant text, for `/copy`.
    last_assistant: Option<String>,
}

impl App<'_> {
    /// Build a fresh app with the given status bar payload.
    ///
    /// Persists user-submitted lines to the platform's state directory; use
    /// [`Self::with_history`] in tests to keep everything in memory.
    #[must_use]
    pub fn new(status: StatusInfo) -> Self {
        Self::with_history(status, HistoryStore::open())
    }

    /// Build an app with a caller-supplied [`HistoryStore`]. Tests pass an
    /// in-memory store; production code uses [`Self::new`].
    #[must_use]
    pub fn with_history(status: StatusInfo, input_history: HistoryStore) -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(Style::default());
        input.set_placeholder_text(
            "Type a message. Enter to submit · Shift+Enter to insert newline · /help for commands.",
        );
        Self {
            history: vec![Entry::System(
                "Welcome to GaussClaw. Type a message, or `/help` for a tour of slash commands."
                    .into(),
            )],
            input,
            status,
            scroll_offset: 0,
            input_history,
            last_assistant: None,
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
                KeyCode::Char('p') => {
                    if let Some(line) = self.input_history.step_back().map(str::to_owned) {
                        self.set_input_to(&line);
                    }
                    return Tick::Continue;
                }
                KeyCode::Char('n') => {
                    if let Some(line) = self.input_history.step_forward().map(str::to_owned) {
                        self.set_input_to(&line);
                    } else {
                        self.clear_input();
                    }
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
            KeyCode::Up if self.input_is_empty_or_recalled() => {
                if let Some(line) = self.input_history.step_back().map(str::to_owned) {
                    self.set_input_to(&line);
                }
                return Tick::Continue;
            }
            KeyCode::Down if self.input_is_empty_or_recalled() => {
                if let Some(line) = self.input_history.step_forward().map(str::to_owned) {
                    self.set_input_to(&line);
                } else {
                    self.clear_input();
                }
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

    fn input_is_empty_or_recalled(&self) -> bool {
        // Up/Down behaves like a history walker only when the buffer is empty
        // or already showing a recalled entry — never when the user is in the
        // middle of composing.
        self.input.lines().iter().all(std::string::String::is_empty)
    }

    fn set_input_to(&mut self, body: &str) {
        let mut ta = TextArea::from(body.split('\n').collect::<Vec<&str>>());
        ta.set_cursor_line_style(Style::default());
        ta.set_placeholder_text(
            "Type a message. Enter to submit · Shift+Enter to insert newline · /help for commands.",
        );
        // Move cursor to end of buffer so the user can keep typing.
        ta.move_cursor(tui_textarea::CursorMove::End);
        self.input = ta;
    }

    fn clear_input(&mut self) {
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        ta.set_placeholder_text(
            "Type a message. Enter to submit · Shift+Enter to insert newline · /help for commands.",
        );
        self.input = ta;
    }

    /// Submit the current input as a user turn. Splits slash commands out
    /// into [`Self::dispatch_slash`].
    fn submit_current(&mut self) {
        let body = self.input.lines().join("\n").trim_end().to_string();
        if body.is_empty() {
            return;
        }
        // Clear the editor and remember the line for Up/Down recall.
        self.clear_input();
        self.input_history.push(&body);
        self.input_history.reset_cursor();

        if let Some(cmd) = body.strip_prefix('/') {
            self.dispatch_slash(cmd);
            return;
        }

        // Echo the user turn, then a stub assistant turn. Real dispatch lands
        // once `gaussclaw-agent` exposes a `run_turn` API to the surface plane.
        self.history.push(Entry::User(body));
        let reply = format!(
            "(stub) Real provider dispatch lands once `gaussclaw-agent::run_turn` is wired into the \
             surface plane. Current model: {model}, taint floor: {taint}.",
            model = self.status.model,
            taint = self.status.taint_floor,
        );
        self.last_assistant = Some(reply.clone());
        self.history.push(Entry::Assistant(reply));
        self.status.turn = self.status.turn.saturating_add(1);
    }

    /// Dispatch a slash command. The TUI ships every command that can
    /// answer from local state today (`/help`, `/quit`, `/clear`, `/info`,
    /// `/version`, `/copy`, `/history`, `/model`, `/receipt`, `/taint`,
    /// `/caps`, `/sandbox`); commands that need agent state announce what
    /// will land them.
    fn dispatch_slash(&mut self, cmd: &str) {
        let (head, rest) = cmd
            .split_once(char::is_whitespace)
            .map_or((cmd, ""), |(h, r)| (h.trim(), r.trim()));

        let body = match head {
            "help" | "?" => HELP_TEXT.into(),

            "quit" | "exit" => {
                self.history
                    .push(Entry::System("Bye. (Use Ctrl+C any time.)".into()));
                return;
            }

            "clear" | "new" => {
                self.history.clear();
                self.history
                    .push(Entry::System("Session cleared.".into()));
                return;
            }

            "version" => format!(
                "GaussClaw TUI v{} · gauss-aether runtime",
                env!("CARGO_PKG_VERSION")
            ),

            "info" | "status" => format!(
                "session  {session}\nmodel    {model}\nturn     {turn}\nchain    {chain}\
                 \ntaint    {taint}\ncaps     {caps}\nhistory  {hist} entries{path_line}",
                session = self.status.session,
                model = self.status.model,
                turn = self.status.turn,
                chain = self.status.chain_head,
                taint = self.status.taint_floor,
                caps = self.status.caps,
                hist = self.input_history.len(),
                path_line = self
                    .input_history
                    .path()
                    .map(|p| format!("\nhistory  → {}", p.display()))
                    .unwrap_or_default(),
            ),

            "history" => {
                if self.input_history.is_empty() {
                    "No prior input recorded. New entries persist under the platform state directory.".into()
                } else {
                    let path_note = self
                        .input_history
                        .path()
                        .map(|p| format!(" · stored at {}", p.display()))
                        .unwrap_or_default();
                    format!(
                        "{} entries on disk{}.\nUse Ctrl+P / Ctrl+N (or Up/Down on an empty line) to recall.",
                        self.input_history.len(),
                        path_note,
                    )
                }
            }

            "copy" => {
                if let Some(text) = self.last_assistant.clone() {
                    emit_osc52(&text);
                    format!(
                        "Copied last assistant reply to the system clipboard ({} chars via OSC 52).",
                        text.chars().count()
                    )
                } else {
                    "Nothing to copy yet — submit a turn first.".into()
                }
            }

            "model" => {
                if rest.is_empty() {
                    format!("Active model: {}", self.status.model)
                } else {
                    self.status.model = rest.to_owned();
                    format!("Active model set to {rest} (local only; persists once gaussclaw-config is wired).")
                }
            }

            "receipt" => format!(
                "Receipt chain head: {chain}\nVerify an export with: gaussclaw receipt verify <envelope.json>",
                chain = self.status.chain_head
            ),

            "taint" => format!(
                "Current taint floor: {floor}\nThe taint lattice ℒ is enforced by gauss-kernel::flow with antitone declassification.",
                floor = self.status.taint_floor
            ),

            "caps" => format!(
                "Granted capabilities: {n}\nRun `gaussclaw doctor --json` to inspect the active grant.",
                n = self.status.caps
            ),

            "sandbox" => "Composite sandbox layers (per tool dispatch):\n  L1 WASM (wasmi)\n  L2 Landlock\n  L3 seccomp\n  L4 bwrap\nUpper-bound compromise: Pr ≤ 1.1 × 10⁻⁷ (theorem T10).".into(),

            "tools" | "config" | "logs" | "queue" | "undo" | "retry" | "paste" | "compact"
            | "resume" | "sessions" | "details" | "statusbar" => {
                format!("/{head} {rest}: awaits agent-loop wiring; tracked in STRATEGY.md.")
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

/// Emit an OSC 52 clipboard write so the parent terminal puts `text` on
/// the user's system clipboard. Works in iTerm2, kitty, WezTerm, Alacritty,
/// recent xterm, and any tmux 3.3+ with `set-clipboard on`. No-ops on
/// terminals that ignore the sequence.
fn emit_osc52(text: &str) {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // ESC ] 52 ; c ; <base64> BEL — written straight to stdout. The TUI is
    // already in raw mode so the escape sequence reaches the terminal
    // emulator without being intercepted.
    let _ = std::io::Write::write_all(&mut io::stdout(), format!("\x1b]52;c;{b64}\x07").as_bytes());
    let _ = std::io::Write::flush(&mut io::stdout());
}

/// In-app help body for `/help`.
const HELP_TEXT: &str = "\
Slash commands
──────────────
  /help, /?          show this help
  /quit, /exit       exit (Ctrl+C / Ctrl+D also work)
  /clear, /new       wipe the visible history (Ctrl+L also)
  /version           show TUI + runtime versions
  /info, /status     dump session, model, chain head, taint, caps
  /history           show on-disk recall buffer location + size
  /model [name]      show or set the active model
  /copy              copy the last assistant reply to the system clipboard
  /receipt           show the active receipt-chain head
  /taint             show the active taint floor
  /caps              show the active capability count
  /sandbox           show composite-sandbox layer order

Keybindings
───────────
  Enter              submit current input
  Shift+Enter        insert a newline
  Ctrl+C, Ctrl+D     quit
  Ctrl+L             clear history
  Ctrl+P / Up        recall previous input
  Ctrl+N / Down      recall next input
  PageUp / PageDown  scroll transcript
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

    // Surface where we'll be persisting input history so operators know
    // exactly which file to grep or revoke.
    if let Some(path) = app.input_history.path().cloned() {
        app.push(Entry::System(format!(
            "Input history persists at {} ({} entries loaded).",
            path.display(),
            app.input_history.len(),
        )));
    }

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
