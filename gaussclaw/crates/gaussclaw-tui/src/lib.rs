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
    clippy::assigning_clones,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::nonminimal_bool
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
    /// The user pressed a cancel key while an agent loop was running.
    /// The runtime should ask the in-flight loop to wind down (typically
    /// by flipping a `gaussclaw_agent::CancelHandle`) and then resume the
    /// TUI input cycle without exiting.
    CancelInFlight,
}

/// Callback the TUI fires on user-initiated cancel.
///
/// Sprint 10 §7 — production runtimes pass a closure that calls
/// `gaussclaw_agent::CancelHandle::request_cancel`; tests pass a
/// closure that flips a local flag.
///
/// Boxed so it's trait-object-safe; cheap clones not needed because the
/// `App` owns it for its lifetime.
pub type CancelCallback = Box<dyn Fn() + Send + Sync>;

// ─── turn dispatch ────────────────────────────────────────────────────────────

/// Outcome of one dispatched user turn.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum TurnOutcome {
    /// The assistant reply text.
    Reply(String),
    /// The turn failed; the string is an operator-readable reason.
    Error(String),
}

/// Runs a user turn for the TUI, off the render thread.
///
/// The TUI is a synchronous render loop, but a real turn is an async
/// round-trip to a model provider. Rather than freeze the UI for the
/// duration, the App hands the prompt to a `TurnDispatcher`, which runs
/// the turn on its own thread/runtime and sends the result back over
/// `tx` **exactly once**. The render loop keeps spinning (and stays
/// cancellable) while the turn is in flight, polling the channel each
/// tick.
///
/// Production runtimes (`gaussclaw-bin`) implement this over a
/// `TurnPolicy` + reqwest-backed provider; tests implement it with an
/// in-thread sender that resolves immediately.
pub trait TurnDispatcher: Send + Sync {
    /// Begin `prompt`. The implementation must send exactly one
    /// [`TurnOutcome`] on `tx` when the turn settles. Dropping `tx`
    /// without sending is treated by the App as a dispatcher fault.
    fn dispatch(&self, prompt: String, tx: std::sync::mpsc::Sender<TurnOutcome>);
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
    /// Data-driven slash-command catalogue (defaults plus plugin
    /// registrations). The TUI consults this for `/commands` listing
    /// and for the "did you mean?" suggestion on unknown commands.
    ///
    /// The TUI's hand-written `dispatch_slash` `match` still owns the
    /// real behaviour for the built-in commands — the registry is
    /// authoritative for *discoverability*, not behaviour. This keeps
    /// the locked snapshot output stable while letting plugins
    /// surface their own command names through `/commands`.
    slash_registry: gaussclaw_cli::slash::SlashRegistry,
    /// True when the runtime has notified the App that an agent-loop
    /// turn is in flight. While `true`, `Ctrl+C` and `<Esc>` fire the
    /// cancel callback instead of quitting the TUI.
    turn_in_flight: bool,
    /// Optional cancel-callback the runtime wires to a
    /// `gaussclaw_agent::CancelHandle`. `None` keeps the legacy
    /// "Ctrl+C quits the TUI hard" behaviour for runtimes that don't
    /// drive an agent loop.
    on_cancel: Option<CancelCallback>,
    /// Optional turn dispatcher. When set, a non-slash submission runs
    /// a real provider turn instead of the local stub echo. `None`
    /// preserves the stub behaviour for runtimes (and tests) that don't
    /// wire an agent.
    dispatcher: Option<std::sync::Arc<dyn TurnDispatcher>>,
    /// Receiver for the in-flight turn started by [`Self::submit_current`].
    /// `Some` exactly while a dispatched turn is outstanding.
    in_flight_rx: Option<std::sync::mpsc::Receiver<TurnOutcome>>,
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
            slash_registry: gaussclaw_cli::slash::SlashRegistry::with_defaults(),
            turn_in_flight: false,
            on_cancel: None,
            dispatcher: None,
            in_flight_rx: None,
        }
    }

    /// Attach a [`TurnDispatcher`]. When set, a non-slash submission
    /// dispatches a real provider turn (off the render thread) instead
    /// of echoing the local stub. Returns `self` for builder chaining.
    #[must_use]
    pub fn with_dispatcher(mut self, dispatcher: std::sync::Arc<dyn TurnDispatcher>) -> Self {
        self.dispatcher = Some(dispatcher);
        self
    }

    /// Borrow the slash-command registry. Plugins / channel adapters
    /// use this to register additional commands at startup. The TUI
    /// surfaces every entry in `/commands` output and uses the
    /// catalogue for the "did you mean?" suggestion on unknown input.
    pub fn slash_registry(&self) -> &gaussclaw_cli::slash::SlashRegistry {
        &self.slash_registry
    }

    /// Mutable handle to the registry — for plugin registration.
    pub fn slash_registry_mut(&mut self) -> &mut gaussclaw_cli::slash::SlashRegistry {
        &mut self.slash_registry
    }

    /// Attach a cancel callback. Sprint 10 §7 — production runtimes
    /// pass a closure that calls
    /// `gaussclaw_agent::CancelHandle::request_cancel`; when set, the
    /// TUI's `Ctrl+C` / `<Esc>` keys fire the callback (asking the
    /// in-flight loop to wind down) instead of hard-quitting the TUI
    /// whenever `mark_turn_in_flight(true)` has been called.
    #[must_use]
    pub fn with_cancel_callback(mut self, cb: CancelCallback) -> Self {
        self.on_cancel = Some(cb);
        self
    }

    /// Tell the App whether an agent-loop turn is currently in flight.
    /// The runtime calls `mark_turn_in_flight(true)` when it dispatches
    /// `AgentLoop::run` and `mark_turn_in_flight(false)` when the loop
    /// returns (whether normally or via cancel). Cancel keys only fire
    /// the callback while `turn_in_flight == true`.
    pub const fn mark_turn_in_flight(&mut self, in_flight: bool) {
        self.turn_in_flight = in_flight;
    }

    /// True when the runtime has marked a turn as in-flight.
    #[must_use]
    pub const fn is_turn_in_flight(&self) -> bool {
        self.turn_in_flight
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

        // Sprint 10 §7 — `<Esc>` is the dedicated "cancel in-flight"
        // key. It does nothing when no turn is in flight, mirroring
        // every modern terminal agent (Cursor, aider, claude-code).
        if key.code == KeyCode::Esc {
            if self.turn_in_flight {
                if let Some(cb) = self.on_cancel.as_ref() {
                    cb();
                }
                self.history.push(Entry::System(
                    "Cancel requested. The loop will wind down at the next boundary.".into(),
                ));
                return Tick::CancelInFlight;
            }
            return Tick::Continue;
        }

        // Global keybindings first.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c' | 'd') => {
                    // Sprint 10 §7 — `Ctrl+C` during an in-flight turn
                    // asks the loop to cancel rather than hard-quitting
                    // the TUI. A second `Ctrl+C` after the loop has
                    // returned (or when no turn is in flight) quits.
                    if self.turn_in_flight {
                        if let Some(cb) = self.on_cancel.as_ref() {
                            cb();
                            self.history.push(Entry::System(
                                "Cancel requested. Press Ctrl+C again after the loop returns to quit.".into(),
                            ));
                            return Tick::CancelInFlight;
                        }
                    }
                    return Tick::Quit;
                }
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

        // Don't start a second turn while one is outstanding.
        if self.turn_in_flight {
            self.history.push(Entry::System(
                "A turn is already running — press Esc to cancel it first.".into(),
            ));
            return;
        }

        self.history.push(Entry::User(body.clone()));

        if let Some(dispatcher) = self.dispatcher.clone() {
            // Real dispatch: hand the prompt to the dispatcher, which
            // runs it off the render thread and reports back over the
            // channel. The render loop polls `in_flight_rx` each tick.
            let (tx, rx) = std::sync::mpsc::channel();
            self.in_flight_rx = Some(rx);
            self.turn_in_flight = true;
            dispatcher.dispatch(body, tx);
        } else {
            // No agent wired (e.g. snapshot tests, surfaces that only
            // inspect local state): keep the legacy stub echo so the
            // round-trip still exercises history + the turn counter.
            let reply = format!(
                "(stub) No provider attached to this surface. Run `gaussclaw serve` or wire a \
                 TurnDispatcher. Current model: {model}, taint floor: {taint}.",
                model = self.status.model,
                taint = self.status.taint_floor,
            );
            self.last_assistant = Some(reply.clone());
            self.history.push(Entry::Assistant(reply));
            self.status.turn = self.status.turn.saturating_add(1);
        }
    }

    /// Poll the in-flight turn, if any, for a settled outcome.
    ///
    /// Non-blocking: the render loop calls this each tick. Returns
    /// `true` when an outcome was consumed (so the loop knows to
    /// re-render), `false` otherwise. On a settled turn the assistant
    /// reply (or an error breadcrumb) is appended to history, the turn
    /// counter advances, and the in-flight state clears.
    pub fn poll_turn(&mut self) -> bool {
        let Some(rx) = self.in_flight_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(TurnOutcome::Reply(text)) => {
                self.last_assistant = Some(text.clone());
                self.history.push(Entry::Assistant(text));
                self.status.turn = self.status.turn.saturating_add(1);
                self.finish_turn();
                true
            }
            Ok(TurnOutcome::Error(msg)) => {
                self.history
                    .push(Entry::System(format!("turn failed: {msg}")));
                self.finish_turn();
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // The dispatcher dropped the sender without ever
                // producing an outcome — surface it rather than hang
                // in-flight forever.
                self.history.push(Entry::System(
                    "turn failed: dispatcher closed without a response.".into(),
                ));
                self.finish_turn();
                true
            }
        }
    }

    /// Clear in-flight bookkeeping once a turn settles.
    fn finish_turn(&mut self) {
        self.in_flight_rx = None;
        self.turn_in_flight = false;
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

            "commands" => self.slash_registry.help_text(),

            "tools" | "config" | "logs" | "queue" | "undo" | "retry" | "paste" | "compact"
            | "resume" | "sessions" | "details" | "statusbar" => {
                format!("/{head} {rest}: awaits agent-loop wiring; tracked in STRATEGY.md.")
            }

            other => {
                // Consult the slash registry for a "did you mean?"
                // suggestion. Plugin-registered commands surface here
                // even though they aren't in the hand-written match.
                if let Some(cmd) = self.slash_registry.resolve(other) {
                    format!(
                        "/{other}: {desc} (plugin-registered; pending TUI wiring).",
                        desc = cmd.description,
                    )
                } else if let Some(suggestion) = self.closest_slash(other) {
                    format!(
                        "Unknown command: /{other}. Did you mean /{suggestion}? Try /help or /commands.",
                    )
                } else {
                    format!("Unknown command: /{other}. Try /help.")
                }
            }
        };
        self.history.push(Entry::System(body));
    }

    /// Cheap edit-distance "did you mean?" against the slash registry.
    /// Returns the closest canonical name within Levenshtein distance ≤ 2.
    fn closest_slash(&self, head: &str) -> Option<&'static str> {
        let mut best: Option<(&'static str, usize)> = None;
        for cmd in self.slash_registry.iter() {
            let d = levenshtein(head, cmd.name);
            if d <= 2 && best.map_or(true, |(_, bd)| d < bd) {
                best = Some((cmd.name, d));
            }
        }
        best.map(|(n, _)| n)
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
        // While a turn is in flight, surface a working indicator in the
        // border title (and that Esc cancels). Idle renders unchanged,
        // so locked snapshots that never dispatch are unaffected.
        let title = if self.turn_in_flight {
            " input · ⋯ working (Esc to cancel) "
        } else {
            " input "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(title);
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

/// Iterative Levenshtein edit distance. Used by the TUI's "did you
/// mean?" suggestion against the slash registry. We early-out at
/// distance > 3 since the caller only cares about ≤ 2.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
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
  Ctrl+C, Ctrl+D     cancel in-flight turn (or quit when idle)
  Esc                cancel in-flight turn (no-op when idle)
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
    run_with_dispatcher(initial, None)
}

/// Run the TUI with an optional [`TurnDispatcher`].
///
/// `gaussclaw-bin` passes a dispatcher backed by a `TurnPolicy` +
/// reqwest provider so the terminal holds a real conversation; passing
/// `None` is equivalent to [`run`] and keeps the local stub echo.
pub fn run_with_dispatcher(
    initial: StatusInfo,
    dispatcher: Option<std::sync::Arc<dyn TurnDispatcher>>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(initial);
    if let Some(d) = dispatcher {
        app = app.with_dispatcher(d);
    }

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
        // The 250 ms poll timeout doubles as the in-flight turn tick:
        // when `poll` returns false (no key), we still fall through and
        // check whether a dispatched turn has settled, so a reply lands
        // within a quarter-second of the provider returning even if the
        // user isn't touching the keyboard.
        if crossterm::event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = crossterm::event::read()? {
                match app.on_key(key) {
                    Tick::Quit => {
                        terminal.draw(|f| app.render(f))?;
                        return Ok(());
                    }
                    // The cancel callback already fired in `on_key`; the
                    // event loop just keeps spinning so the user sees
                    // the system-message breadcrumb appear and can
                    // continue interacting once the loop returns.
                    Tick::CancelInFlight | Tick::Continue => {}
                }
            }
        }
        // Drain any settled in-flight turn (no-op when none outstanding).
        app.poll_turn();
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

    /// `/commands` lists the entire slash-registry — the catalogue
    /// surfaces every registered command (defaults + plugin entries).
    #[test]
    fn slash_commands_lists_registry_entries() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "/commands");
        app.on_key(enter());
        let last = app.history().last().expect("entry");
        match last {
            Entry::System(body) => {
                assert!(body.contains("Available slash commands"));
                assert!(body.contains("/help"));
                assert!(body.contains("/compact"));
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    /// A near-miss surfaces the "did you mean?" hint sourced from the
    /// registry's catalogue.
    #[test]
    fn near_miss_offers_did_you_mean() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "/hep"); // 1 edit from /help
        app.on_key(enter());
        let last = app.history().last().expect("entry");
        match last {
            Entry::System(body) => {
                assert!(body.contains("Did you mean /help"));
            }
            other => panic!("expected System, got {other:?}"),
        }
    }

    /// Plugin-registered command (added via `slash_registry_mut`) is
    /// resolved by the TUI fallback path even though the hand-written
    /// `match` doesn't list it. The body announces "plugin-registered".
    #[test]
    fn plugin_registered_command_resolves_via_registry() {
        use gaussclaw_cli::slash::{SlashCommand, SlashKind};

        let mut app = App::new(StatusInfo::default());
        app.slash_registry_mut()
            .register(SlashCommand::new(
                "deploy",
                &[],
                "ship the build",
                SlashKind::Agent,
                true,
            ))
            .unwrap();
        type_str(&mut app, "/deploy");
        app.on_key(enter());
        let last = app.history().last().expect("entry");
        match last {
            Entry::System(body) => {
                assert!(body.contains("/deploy"));
                assert!(body.contains("plugin-registered"));
                assert!(body.contains("ship the build"));
            }
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

    // ── Sprint 10 §7: Ctrl+C / Esc cancel-in-flight wiring ────────────────

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    /// Returns `(app, flag)` where flipping `flag` proves the cancel
    /// callback fired. The callback only flips `flag` — production
    /// wires it to `CancelHandle::request_cancel`.
    fn app_with_cancel_flag() -> (App<'static>, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = flag.clone();
        let app = App::new(StatusInfo::default()).with_cancel_callback(Box::new(move || {
            f.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        (app, flag)
    }

    #[test]
    fn ctrl_c_with_no_callback_still_quits_when_no_turn_in_flight() {
        // Legacy behaviour preserved: a runtime that doesn't drive the
        // agent loop (e.g. a pure shell demo) keeps the hard-quit.
        let mut app = App::new(StatusInfo::default());
        assert_eq!(app.on_key(ctrl('c')), Tick::Quit);
    }

    #[test]
    fn ctrl_c_during_turn_in_flight_fires_callback_and_returns_cancel() {
        let (mut app, fired) = app_with_cancel_flag();
        app.mark_turn_in_flight(true);
        assert_eq!(app.on_key(ctrl('c')), Tick::CancelInFlight);
        assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
        // History gains a system breadcrumb so the user sees why
        // Ctrl+C didn't quit.
        assert!(app
            .history()
            .iter()
            .any(|e| matches!(e, Entry::System(s) if s.contains("Cancel requested"))));
    }

    #[test]
    fn ctrl_c_after_in_flight_clears_still_quits() {
        // After the runtime calls `mark_turn_in_flight(false)`, the
        // next Ctrl+C reverts to quitting — the "two presses to exit"
        // UX the help footer documents.
        let (mut app, _fired) = app_with_cancel_flag();
        app.mark_turn_in_flight(true);
        app.on_key(ctrl('c'));
        app.mark_turn_in_flight(false);
        assert_eq!(app.on_key(ctrl('c')), Tick::Quit);
    }

    #[test]
    fn esc_during_turn_in_flight_fires_cancel_callback() {
        let (mut app, fired) = app_with_cancel_flag();
        app.mark_turn_in_flight(true);
        assert_eq!(app.on_key(esc()), Tick::CancelInFlight);
        assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn esc_when_no_turn_in_flight_is_a_noop() {
        // `<Esc>` is a dedicated cancel key — when idle it must NOT
        // quit, NOT clear, and NOT fire the callback. Mirrors aider /
        // claude-code / Cursor's behaviour.
        let (mut app, fired) = app_with_cancel_flag();
        assert_eq!(app.on_key(esc()), Tick::Continue);
        assert!(!fired.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn turn_in_flight_flag_round_trips() {
        let mut app = App::new(StatusInfo::default());
        assert!(!app.is_turn_in_flight());
        app.mark_turn_in_flight(true);
        assert!(app.is_turn_in_flight());
        app.mark_turn_in_flight(false);
        assert!(!app.is_turn_in_flight());
    }

    // ── turn dispatch wiring ──────────────────────────────────────────────

    /// A dispatcher that replies immediately on the calling thread, so
    /// the next `poll_turn` observes the outcome deterministically.
    struct ImmediateDispatcher(TurnOutcome);
    impl TurnDispatcher for ImmediateDispatcher {
        fn dispatch(&self, prompt: String, tx: std::sync::mpsc::Sender<TurnOutcome>) {
            // Echo the prompt into a reply so we can assert the dispatcher
            // saw the right input.
            let outcome = match &self.0 {
                TurnOutcome::Reply(_) => TurnOutcome::Reply(format!("got: {prompt}")),
                TurnOutcome::Error(e) => TurnOutcome::Error(e.clone()),
            };
            let _ = tx.send(outcome);
        }
    }

    fn app_with_dispatcher(outcome: TurnOutcome) -> App<'static> {
        App::with_history(StatusInfo::default(), HistoryStore::in_memory())
            .with_dispatcher(std::sync::Arc::new(ImmediateDispatcher(outcome)))
    }

    #[test]
    fn dispatcher_path_marks_in_flight_and_defers_reply() {
        let mut app = app_with_dispatcher(TurnOutcome::Reply(String::new()));
        type_str(&mut app, "ping");
        app.on_key(enter());
        // The user line is in history and a turn is in flight — but no
        // assistant entry yet (it arrives via poll_turn).
        assert!(app.is_turn_in_flight());
        assert!(matches!(app.history().last(), Some(Entry::User(b)) if b == "ping"));
    }

    #[test]
    fn poll_turn_appends_reply_and_clears_in_flight() {
        let mut app = app_with_dispatcher(TurnOutcome::Reply(String::new()));
        type_str(&mut app, "ping");
        app.on_key(enter());
        assert!(app.poll_turn());
        assert!(!app.is_turn_in_flight());
        assert!(matches!(app.history().last(), Some(Entry::Assistant(b)) if b == "got: ping"));
        // A second poll with nothing outstanding is a no-op.
        assert!(!app.poll_turn());
    }

    #[test]
    fn poll_turn_surfaces_dispatch_errors_as_system_entry() {
        let mut app = app_with_dispatcher(TurnOutcome::Error("upstream 401".into()));
        type_str(&mut app, "ping");
        app.on_key(enter());
        assert!(app.poll_turn());
        assert!(!app.is_turn_in_flight());
        assert!(
            matches!(app.history().last(), Some(Entry::System(b)) if b.contains("upstream 401"))
        );
    }

    #[test]
    fn dropped_sender_without_outcome_is_reported() {
        struct DropDispatcher;
        impl TurnDispatcher for DropDispatcher {
            fn dispatch(&self, _prompt: String, _tx: std::sync::mpsc::Sender<TurnOutcome>) {
                // Drop tx without sending.
            }
        }
        let mut app = App::with_history(StatusInfo::default(), HistoryStore::in_memory())
            .with_dispatcher(std::sync::Arc::new(DropDispatcher));
        type_str(&mut app, "ping");
        app.on_key(enter());
        assert!(app.poll_turn());
        assert!(!app.is_turn_in_flight());
        assert!(
            matches!(app.history().last(), Some(Entry::System(b)) if b.contains("closed without"))
        );
    }

    #[test]
    fn second_submit_while_in_flight_is_rejected() {
        let mut app = app_with_dispatcher(TurnOutcome::Reply(String::new()));
        type_str(&mut app, "first");
        app.on_key(enter());
        assert!(app.is_turn_in_flight());
        type_str(&mut app, "second");
        app.on_key(enter());
        // The second submission is refused with a system breadcrumb; the
        // first turn is still the one in flight.
        assert!(app.is_turn_in_flight());
        assert!(
            matches!(app.history().last(), Some(Entry::System(b)) if b.contains("already running"))
        );
    }

    #[test]
    fn no_dispatcher_keeps_stub_echo() {
        let mut app = App::with_history(StatusInfo::default(), HistoryStore::in_memory());
        type_str(&mut app, "ping");
        app.on_key(enter());
        // No dispatcher → immediate stub assistant reply, no in-flight state.
        assert!(!app.is_turn_in_flight());
        assert!(matches!(app.history().last(), Some(Entry::Assistant(b)) if b.contains("(stub)")));
    }
}
