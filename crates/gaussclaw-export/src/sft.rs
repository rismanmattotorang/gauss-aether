//! Supervised Fine-Tuning (SFT) JSONL writer.
//!
//! The on-wire schema is the OpenAI / Hermes / Atropos-compatible
//! `{"messages":[{"role","content"},…]}` form. Every Hermes consumer
//! parses a GaussClaw SFT record without code changes; the
//! `gaussclaw_meta` field is additive and consumers that ignore it
//! still read the canonical bytes.
//!
//! ## Wire shape
//!
//! ```jsonl
//! {"messages":[{"role":"user","content":"…"},{"role":"assistant","content":"…"}],
//!  "gaussclaw_meta":{"session_id":"…","turn_id":42,"taint":"user","ts":"2025-…"}}
//! ```

use std::io;

use gauss_core::TaintLabel;
use gaussclaw_store::Turn;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// One chat message inside an SFT record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftMessage {
    /// `"user"` | `"assistant"` | `"system"` | `"tool"`.
    pub role: String,
    /// Free-text body.
    pub content: String,
}

impl SftMessage {
    /// Build a new message.
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// GaussClaw-additive metadata. Hermes consumers ignore this field;
/// envelope-aware consumers read it for the audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftMeta {
    /// Originating session id.
    pub session_id: String,
    /// Turn id of the assistant reply this record is built around.
    pub turn_id: u64,
    /// Information-flow taint of the assistant reply.
    pub taint: TaintLabel,
    /// RFC3339 timestamp of the assistant turn.
    pub ts: String,
}

/// One SFT JSONL record. Bit-for-bit Hermes parity on `messages`;
/// `gaussclaw_meta` is an additive optional field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftRecord {
    /// Conversation messages, in turn order.
    pub messages: Vec<SftMessage>,
    /// Additive GaussClaw metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gaussclaw_meta: Option<SftMeta>,
}

impl SftRecord {
    /// Build a record with no metadata (Hermes-byte-equivalent shape).
    #[must_use]
    pub fn from_messages(messages: Vec<SftMessage>) -> Self {
        Self {
            messages,
            gaussclaw_meta: None,
        }
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_meta(mut self, meta: SftMeta) -> Self {
        self.gaussclaw_meta = Some(meta);
        self
    }
}

/// Convert a session's [`Turn`] stream into SFT records.
///
/// Pairs each `assistant` turn with the nearest preceding `user`
/// (and `system` if present at the start) into one record. Turns that
/// don't pair leave no record. Tool turns are folded in as their own
/// messages so a fine-tuner that wants to learn tool-use sees them.
///
/// Bit-stable: every record has its messages in append order and its
/// `gaussclaw_meta` populated from the assistant turn.
#[must_use]
pub fn into_sft_records(turns: &[Turn]) -> Vec<SftRecord> {
    let mut out = Vec::new();
    let mut buffer: Vec<SftMessage> = Vec::new();
    // Carry a leading `system` across records — most fine-tuning corpora
    // keep the system prompt with every example.
    let mut leading_system: Option<SftMessage> = None;
    for t in turns {
        match t.role.as_str() {
            "system" => {
                let msg = SftMessage::new("system", &t.content);
                if buffer.is_empty() {
                    leading_system = Some(msg);
                } else {
                    buffer.push(msg);
                }
            }
            "assistant" => {
                buffer.push(SftMessage::new("assistant", &t.content));
                let mut messages: Vec<SftMessage> = Vec::new();
                if let Some(sys) = &leading_system {
                    messages.push(sys.clone());
                }
                messages.append(&mut buffer);
                let meta = SftMeta {
                    session_id: t.session_id.clone(),
                    turn_id: t.id,
                    taint: t.taint,
                    ts: t.ts.clone(),
                };
                out.push(SftRecord::from_messages(messages).with_meta(meta));
            }
            _ => {
                // user / tool / anything else accumulates until the
                // next assistant flush.
                buffer.push(SftMessage::new(&t.role, &t.content));
            }
        }
    }
    out
}

/// Streaming JSONL writer for [`SftRecord`].
pub struct SftWriter<W: AsyncWrite + Unpin> {
    inner: W,
    written: u64,
}

impl<W: AsyncWrite + Unpin> SftWriter<W> {
    /// Build a writer over `inner`.
    pub fn new(inner: W) -> Self {
        Self { inner, written: 0 }
    }

    /// Number of records written so far.
    #[must_use]
    pub const fn written(&self) -> u64 {
        self.written
    }

    /// Write one record as `<json>\n`.
    ///
    /// # Errors
    /// Forwards any I/O error from the underlying writer.
    pub async fn write_record(&mut self, r: &SftRecord) -> io::Result<()> {
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

    async fn three_turn_session() -> Vec<Turn> {
        let s = SessionStore::open_in_memory().await.unwrap();
        let sess = s.create_session("tui", "m").await;
        let (u, _) = s
            .append_turn(&sess.id, None, "user", "what is 2+2?", TaintLabel::User)
            .await
            .unwrap();
        let (a, _) = s
            .append_turn(
                &sess.id,
                Some(u.id),
                "assistant",
                "2+2 = 4",
                TaintLabel::User,
            )
            .await
            .unwrap();
        let (_u2, _) = s
            .append_turn(&sess.id, Some(a.id), "user", "thanks", TaintLabel::User)
            .await
            .unwrap();
        s.list_session_turns(&sess.id).await
    }

    #[tokio::test]
    async fn into_sft_records_pairs_user_assistant() {
        let turns = three_turn_session().await;
        let recs = into_sft_records(&turns);
        // Only one assistant turn → one SFT record.
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].messages.len(), 2);
        assert_eq!(recs[0].messages[0].role, "user");
        assert_eq!(recs[0].messages[1].role, "assistant");
        assert_eq!(recs[0].messages[1].content, "2+2 = 4");
        let meta = recs[0].gaussclaw_meta.as_ref().unwrap();
        assert_eq!(meta.taint, TaintLabel::User);
    }

    #[tokio::test]
    async fn into_sft_records_carries_leading_system_into_each_record() {
        let s = SessionStore::open_in_memory().await.unwrap();
        let sess = s.create_session("tui", "m").await;
        s.append_turn(&sess.id, None, "system", "you are helpful", TaintLabel::Trusted)
            .await
            .unwrap();
        s.append_turn(&sess.id, None, "user", "q1", TaintLabel::User)
            .await
            .unwrap();
        s.append_turn(&sess.id, None, "assistant", "a1", TaintLabel::User)
            .await
            .unwrap();
        s.append_turn(&sess.id, None, "user", "q2", TaintLabel::User)
            .await
            .unwrap();
        s.append_turn(&sess.id, None, "assistant", "a2", TaintLabel::User)
            .await
            .unwrap();
        let turns = s.list_session_turns(&sess.id).await;
        let recs = into_sft_records(&turns);
        assert_eq!(recs.len(), 2);
        for r in &recs {
            assert_eq!(r.messages[0].role, "system");
            assert_eq!(r.messages[0].content, "you are helpful");
        }
    }

    #[tokio::test]
    async fn writer_emits_one_jsonl_per_record() {
        let turns = three_turn_session().await;
        let recs = into_sft_records(&turns);
        let mut buf: Vec<u8> = Vec::new();
        let mut w = SftWriter::new(&mut buf);
        for r in &recs {
            w.write_record(r).await.unwrap();
        }
        w.flush().await.unwrap();
        assert_eq!(w.written(), 1);
        drop(w);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.ends_with('\n'));
        // Parses back as SFT record:
        let parsed: SftRecord = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(parsed.messages.len(), 2);
    }

    #[test]
    fn record_without_meta_serialises_as_pure_hermes_shape() {
        // Hermes byte-equivalent shape: no `gaussclaw_meta` field at all.
        let r = SftRecord::from_messages(vec![
            SftMessage::new("user", "hi"),
            SftMessage::new("assistant", "hello"),
        ]);
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("gaussclaw_meta"));
        assert_eq!(
            s,
            r#"{"messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"}]}"#
        );
    }
}
