//! Direct Preference Optimization (DPO) JSONL writer.
//!
//! The on-wire schema matches Hermes / Atropos:
//!
//! ```jsonl
//! {"prompt":"…","chosen":"…","rejected":"…",
//!  "gaussclaw_meta":{"session_id","prompt_turn_id","chosen_turn_id","rejected_turn_id","taint","ts"}}
//! ```
//!
//! `gaussclaw_meta` is additive — Hermes parsers ignore it and still
//! read the canonical preference pair.

use std::io;

use gauss_core::TaintLabel;
use gaussclaw_store::Turn;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// One preference pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DpoRecord {
    /// User prompt the chosen and rejected replies both answer.
    pub prompt: String,
    /// Preferred assistant reply.
    pub chosen: String,
    /// Less-preferred assistant reply.
    pub rejected: String,
    /// Additive GaussClaw metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gaussclaw_meta: Option<DpoMeta>,
}

/// GaussClaw-additive DPO metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DpoMeta {
    /// Originating session id.
    pub session_id: String,
    /// Turn id of the user prompt.
    pub prompt_turn_id: u64,
    /// Turn id of the chosen reply.
    pub chosen_turn_id: u64,
    /// Turn id of the rejected reply.
    pub rejected_turn_id: u64,
    /// Information-flow taint (join of the three turns' taints).
    pub taint: TaintLabel,
    /// RFC3339 timestamp of the chosen turn.
    pub ts: String,
}

impl DpoRecord {
    /// Build a record without metadata (Hermes byte-equivalent shape).
    #[must_use]
    pub fn new(
        prompt: impl Into<String>,
        chosen: impl Into<String>,
        rejected: impl Into<String>,
    ) -> Self {
        Self {
            prompt: prompt.into(),
            chosen: chosen.into(),
            rejected: rejected.into(),
            gaussclaw_meta: None,
        }
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_meta(mut self, meta: DpoMeta) -> Self {
        self.gaussclaw_meta = Some(meta);
        self
    }
}

/// Convert a flat turn list into DPO pairs.
///
/// The pairing convention: for each `user` turn, if there are two or
/// more immediately-following `assistant` turns sharing the same parent,
/// emit one pair `(user → assistant_a vs assistant_b)`. The order of
/// `chosen` and `rejected` is determined by `prefer_first_assistant`:
/// when `true`, the first assistant reply is `chosen`; when `false`,
/// the second is `chosen`. This is enough to round-trip a Hermes-style
/// "regenerate vs original" pair-mining workflow.
///
/// Returns an empty vec if no qualifying pair exists.
#[must_use]
pub fn into_dpo_pairs(turns: &[Turn], prefer_first_assistant: bool) -> Vec<DpoRecord> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < turns.len() {
        if turns[i].role == "user" {
            // Collect every immediately-following assistant turn that shares
            // this user turn as its parent.
            let parent_id = turns[i].id;
            let mut assistants: Vec<&Turn> = Vec::new();
            let mut j = i.saturating_add(1);
            while j < turns.len()
                && turns[j].role == "assistant"
                && turns[j].parent_id == Some(parent_id)
            {
                assistants.push(&turns[j]);
                j = j.saturating_add(1);
            }
            if assistants.len() >= 2 {
                let (chosen, rejected) = if prefer_first_assistant {
                    (assistants[0], assistants[1])
                } else {
                    (assistants[1], assistants[0])
                };
                let taint = turns[i].taint.join(chosen.taint).join(rejected.taint);
                let meta = DpoMeta {
                    session_id: turns[i].session_id.clone(),
                    prompt_turn_id: turns[i].id,
                    chosen_turn_id: chosen.id,
                    rejected_turn_id: rejected.id,
                    taint,
                    ts: chosen.ts.clone(),
                };
                out.push(
                    DpoRecord::new(&turns[i].content, &chosen.content, &rejected.content)
                        .with_meta(meta),
                );
            }
            i = j;
        } else {
            i = i.saturating_add(1);
        }
    }
    out
}

/// Streaming JSONL writer for [`DpoRecord`].
pub struct DpoWriter<W: AsyncWrite + Unpin> {
    inner: W,
    written: u64,
}

impl<W: AsyncWrite + Unpin> DpoWriter<W> {
    /// Build a writer.
    pub fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    /// Records written so far.
    #[must_use]
    pub const fn written(&self) -> u64 {
        self.written
    }

    /// Write one record as `<json>\n`.
    ///
    /// # Errors
    /// Forwards any underlying I/O error.
    pub async fn write_record(&mut self, r: &DpoRecord) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(r)?;
        bytes.push(b'\n');
        self.inner.write_all(&bytes).await?;
        self.written = self.written.saturating_add(1);
        Ok(())
    }

    /// Flush the underlying writer.
    ///
    /// # Errors
    /// Forwards any flush error.
    pub async fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().await
    }

    /// Consume the writer and return the inner sink.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaussclaw_store::SessionStore;

    async fn session_with_two_assistant_branches() -> Vec<Turn> {
        let s = SessionStore::open_in_memory().await.unwrap();
        let sess = s.create_session("tui", "m").await;
        let (u, _) = s
            .append_turn(&sess.id, None, "user", "joke?", TaintLabel::User)
            .await
            .unwrap();
        s.append_turn(
            &sess.id,
            Some(u.id),
            "assistant",
            "first joke",
            TaintLabel::User,
        )
        .await
        .unwrap();
        s.append_turn(
            &sess.id,
            Some(u.id),
            "assistant",
            "second joke",
            TaintLabel::User,
        )
        .await
        .unwrap();
        s.list_session_turns(&sess.id).await
    }

    #[tokio::test]
    async fn into_dpo_pairs_emits_one_record_for_two_assistants_under_one_user() {
        let turns = session_with_two_assistant_branches().await;
        let pairs = into_dpo_pairs(&turns, true);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].prompt, "joke?");
        assert_eq!(pairs[0].chosen, "first joke");
        assert_eq!(pairs[0].rejected, "second joke");
    }

    #[tokio::test]
    async fn into_dpo_pairs_flip_swaps_chosen_rejected() {
        let turns = session_with_two_assistant_branches().await;
        let pairs = into_dpo_pairs(&turns, false);
        assert_eq!(pairs[0].chosen, "second joke");
        assert_eq!(pairs[0].rejected, "first joke");
    }

    #[tokio::test]
    async fn single_assistant_emits_no_pair() {
        let s = SessionStore::open_in_memory().await.unwrap();
        let sess = s.create_session("tui", "m").await;
        let (u, _) = s
            .append_turn(&sess.id, None, "user", "hi", TaintLabel::User)
            .await
            .unwrap();
        s.append_turn(&sess.id, Some(u.id), "assistant", "hello", TaintLabel::User)
            .await
            .unwrap();
        let turns = s.list_session_turns(&sess.id).await;
        assert!(into_dpo_pairs(&turns, true).is_empty());
    }

    #[tokio::test]
    async fn writer_round_trips() {
        let turns = session_with_two_assistant_branches().await;
        let pairs = into_dpo_pairs(&turns, true);
        let mut buf: Vec<u8> = Vec::new();
        let mut w = DpoWriter::new(&mut buf);
        for p in &pairs {
            w.write_record(p).await.unwrap();
        }
        w.flush().await.unwrap();
        drop(w);
        let parsed: DpoRecord = serde_json::from_slice(buf.trim_ascii_end()).unwrap();
        assert_eq!(parsed.chosen, "first joke");
    }

    #[test]
    fn record_without_meta_serialises_as_pure_hermes_shape() {
        let r = DpoRecord::new("p", "c", "r");
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"prompt":"p","chosen":"c","rejected":"r"}"#);
    }
}
