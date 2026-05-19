//! Modal overlay system for the GaussClaw TUI.
//!
//! Hermes's Ink TUI ships modal overlays for approval prompts, clarify
//! questions, and password / API-key entry; we mirror the surface in
//! Ratatui. An overlay owns the keyboard while open: every key is
//! intercepted by [`Overlay::on_key`] rather than the underlying
//! conversation pane.
//!
//! Three overlay types ship today:
//!
//! 1. [`Overlay::approval`] — a yes/no/details prompt for SAG approvals.
//! 2. [`Overlay::clarify`]  — a numbered quick-pick over up to nine
//!    options (Hermes parity: `1`..=`9` quick-select).
//! 3. [`Overlay::password`] — masked single-line entry for a secret
//!    (API keys, OAuth tokens, signing keys).
//!
//! Each overlay returns an [`OverlayResult`] when it dismisses; the
//! caller threads that result back to the agent loop or the surface
//! plane.

use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

/// Outcome of an overlay-handled key press.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OverlayResult {
    /// Overlay stays open; nothing else for the host loop to do.
    Continue,
    /// Operator approved (approval overlay).
    Approved,
    /// Operator refused (approval / clarify overlay).
    Refused,
    /// Operator requested more detail / `details` key.
    Details,
    /// Operator picked a clarify option (0-based index).
    Picked(usize),
    /// Operator submitted a password / API key.
    Password(String),
    /// Operator dismissed without committing (Esc / Ctrl+C).
    Cancelled,
}

/// Kind of [`Overlay::Picker`] — drives the header label + which
/// quickkeys are active. Sprint 5 §9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PickerKind {
    /// Pick an LLM model (provider + model name).
    Model,
    /// Pick a persisted session to resume.
    Session,
    /// Pick a sub-agent / delegate to dispatch to.
    Agents,
    /// Browse / preview a Skill Manifest without installing.
    Skills,
}

impl PickerKind {
    /// Display-friendly tag.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Session => "session",
            Self::Agents => "agents",
            Self::Skills => "skills",
        }
    }
}

/// One row in [`Overlay::Picker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerRow {
    /// Primary label rendered first (e.g. `anthropic/claude-3.5-sonnet`).
    pub primary: String,
    /// Secondary detail rendered dimmer below the primary (e.g.
    /// `200k ctx · $3/Mtok · streaming`).
    pub secondary: String,
}

impl PickerRow {
    /// Build a row from two strings.
    #[must_use]
    pub fn new(primary: impl Into<String>, secondary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            secondary: secondary.into(),
        }
    }
}

/// Lifecycle state of one [`TodoItem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TodoStatus {
    /// Not yet started.
    Pending,
    /// In progress.
    InProgress,
    /// Completed.
    Done,
}

impl TodoStatus {
    /// Cycle to the next state (Pending → InProgress → Done → Pending).
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Pending => Self::InProgress,
            Self::InProgress => Self::Done,
            Self::Done => Self::Pending,
        }
    }

    /// Glyph for the rendered row.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::InProgress => "◐",
            Self::Done => "●",
        }
    }
}

/// One row in [`Overlay::Todo`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    /// Item text.
    pub text: String,
    /// Lifecycle status.
    pub status: TodoStatus,
}

impl TodoItem {
    /// Build a pending todo item.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status: TodoStatus::Pending,
        }
    }
}

/// One overlay instance — typed by variant rather than by trait so a
/// single field in the `App` covers every overlay shape.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Overlay {
    /// Approval prompt — yes / no / details.
    Approval {
        /// Prompt heading (e.g. "Approve shell command?").
        title: Cow<'static, str>,
        /// Pre-formatted body lines (capability set, taint, args).
        body: Vec<String>,
    },
    /// Clarify quick-pick — up to nine options with 1-based shortcuts.
    Clarify {
        /// Question text.
        title: Cow<'static, str>,
        /// Option strings; max nine surfaced via quick-pick keys.
        options: Vec<String>,
        /// Currently focused option (for ↑ / ↓ navigation).
        cursor: usize,
    },
    /// Password / secret entry — masked input.
    Password {
        /// Prompt heading.
        title: Cow<'static, str>,
        /// Help text under the prompt.
        hint: Cow<'static, str>,
        /// Accumulated input buffer (rendered as bullets).
        buf: String,
    },
    /// Generic picker — model / session / agents / skills (Sprint 5 §9).
    /// Each row carries a primary + secondary label so the host can
    /// surface model id + cost line, session id + turn count, etc.
    Picker {
        /// Picker kind (drives the header label).
        kind: PickerKind,
        /// Prompt heading (`"Pick a model"`, `"Resume a session"`, …).
        title: Cow<'static, str>,
        /// Rows; unbounded but the renderer clips to the visible area.
        rows: Vec<PickerRow>,
        /// Currently focused row (0-based).
        cursor: usize,
        /// Topmost visible row index — adjusted on cursor move so the
        /// focus row always stays on screen.
        viewport_top: usize,
    },
    /// Todo panel — list of items with cycle-status keystrokes (Sprint 5 §9).
    Todo {
        /// Prompt heading.
        title: Cow<'static, str>,
        /// Items in render order.
        items: Vec<TodoItem>,
        /// Focused row.
        cursor: usize,
    },
}

impl Overlay {
    /// Build an approval overlay.
    #[must_use]
    pub fn approval(title: impl Into<Cow<'static, str>>, body: Vec<String>) -> Self {
        Self::Approval {
            title: title.into(),
            body,
        }
    }

    /// Build a clarify overlay with up to nine numbered options.
    #[must_use]
    pub fn clarify(title: impl Into<Cow<'static, str>>, options: Vec<String>) -> Self {
        Self::Clarify {
            title: title.into(),
            options,
            cursor: 0,
        }
    }

    /// Build a masked password / API-key entry overlay.
    #[must_use]
    pub fn password(
        title: impl Into<Cow<'static, str>>,
        hint: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self::Password {
            title: title.into(),
            hint: hint.into(),
            buf: String::new(),
        }
    }

    /// Build a model picker.
    #[must_use]
    pub fn model_picker(rows: Vec<PickerRow>) -> Self {
        Self::picker(PickerKind::Model, "Pick a model", rows)
    }

    /// Build a session picker.
    #[must_use]
    pub fn session_picker(rows: Vec<PickerRow>) -> Self {
        Self::picker(PickerKind::Session, "Resume a session", rows)
    }

    /// Build an agents / delegate picker.
    #[must_use]
    pub fn agents_picker(rows: Vec<PickerRow>) -> Self {
        Self::picker(PickerKind::Agents, "Pick a sub-agent", rows)
    }

    /// Build a skills-hub browser.
    #[must_use]
    pub fn skills_hub(rows: Vec<PickerRow>) -> Self {
        Self::picker(PickerKind::Skills, "Skills", rows)
    }

    /// Generic picker constructor (used by the per-kind helpers).
    #[must_use]
    pub fn picker(
        kind: PickerKind,
        title: impl Into<Cow<'static, str>>,
        rows: Vec<PickerRow>,
    ) -> Self {
        Self::Picker {
            kind,
            title: title.into(),
            rows,
            cursor: 0,
            viewport_top: 0,
        }
    }

    /// Build a todo panel.
    #[must_use]
    pub fn todo(title: impl Into<Cow<'static, str>>, items: Vec<TodoItem>) -> Self {
        Self::Todo {
            title: title.into(),
            items,
            cursor: 0,
        }
    }

    /// Handle one key event. Returns [`OverlayResult::Continue`] when
    /// the overlay should stay open.
    pub fn on_key(&mut self, key: KeyEvent) -> OverlayResult {
        if key.kind != KeyEventKind::Press {
            return OverlayResult::Continue;
        }

        // Universal escape hatch.
        if key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            return OverlayResult::Cancelled;
        }

        match self {
            Self::Approval { .. } => match key.code {
                KeyCode::Char('o') | KeyCode::Char('y') | KeyCode::Enter => OverlayResult::Approved,
                KeyCode::Char('s') | KeyCode::Char('n') => OverlayResult::Refused,
                KeyCode::Char('d') | KeyCode::Char('?') => OverlayResult::Details,
                _ => OverlayResult::Continue,
            },
            Self::Clarify {
                options, cursor, ..
            } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if *cursor > 0 {
                        *cursor -= 1;
                    }
                    OverlayResult::Continue
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if *cursor + 1 < options.len() {
                        *cursor += 1;
                    }
                    OverlayResult::Continue
                }
                KeyCode::Enter => {
                    if options.is_empty() {
                        OverlayResult::Cancelled
                    } else {
                        OverlayResult::Picked(*cursor)
                    }
                }
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    let idx = (c as u8).saturating_sub(b'1') as usize;
                    if idx < options.len() {
                        OverlayResult::Picked(idx)
                    } else {
                        OverlayResult::Continue
                    }
                }
                _ => OverlayResult::Continue,
            },
            Self::Password { buf, .. } => match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buf.push(c);
                    OverlayResult::Continue
                }
                KeyCode::Backspace => {
                    buf.pop();
                    OverlayResult::Continue
                }
                KeyCode::Enter => OverlayResult::Password(std::mem::take(buf)),
                _ => OverlayResult::Continue,
            },
            Self::Picker {
                rows,
                cursor,
                viewport_top,
                ..
            } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if *cursor > 0 {
                        *cursor -= 1;
                        if *cursor < *viewport_top {
                            *viewport_top = *cursor;
                        }
                    }
                    OverlayResult::Continue
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if *cursor + 1 < rows.len() {
                        *cursor += 1;
                        // Viewport scroll handled at render time
                        // against the actual visible height.
                    }
                    OverlayResult::Continue
                }
                KeyCode::PageUp => {
                    *cursor = cursor.saturating_sub(10);
                    if *cursor < *viewport_top {
                        *viewport_top = *cursor;
                    }
                    OverlayResult::Continue
                }
                KeyCode::PageDown => {
                    *cursor = (*cursor + 10).min(rows.len().saturating_sub(1));
                    OverlayResult::Continue
                }
                KeyCode::Home => {
                    *cursor = 0;
                    *viewport_top = 0;
                    OverlayResult::Continue
                }
                KeyCode::End => {
                    *cursor = rows.len().saturating_sub(1);
                    OverlayResult::Continue
                }
                KeyCode::Enter => {
                    if rows.is_empty() {
                        OverlayResult::Cancelled
                    } else {
                        OverlayResult::Picked(*cursor)
                    }
                }
                _ => OverlayResult::Continue,
            },
            Self::Todo { items, cursor, .. } => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if *cursor > 0 {
                        *cursor -= 1;
                    }
                    OverlayResult::Continue
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if *cursor + 1 < items.len() {
                        *cursor += 1;
                    }
                    OverlayResult::Continue
                }
                KeyCode::Char(' ') | KeyCode::Char('x') => {
                    // Toggle status of the focused item; the host
                    // mirrors the change into its own state on the
                    // next on_key tick. Empty list is a no-op.
                    if let Some(item) = items.get_mut(*cursor) {
                        item.status = item.status.next();
                    }
                    OverlayResult::Continue
                }
                KeyCode::Enter => {
                    if items.is_empty() {
                        OverlayResult::Cancelled
                    } else {
                        OverlayResult::Picked(*cursor)
                    }
                }
                _ => OverlayResult::Continue,
            },
        }
    }

    /// Render the overlay centred over `area`. Uses [`Clear`] first so
    /// the underlying pane is hidden behind a solid panel.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let (panel_w, panel_h) = self.panel_dimensions(area);
        let panel = centre(area, panel_w, panel_h);
        frame.render_widget(Clear, panel);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        match self {
            Self::Approval { title, body } => {
                let inner = block
                    .clone()
                    .title(format!(" {title} "))
                    .border_style(Style::default().fg(Color::Yellow));
                let inner_area = inner.inner(panel);
                frame.render_widget(inner, panel);

                let mut lines: Vec<Line<'_>> = body.iter().map(|l| Line::raw(l.clone())).collect();
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        "[o]",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" approve  "),
                    Span::styled(
                        "[s]",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" refuse  "),
                    Span::styled(
                        "[d]",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" details  "),
                    Span::styled("[Esc]", Style::default().fg(Color::DarkGray)),
                    Span::raw(" cancel"),
                ]));
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner_area);
            }
            Self::Clarify {
                title,
                options,
                cursor,
            } => {
                let inner = block.clone().title(format!(" {title} "));
                let inner_area = inner.inner(panel);
                frame.render_widget(inner, panel);

                let mut lines: Vec<Line<'_>> = options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        let key = format!("{}", i.saturating_add(1));
                        let prefix = if i == *cursor { "› " } else { "  " };
                        let style = if i == *cursor {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        Line::from(vec![
                            Span::raw(prefix),
                            Span::styled(format!("[{key}]"), style),
                            Span::raw(" "),
                            Span::styled(o.clone(), style),
                        ])
                    })
                    .collect();
                lines.push(Line::raw(""));
                lines.push(Line::raw(
                    "↑/↓ move · 1-9 quick-pick · Enter confirm · Esc cancel",
                ));
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner_area);
            }
            Self::Password { title, hint, buf } => {
                let inner = block
                    .clone()
                    .title(format!(" {title} "))
                    .border_style(Style::default().fg(Color::Red));
                let inner_area = inner.inner(panel);
                frame.render_widget(inner, panel);

                let mut lines: Vec<Line<'_>> = Vec::new();
                lines.push(Line::raw(hint.as_ref()));
                lines.push(Line::raw(""));
                let mask: String = std::iter::repeat('•').take(buf.chars().count()).collect();
                lines.push(Line::from(vec![
                    Span::raw("› "),
                    Span::styled(mask, Style::default().fg(Color::Yellow)),
                    Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
                ]));
                lines.push(Line::raw(""));
                lines.push(Line::raw("Enter submit · Backspace delete · Esc cancel"));
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner_area);
            }
            Self::Picker {
                kind,
                title,
                rows,
                cursor,
                viewport_top,
            } => {
                let inner = block
                    .clone()
                    .title(format!(" {title} ({}) ", kind.as_str()));
                let inner_area = inner.inner(panel);
                frame.render_widget(inner, panel);

                // Each row occupies two lines (primary + secondary).
                // Reserve the last line for the help footer.
                let row_lines: usize = 2;
                let footer_lines: usize = 2;
                let visible_rows = inner_area
                    .height
                    .saturating_sub(footer_lines as u16)
                    .saturating_div(row_lines as u16) as usize;
                let visible_rows = visible_rows.max(1);
                // Adjust viewport_top to keep cursor visible. (Mut
                // via interior shadow — render is &self, so we only
                // adjust an on-stack copy.)
                let mut top = *viewport_top;
                if *cursor < top {
                    top = *cursor;
                } else if *cursor >= top + visible_rows {
                    top = cursor.saturating_sub(visible_rows.saturating_sub(1));
                }
                let end = (top + visible_rows).min(rows.len());

                let mut lines: Vec<Line<'_>> = Vec::new();
                if rows.is_empty() {
                    lines.push(Line::raw("(no entries)"));
                } else {
                    for (i, row) in rows[top..end].iter().enumerate() {
                        let actual_idx = top + i;
                        let focused = actual_idx == *cursor;
                        let prefix = if focused { "› " } else { "  " };
                        let primary_style = if focused {
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        lines.push(Line::from(vec![
                            Span::raw(prefix),
                            Span::styled(row.primary.clone(), primary_style),
                        ]));
                        if row.secondary.is_empty() {
                            lines.push(Line::raw(""));
                        } else {
                            lines.push(Line::from(vec![
                                Span::raw("    "),
                                Span::styled(
                                    row.secondary.clone(),
                                    Style::default().fg(Color::DarkGray),
                                ),
                            ]));
                        }
                    }
                }
                lines.push(Line::raw(""));
                let count_line = format!(
                    "{} of {} · ↑/↓ navigate · Enter select · Esc cancel",
                    cursor.saturating_add(1).min(rows.len()),
                    rows.len()
                );
                lines.push(Line::styled(
                    count_line,
                    Style::default().fg(Color::DarkGray),
                ));
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner_area);
            }
            Self::Todo {
                title,
                items,
                cursor,
            } => {
                let inner = block.clone().title(format!(" {title} (todo) "));
                let inner_area = inner.inner(panel);
                frame.render_widget(inner, panel);

                let mut lines: Vec<Line<'_>> = Vec::new();
                if items.is_empty() {
                    lines.push(Line::raw("(no todo items)"));
                } else {
                    for (i, item) in items.iter().enumerate() {
                        let focused = i == *cursor;
                        let prefix = if focused { "› " } else { "  " };
                        let glyph = item.status.glyph();
                        let style = match item.status {
                            TodoStatus::Done => Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::CROSSED_OUT),
                            TodoStatus::InProgress => Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                            TodoStatus::Pending => Style::default(),
                        };
                        let focus_style = if focused {
                            style.add_modifier(Modifier::REVERSED)
                        } else {
                            style
                        };
                        lines.push(Line::from(vec![
                            Span::raw(prefix),
                            Span::styled(format!("{glyph} "), focus_style),
                            Span::styled(item.text.clone(), focus_style),
                        ]));
                    }
                }
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    "↑/↓ move · Space/x cycle status · Enter pick · Esc close",
                    Style::default().fg(Color::DarkGray),
                ));
                frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner_area);
            }
        }
    }

    /// Pick a panel size based on overlay variant + outer area.
    fn panel_dimensions(&self, area: Rect) -> (u16, u16) {
        match self {
            Self::Picker { rows, .. } => {
                let w = area.width.saturating_mul(7).saturating_div(10).max(60);
                let body_h = (rows.len() as u16).saturating_mul(2).clamp(6, 24);
                let h = body_h.saturating_add(4); // borders + footer
                (w.min(area.width), h.min(area.height))
            }
            Self::Todo { items, .. } => {
                let w = area.width.saturating_mul(6).saturating_div(10).max(50);
                let body_h = (items.len() as u16).clamp(4, 18);
                let h = body_h.saturating_add(4);
                (w.min(area.width), h.min(area.height))
            }
            _ => (60, 14),
        }
    }
}

/// Compute a centred sub-rectangle of `width` × `height` cells.
fn centre(area: Rect, width: u16, height: u16) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .split(area);
    let columns = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(width),
        Constraint::Min(0),
    ])
    .split(popup_layout[1]);
    columns[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn keycode(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn approval_quickkeys() {
        let mut a = Overlay::approval("Approve?", vec!["body".into()]);
        assert_eq!(a.on_key(key('o')), OverlayResult::Approved);
        assert_eq!(a.on_key(key('s')), OverlayResult::Refused);
        assert_eq!(a.on_key(key('d')), OverlayResult::Details);
        assert_eq!(a.on_key(keycode(KeyCode::Enter)), OverlayResult::Approved);
    }

    #[test]
    fn approval_escape_cancels() {
        let mut a = Overlay::approval("?", vec![]);
        assert_eq!(a.on_key(keycode(KeyCode::Esc)), OverlayResult::Cancelled);
    }

    #[test]
    fn clarify_quickpick_returns_index() {
        let mut c = Overlay::clarify("pick", vec!["one".into(), "two".into(), "three".into()]);
        assert_eq!(c.on_key(key('2')), OverlayResult::Picked(1));
    }

    #[test]
    fn clarify_arrow_keys_then_enter() {
        let mut c = Overlay::clarify("pick", vec!["one".into(), "two".into(), "three".into()]);
        assert_eq!(c.on_key(keycode(KeyCode::Down)), OverlayResult::Continue);
        assert_eq!(c.on_key(keycode(KeyCode::Down)), OverlayResult::Continue);
        // Already at index 2; Down at boundary stays put.
        assert_eq!(c.on_key(keycode(KeyCode::Down)), OverlayResult::Continue);
        assert_eq!(c.on_key(keycode(KeyCode::Enter)), OverlayResult::Picked(2));
    }

    #[test]
    fn clarify_ignores_out_of_range_quickpick() {
        let mut c = Overlay::clarify("pick", vec!["one".into(), "two".into()]);
        assert_eq!(c.on_key(key('9')), OverlayResult::Continue);
    }

    #[test]
    fn password_accumulates_and_submits() {
        let mut p = Overlay::password("secret?", "paste your API key");
        for c in "hunter2".chars() {
            assert_eq!(p.on_key(key(c)), OverlayResult::Continue);
        }
        let out = p.on_key(keycode(KeyCode::Enter));
        assert_eq!(out, OverlayResult::Password("hunter2".into()));
    }

    #[test]
    fn password_backspace_pops() {
        let mut p = Overlay::password("?", "");
        p.on_key(key('a'));
        p.on_key(key('b'));
        p.on_key(keycode(KeyCode::Backspace));
        if let OverlayResult::Password(s) = p.on_key(keycode(KeyCode::Enter)) {
            assert_eq!(s, "a");
        } else {
            panic!("expected Password");
        }
    }

    // ─── Sprint 5 §9 — Picker / Todo ───────────────────────────────────

    fn sample_rows(n: usize) -> Vec<PickerRow> {
        (0..n)
            .map(|i| PickerRow::new(format!("primary-{i}"), format!("secondary-{i}")))
            .collect()
    }

    #[test]
    fn model_picker_carries_model_kind() {
        let p = Overlay::model_picker(sample_rows(3));
        match p {
            Overlay::Picker { kind, .. } => assert_eq!(kind, PickerKind::Model),
            _ => panic!("expected Picker"),
        }
    }

    #[test]
    fn session_picker_arrows_then_enter() {
        let mut p = Overlay::session_picker(sample_rows(5));
        assert_eq!(p.on_key(keycode(KeyCode::Down)), OverlayResult::Continue);
        assert_eq!(p.on_key(keycode(KeyCode::Down)), OverlayResult::Continue);
        assert_eq!(p.on_key(keycode(KeyCode::Enter)), OverlayResult::Picked(2));
    }

    #[test]
    fn agents_picker_pageup_pagedown() {
        let mut p = Overlay::agents_picker(sample_rows(40));
        // Start at 0; PageDown jumps +10.
        assert_eq!(
            p.on_key(keycode(KeyCode::PageDown)),
            OverlayResult::Continue
        );
        if let Overlay::Picker { cursor, .. } = p {
            assert_eq!(cursor, 10);
        }
    }

    #[test]
    fn picker_home_end_keys() {
        let mut p = Overlay::skills_hub(sample_rows(20));
        p.on_key(keycode(KeyCode::End));
        if let Overlay::Picker { cursor, .. } = &p {
            assert_eq!(*cursor, 19);
        }
        p.on_key(keycode(KeyCode::Home));
        if let Overlay::Picker { cursor, .. } = p {
            assert_eq!(cursor, 0);
        }
    }

    #[test]
    fn empty_picker_enter_cancels() {
        let mut p = Overlay::model_picker(vec![]);
        assert_eq!(p.on_key(keycode(KeyCode::Enter)), OverlayResult::Cancelled);
    }

    #[test]
    fn picker_esc_cancels() {
        let mut p = Overlay::model_picker(sample_rows(2));
        assert_eq!(p.on_key(keycode(KeyCode::Esc)), OverlayResult::Cancelled);
    }

    #[test]
    fn picker_down_at_last_stays_put() {
        let mut p = Overlay::model_picker(sample_rows(3));
        p.on_key(keycode(KeyCode::End));
        p.on_key(keycode(KeyCode::Down));
        if let Overlay::Picker { cursor, .. } = p {
            assert_eq!(cursor, 2);
        }
    }

    #[test]
    fn todo_status_cycles_through_three_states() {
        let mut t = Overlay::todo(
            "Pending tasks",
            vec![TodoItem::new("write tests"), TodoItem::new("ship feature")],
        );
        assert_eq!(t.on_key(key(' ')), OverlayResult::Continue);
        if let Overlay::Todo { items, .. } = &t {
            assert_eq!(items[0].status, TodoStatus::InProgress);
        }
        t.on_key(key('x'));
        if let Overlay::Todo { items, .. } = &t {
            assert_eq!(items[0].status, TodoStatus::Done);
        }
        t.on_key(key(' '));
        if let Overlay::Todo { items, .. } = t {
            assert_eq!(items[0].status, TodoStatus::Pending);
        }
    }

    #[test]
    fn todo_enter_returns_picked() {
        let mut t = Overlay::todo(
            "tasks",
            vec![TodoItem::new("a"), TodoItem::new("b"), TodoItem::new("c")],
        );
        t.on_key(keycode(KeyCode::Down));
        assert_eq!(t.on_key(keycode(KeyCode::Enter)), OverlayResult::Picked(1));
    }

    #[test]
    fn todo_empty_enter_cancels() {
        let mut t = Overlay::todo("tasks", vec![]);
        assert_eq!(t.on_key(keycode(KeyCode::Enter)), OverlayResult::Cancelled);
    }

    #[test]
    fn picker_kind_string_tag() {
        assert_eq!(PickerKind::Model.as_str(), "model");
        assert_eq!(PickerKind::Session.as_str(), "session");
        assert_eq!(PickerKind::Agents.as_str(), "agents");
        assert_eq!(PickerKind::Skills.as_str(), "skills");
    }
}
