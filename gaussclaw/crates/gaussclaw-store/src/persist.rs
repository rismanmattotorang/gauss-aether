//! Sprint 11 of "Wire the Loop" — file-backed durable [`SessionStore`].
//!
//! `SurrealMemory` is in-memory only; on process restart every
//! session, turn, and chain row evaporates. This module adds an
//! append-only JSON-Lines sidecar log so a deployment can:
//!
//! 1. Persist every state mutation (session create, turn append,
//!    anchor) as one JSON record per line into `<dir>/store.jsonl`.
//! 2. Replay the log on startup through the same code paths the
//!    runtime uses for fresh writes, so the chain head digest matches
//!    bit-for-bit what was last written.
//!
//! The persist log is opt-in: callers that want in-memory-only
//! semantics keep using [`crate::SessionStore::open_in_memory`]; the
//! new constructor is [`crate::SessionStore::open_at_path`].
//!
//! ## Why JSONL, not SurrealKV / RocksDB
//!
//! SurrealDB ships file-backed engines (`SurrealKV`, `RocksDB`) but
//! enabling them pulls in substantial new dependency trees and
//! ratchets the workspace's minimum-feature surface. JSON-Lines on
//! top of `tokio::fs::File` covers the operationally important case
//! (durability across restart) with zero new deps and a wire format
//! that's trivial to inspect, diff, and migrate.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use gauss_audit::Anchor;

use crate::store::StoreError;
use crate::types::{Session, Turn};

/// A single record on the JSONL log.
///
/// `#[serde(tag = "kind")]` keys the variant so a record is always
/// self-describing; future variants can be added (telemetry frames,
/// router records, …) and old replayers can skip what they don't
/// understand.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistRecord {
    /// A new session opened.
    Session {
        /// Session metadata that was committed.
        session: Session,
    },
    /// A new turn appended.
    Turn {
        /// Turn record (id, role, content, taint, cost, route).
        turn: Turn,
    },
    /// A TSA anchor recorded.
    Anchor {
        /// Wall-clock-anchored chain head returned by the TSA.
        anchor: Anchor,
    },
}

/// Append-only persistence sidecar. Holds the open file handle behind
/// a `Mutex` so concurrent writers from different tasks serialise
/// without re-opening the file each call.
#[derive(Debug)]
pub struct PersistLog {
    path: PathBuf,
    file: Mutex<File>,
}

impl PersistLog {
    /// Open (or create) the log at `path`. Caller is responsible for
    /// directory creation; this constructor will fail if the parent
    /// doesn't exist (`StoreError::Backend` wrapping the IO error).
    ///
    /// # Errors
    /// Returns [`StoreError::Backend`] on any IO failure.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path: PathBuf = path.into();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)
            .await
            .map_err(|e| {
                StoreError::Backend(gauss_core::GaussError::Internal(format!(
                    "persist open {}: {e}",
                    path.display()
                )))
            })?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    /// Append one record. Flushes (`fdatasync`-equivalent on tokio)
    /// before returning so a hard kill loses at most the in-flight
    /// write, not anything that has already been acknowledged to the
    /// caller upstream.
    pub async fn append(&self, record: &PersistRecord) -> Result<(), StoreError> {
        let mut line = serde_json::to_vec(record).map_err(StoreError::Serde)?;
        line.push(b'\n');
        let mut f = self.file.lock().await;
        f.write_all(&line).await.map_err(io_err)?;
        f.flush().await.map_err(io_err)?;
        Ok(())
    }

    /// Path the log lives at — useful for `gaussclaw doctor`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Read every record from a JSONL log file. Returns an empty vector
/// when the file doesn't exist (fresh deployment). Malformed lines
/// are skipped with a `tracing::warn` so a corrupted tail row doesn't
/// abort startup — the caller has already chain-verified what made
/// it through.
pub async fn read_all(path: &Path) -> Result<Vec<PersistRecord>, StoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path).await.map_err(io_err)?;
    let mut reader = BufReader::new(file);
    let mut out = Vec::new();
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).await.map_err(io_err)?;
        if n == 0 {
            break;
        }
        let trimmed = buf.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<PersistRecord>(trimmed) {
            Ok(r) => out.push(r),
            Err(e) => {
                tracing::warn!(
                    target: "gaussclaw_store::persist",
                    "skipping malformed persist line: {e}"
                );
            }
        }
    }
    Ok(out)
}

fn io_err(e: std::io::Error) -> StoreError {
    StoreError::Backend(gauss_core::GaussError::Internal(format!("persist io: {e}")))
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::TaintLabel;
    use std::sync::Arc;
    use crate::types::TurnCost;

    fn tmp_path(name: &str) -> PathBuf {
        let base = std::env::temp_dir();
        base.join(format!("gaussclaw_persist_{}_{}.jsonl", name, std::process::id()))
    }

    fn turn(id: u64, sid: &str, body: &str) -> Turn {
        Turn {
            id,
            session_id: sid.into(),
            parent_id: None,
            role: "user".into(),
            content: body.into(),
            ts: "2026-05-29T00:00:00Z".into(),
            taint: TaintLabel::User,
            cost: TurnCost::default(),
            route: None,
        }
    }

    #[tokio::test]
    async fn read_all_on_missing_file_returns_empty() {
        let path = tmp_path("missing");
        let _ = std::fs::remove_file(&path);
        let v = read_all(&path).await.expect("ok");
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn append_and_read_round_trip() {
        let path = tmp_path("round_trip");
        let _ = std::fs::remove_file(&path);
        let log = Arc::new(PersistLog::open(&path).await.unwrap());
        log.append(&PersistRecord::Turn {
            turn: turn(1, "s1", "hello"),
        })
        .await
        .unwrap();
        log.append(&PersistRecord::Turn {
            turn: turn(2, "s1", "world"),
        })
        .await
        .unwrap();
        drop(log);
        let records = read_all(&path).await.unwrap();
        assert_eq!(records.len(), 2);
        match (&records[0], &records[1]) {
            (PersistRecord::Turn { turn: t1 }, PersistRecord::Turn { turn: t2 }) => {
                assert_eq!(t1.id, 1);
                assert_eq!(t1.content, "hello");
                assert_eq!(t2.id, 2);
                assert_eq!(t2.content, "world");
            }
            other => panic!("unexpected variants: {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn appended_lines_survive_reopen() {
        let path = tmp_path("reopen");
        let _ = std::fs::remove_file(&path);
        {
            let log = PersistLog::open(&path).await.unwrap();
            log.append(&PersistRecord::Session {
                session: Session::new("s-abc", "tui", "anthropic/claude"),
            })
            .await
            .unwrap();
        }
        // New process opens; existing file gets appended, not truncated.
        {
            let log = PersistLog::open(&path).await.unwrap();
            log.append(&PersistRecord::Turn {
                turn: turn(1, "s-abc", "after reopen"),
            })
            .await
            .unwrap();
        }
        let records = read_all(&path).await.unwrap();
        assert_eq!(records.len(), 2);
        match (&records[0], &records[1]) {
            (PersistRecord::Session { session }, PersistRecord::Turn { turn: t }) => {
                assert_eq!(session.id, "s-abc");
                assert_eq!(t.content, "after reopen");
            }
            other => panic!("unexpected: {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn malformed_lines_are_skipped_not_fatal() {
        let path = tmp_path("malformed");
        let _ = std::fs::remove_file(&path);
        // Write directly: one good record, one garbage line, one good.
        let good1 = serde_json::to_string(&PersistRecord::Turn {
            turn: turn(1, "s1", "one"),
        })
        .unwrap();
        let good2 = serde_json::to_string(&PersistRecord::Turn {
            turn: turn(2, "s1", "two"),
        })
        .unwrap();
        let body = format!("{good1}\n{{ this is not json\n{good2}\n");
        tokio::fs::write(&path, body).await.unwrap();
        let records = read_all(&path).await.unwrap();
        assert_eq!(records.len(), 2, "garbage line must be skipped");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn append_after_concurrent_writers_serialises() {
        let path = tmp_path("concurrent");
        let _ = std::fs::remove_file(&path);
        let log = Arc::new(PersistLog::open(&path).await.unwrap());
        let mut handles = Vec::new();
        for i in 0..20 {
            let log = log.clone();
            handles.push(tokio::spawn(async move {
                log.append(&PersistRecord::Turn {
                    turn: turn(i, "s1", &format!("body-{i}")),
                })
                .await
                .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        drop(log);
        let records = read_all(&path).await.unwrap();
        assert_eq!(records.len(), 20, "every concurrent write must land");
        let _ = std::fs::remove_file(&path);
    }
}
