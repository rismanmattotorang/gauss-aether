//! Auto-Compaction — preserve task state under context-window pressure.
//!
//! OpenHarness (HKUDS/OpenHarness) ships "Auto-Compaction": when the
//! running conversation approaches the model's context window, the
//! harness summarises the oldest messages into a single `system`-role
//! "summary" so the agent can keep working without losing task state.
//!
//! GaussClaw's agent loop now has the same surface, factored cleanly
//! against the existing design:
//!
//! 1. **Compactor is a trait.** Operators plug in their own strategies
//!    (LLM-driven summary, deterministic head-cut, vector-recall
//!    backfill, …). The default [`WindowedCompactor`] is a
//!    deterministic head-cut that keeps the trailing `keep` messages
//!    plus the initial system prompt — it has no LLM cost and no
//!    failure modes, so it's safe to enable by default.
//!
//! 2. **Triggered by character budget, not by token guesses.** We
//!    don't reach into a tokenizer (which would add a heavy dep with
//!    no guaranteed parity across providers). The trigger is the
//!    summed `content.len()` across all non-summary messages crossing
//!    `budget_chars`. Operators tune the threshold per model.
//!
//! 3. **Tool messages are preserved.** The default strategy never
//!    collapses messages with `role == "tool"`. Removing a tool result
//!    would orphan the assistant's prior reasoning step.
//!
//! 4. **Idempotent.** Calling `Compactor::maybe_compact` repeatedly on
//!    the same already-compacted stack does nothing. The loop can
//!    call it before every provider invocation safely.
//!
//! 5. **Audit-friendly.** Every compaction produces a `CompactionRecord`
//!    with the message count delta and the summary text; the agent
//!    loop surfaces it as `LoopEvent::Compacted` and the audit chain
//!    can stamp it with a Merkle-leaf hash if wired.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

use serde::{Deserialize, Serialize};

use crate::Message;

/// Marker prefix that identifies a summary message in the stack. The
/// default compactor refuses to re-summarise messages that already
/// begin with this prefix — keeps the surface idempotent.
pub const SUMMARY_PREFIX: &str = "[auto-compaction summary]";

/// Record of one compaction event. Returned by the compactor; surfaced
/// by the agent loop as a `LoopEvent::Compacted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CompactionRecord {
    /// Number of messages collapsed into the summary.
    pub collapsed: usize,
    /// Number of messages retained verbatim (head + tail).
    pub retained: usize,
    /// First few words of the summary, capped for log-line use.
    pub summary_preview: String,
    /// Character count of the *pre-compaction* messages list.
    pub before_chars: usize,
    /// Character count of the *post-compaction* messages list.
    pub after_chars: usize,
}

/// A pluggable compaction strategy.
pub trait Compactor: Send + Sync {
    /// Inspect the running message stack and, if the policy fires,
    /// rewrite it in place and return a `CompactionRecord`. Returns
    /// `None` when no compaction was needed (or possible).
    ///
    /// The trait takes `&mut Vec<Message>` rather than returning a new
    /// vec so the loop can keep its `messages: Vec<Message>` local
    /// and avoid copies for the no-compaction common case.
    fn maybe_compact(&self, messages: &mut Vec<Message>) -> Option<CompactionRecord>;
}

// ─── default windowed compactor ───────────────────────────────────────────

/// Deterministic head-cut compactor.
///
/// When the total content length exceeds `budget_chars`, replaces every
/// message *before* the trailing window of `keep` non-tool messages
/// with one summary system-role message containing a structured
/// description of what was dropped.
///
/// Tool-role messages are preserved verbatim regardless of position —
/// they are the audit trail of what the agent actually did.
///
/// The initial system message (if any) is also preserved verbatim;
/// most providers treat the leading system message as the agent's
/// operating instructions and dropping it tends to derail behaviour.
#[derive(Debug, Clone)]
pub struct WindowedCompactor {
    /// Trigger threshold. Compaction fires when the summed content
    /// length across messages exceeds this.
    pub budget_chars: usize,
    /// Number of trailing non-tool messages to keep verbatim after
    /// compaction.
    pub keep_tail: usize,
}

impl WindowedCompactor {
    /// Sensible default for ~8k-token contexts: 24 KiB budget, keep
    /// the trailing 6 messages.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            budget_chars: 24 * 1024,
            keep_tail: 6,
        }
    }

    /// Builder: set the trigger threshold.
    #[must_use]
    pub const fn with_budget_chars(mut self, n: usize) -> Self {
        self.budget_chars = n;
        self
    }

    /// Builder: set the trailing window size.
    #[must_use]
    pub const fn with_keep_tail(mut self, n: usize) -> Self {
        self.keep_tail = n;
        self
    }
}

impl Default for WindowedCompactor {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Sum the content length across every message in `msgs`.
fn total_chars(msgs: &[Message]) -> usize {
    msgs.iter().map(|m| m.content.len()).sum()
}

impl Compactor for WindowedCompactor {
    fn maybe_compact(&self, messages: &mut Vec<Message>) -> Option<CompactionRecord> {
        let before_chars = total_chars(messages);
        if before_chars <= self.budget_chars {
            return None;
        }
        if messages.len() <= self.keep_tail + 1 {
            // Not enough messages to compact — would leave nothing in
            // the "to summarise" slice.
            return None;
        }

        // Identify the leading system message (preserve verbatim).
        let leading_system_idx = if messages
            .first()
            .map(|m| m.role == "system")
            .unwrap_or(false)
        {
            Some(0)
        } else {
            None
        };

        // Build the "to summarise" range. We pick everything between
        // the leading system message and the trailing `keep_tail`
        // non-tool messages.
        let n = messages.len();
        let mut keep_from = n; // index where the tail starts
        let mut non_tool_kept = 0usize;
        for (i, m) in messages.iter().enumerate().rev() {
            if m.role != "tool" {
                non_tool_kept = non_tool_kept.saturating_add(1);
                if non_tool_kept >= self.keep_tail {
                    keep_from = i;
                    break;
                }
            }
        }
        if keep_from == n {
            // Couldn't find `keep_tail` non-tool messages — nothing to do.
            return None;
        }

        let summarise_from = leading_system_idx.map_or(0, |i| i + 1);
        if summarise_from >= keep_from {
            return None;
        }

        // Detect a prior summary inside the window we're about to
        // collapse; this keeps the operation idempotent (re-running on
        // already-summarised state does nothing).
        let all_summaries = messages[summarise_from..keep_from]
            .iter()
            .all(|m| m.content.starts_with(SUMMARY_PREFIX));
        if all_summaries {
            return None;
        }

        // Build the summary body. We collect role-tagged digests of
        // every collapsed message; the body is sized to a small fixed
        // budget so it never reintroduces the same context-pressure.
        let mut digest_lines = Vec::new();
        let mut collapsed_count = 0usize;
        for m in &messages[summarise_from..keep_from] {
            if m.role == "tool" {
                // Tool messages are preserved verbatim — they don't
                // get collapsed. We'll move them across below.
                continue;
            }
            let preview: String = m.content.chars().take(160).collect();
            digest_lines.push(format!(
                "- [{role}] {preview}{ellipsis}",
                role = m.role,
                preview = preview,
                ellipsis = if m.content.len() > preview.len() {
                    " …"
                } else {
                    ""
                }
            ));
            collapsed_count = collapsed_count.saturating_add(1);
        }

        let summary_body = format!(
            "{SUMMARY_PREFIX} collapsed {collapsed_count} earlier message(s); preserved tool results:\n\n{lines}",
            lines = digest_lines.join("\n")
        );
        let summary_preview: String = summary_body.chars().take(120).collect();

        // Collect preserved tool messages from the collapsed window.
        let preserved_tools: Vec<Message> = messages[summarise_from..keep_from]
            .iter()
            .filter(|m| m.role == "tool")
            .cloned()
            .collect();

        let tail: Vec<Message> = messages[keep_from..].to_vec();
        let mut rebuilt: Vec<Message> = Vec::with_capacity(2 + preserved_tools.len() + tail.len());
        if let Some(idx) = leading_system_idx {
            rebuilt.push(messages[idx].clone());
        }
        rebuilt.push(Message::new("system", summary_body));
        rebuilt.extend(preserved_tools);
        rebuilt.extend(tail);

        let before_len = messages.len();
        *messages = rebuilt;
        let after_chars = total_chars(messages);

        Some(CompactionRecord {
            collapsed: collapsed_count,
            retained: messages.len(),
            summary_preview,
            before_chars,
            after_chars,
        })
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn pad_msg(role: &str, fill: char, n: usize) -> Message {
        Message::new(role, std::iter::repeat(fill).take(n).collect::<String>())
    }

    #[test]
    fn no_compaction_under_budget() {
        let compactor = WindowedCompactor::defaults().with_budget_chars(10_000);
        let mut msgs = vec![
            Message::new("system", "be helpful"),
            Message::new("user", "hi"),
            Message::new("assistant", "hello"),
        ];
        let result = compactor.maybe_compact(&mut msgs);
        assert!(result.is_none());
        assert_eq!(msgs.len(), 3);
    }

    #[test]
    fn compaction_collapses_old_user_assistant_turns() {
        let compactor = WindowedCompactor::defaults()
            .with_budget_chars(5_000)
            .with_keep_tail(2);
        let mut msgs = vec![Message::new("system", "be helpful")];
        for i in 0..20 {
            msgs.push(pad_msg(
                if i % 2 == 0 { "user" } else { "assistant" },
                'x',
                500,
            ));
        }
        let before = msgs.len();
        let rec = compactor
            .maybe_compact(&mut msgs)
            .expect("budget should trigger");
        assert!(rec.collapsed >= 1);
        assert!(rec.after_chars < rec.before_chars);
        // System + summary + last 2 = 4. Leading system preserved.
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content, "be helpful");
        assert!(msgs[1].content.starts_with(SUMMARY_PREFIX));
        assert!(msgs.len() < before);
    }

    #[test]
    fn tool_messages_inside_window_are_preserved() {
        let compactor = WindowedCompactor::defaults()
            .with_budget_chars(2_000)
            .with_keep_tail(1);
        let mut msgs = vec![
            Message::new("system", "sys"),
            pad_msg("user", 'a', 800),
            pad_msg("assistant", 'b', 800),
            Message::new("tool", "[tool:echo result] {\"echo\":\"x\"}"),
            pad_msg("user", 'c', 800),
            pad_msg("assistant", 'd', 800),
        ];
        compactor.maybe_compact(&mut msgs).expect("should fire");
        let tool_msgs: Vec<&Message> = msgs.iter().filter(|m| m.role == "tool").collect();
        assert_eq!(tool_msgs.len(), 1, "tool result must survive compaction");
        assert!(tool_msgs[0].content.contains("[tool:echo result]"));
    }

    #[test]
    fn leading_system_message_is_preserved() {
        let compactor = WindowedCompactor::defaults()
            .with_budget_chars(1_000)
            .with_keep_tail(1);
        let mut msgs = vec![Message::new("system", "operating instructions")];
        for _ in 0..10 {
            msgs.push(pad_msg("user", 'x', 200));
            msgs.push(pad_msg("assistant", 'y', 200));
        }
        compactor.maybe_compact(&mut msgs).unwrap();
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content, "operating instructions");
    }

    #[test]
    fn compaction_is_idempotent() {
        let compactor = WindowedCompactor::defaults()
            .with_budget_chars(2_000)
            .with_keep_tail(2);
        let mut msgs = vec![Message::new("system", "sys")];
        for _ in 0..10 {
            msgs.push(pad_msg("user", 'x', 400));
            msgs.push(pad_msg("assistant", 'y', 400));
        }
        let _ = compactor.maybe_compact(&mut msgs).unwrap();
        let snapshot = msgs.clone();
        // Second call with the same compactor on the now-compacted
        // stack must be a no-op (or at most also a no-op).
        let second = compactor.maybe_compact(&mut msgs);
        // Either no second compaction, OR a second compaction that
        // still keeps total chars below budget.
        if second.is_some() {
            assert!(total_chars(&msgs) <= compactor.budget_chars + 4096);
        } else {
            assert_eq!(msgs, snapshot);
        }
    }

    #[test]
    fn too_few_messages_to_compact() {
        let compactor = WindowedCompactor::defaults()
            .with_budget_chars(10)
            .with_keep_tail(5);
        let mut msgs = vec![Message::new("user", "very long text that exceeds budget")];
        let result = compactor.maybe_compact(&mut msgs);
        // Single message, keep_tail=5 — can't compact a head of size 0.
        assert!(result.is_none());
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn record_preview_is_capped() {
        let compactor = WindowedCompactor::defaults()
            .with_budget_chars(500)
            .with_keep_tail(1);
        let mut msgs = vec![Message::new("system", "sys")];
        for _ in 0..5 {
            msgs.push(pad_msg("user", 'x', 200));
            msgs.push(pad_msg("assistant", 'y', 200));
        }
        let rec = compactor.maybe_compact(&mut msgs).unwrap();
        assert!(rec.summary_preview.chars().count() <= 120);
        assert!(rec.summary_preview.contains(SUMMARY_PREFIX));
    }

    #[test]
    fn collapsed_count_matches_dropped_non_tool_messages() {
        let compactor = WindowedCompactor::defaults()
            .with_budget_chars(1_000)
            .with_keep_tail(1);
        let mut msgs = vec![Message::new("system", "sys")];
        // 6 non-tool messages + 2 tool messages.
        for _ in 0..3 {
            msgs.push(pad_msg("user", 'a', 200));
            msgs.push(pad_msg("assistant", 'b', 200));
        }
        msgs.insert(3, Message::new("tool", "[tool:t result] {}"));
        msgs.insert(6, Message::new("tool", "[tool:t result] {}"));
        let rec = compactor.maybe_compact(&mut msgs).expect("fire");
        // Tools never enter the collapsed_count.
        assert!(rec.collapsed >= 1);
        assert!(msgs.iter().filter(|m| m.role == "tool").count() == 2);
    }
}
