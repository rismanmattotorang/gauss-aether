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
        }
    }

    /// Render the overlay centred over `area`. Uses [`Clear`] first so
    /// the underlying pane is hidden behind a solid panel.
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let panel = centre(area, 60, 14);
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

                let mut lines: Vec<Line> = body.iter().map(|l| Line::raw(l.clone())).collect();
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

                let mut lines: Vec<Line> = options
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

                let mut lines: Vec<Line> = Vec::new();
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
}
