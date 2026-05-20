//! `gaussclaw-kanban` — opt-in Kanban board.
//!
//! Sprint 8 §8 of `/ROADMAP.md`. Hermes ships a Kanban CLI / DB /
//! tool / plugin set; we mirror the data model with a typed Rust
//! board + cap-gated mutations. The crate is opt-in (Cargo
//! feature off by default in downstream crates); operators with
//! "use task tracker from agent" workflows enable it explicitly.
//!
//! Data model:
//!
//! - [`Board`] — top-level container keyed by `(peer, board_id)`.
//! - [`Column`] — ordered list of cards (e.g. `todo` / `doing` /
//!   `done`).
//! - [`Card`] — title + body + status + tags + created_at.
//!
//! Hermes-superiority axes:
//!
//! - **Cap-gated mutation.** Every `add_card` / `move_card` /
//!   `delete_card` admit-checks `cap:kanban:write` (aliased to
//!   the existing MEMORY_READ bit until a dedicated bit lands).
//! - **Deterministic id allocator.** Cards get monotonic `u64`
//!   ids per board; the conformance suite locks the trace from `0`.
//! - **Audit-aware receipts.** Every mutation returns a typed
//!   `BoardReceipt` the caller appends to the chain.

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_docs_in_private_items,
    clippy::too_long_first_doc_paragraph,
    clippy::significant_drop_tightening,
    clippy::too_many_arguments
)]
#![allow(rustdoc::broken_intra_doc_links)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use gauss_core::CapToken;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable card id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CardId(pub u64);

/// One card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    /// Stable id.
    pub id: CardId,
    /// Title.
    pub title: String,
    /// Body (free-form markdown).
    pub body: String,
    /// Operator-supplied tags.
    pub tags: Vec<String>,
    /// UNIX seconds the card was created.
    pub created_at: i64,
}

/// One column. Columns are ordered; cards within a column are also
/// ordered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// Column name (`todo`, `doing`, `done`, or operator-defined).
    pub name: String,
    /// Cards in display order.
    pub cards: Vec<Card>,
}

impl Column {
    /// Build an empty column.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            cards: Vec::new(),
        }
    }
}

/// One board — typed container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    /// Peer the board belongs to.
    pub peer: String,
    /// Board id (free-form).
    pub board_id: String,
    /// Columns in display order.
    pub columns: Vec<Column>,
    /// UNIX seconds the board was created.
    pub created_at: i64,
}

impl Board {
    /// Build a board with the canonical `todo` / `doing` / `done`
    /// columns.
    #[must_use]
    pub fn default_layout(peer: impl Into<String>, board_id: impl Into<String>, now: i64) -> Self {
        Self {
            peer: peer.into(),
            board_id: board_id.into(),
            columns: vec![
                Column::new("todo"),
                Column::new("doing"),
                Column::new("done"),
            ],
            created_at: now,
        }
    }
}

/// Mutation receipt — every successful op writes one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardReceipt {
    /// Operation tag.
    pub op: ReceiptOp,
    /// Board this op acted on.
    pub board: String,
    /// Card id (when applicable).
    pub card: Option<CardId>,
    /// Source column (for `move_card`).
    pub from: Option<String>,
    /// Destination column (for `move_card` or `add_card`).
    pub to: Option<String>,
    /// UNIX seconds.
    pub timestamp: i64,
}

/// Mutation op kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReceiptOp {
    /// Created a board.
    CreateBoard,
    /// Added a card.
    AddCard,
    /// Moved a card between columns.
    MoveCard,
    /// Deleted a card.
    DeleteCard,
    /// Removed a board.
    DeleteBoard,
}

/// Crate-wide error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KanbanError {
    /// Caller's grant didn't satisfy the required cap.
    #[error("admit refused: required 0x{required:016x} not in grant 0x{grant:016x}")]
    AdmitRefused {
        /// Required bits.
        required: u64,
        /// Granted bits.
        grant: u64,
    },
    /// Board not found.
    #[error("no such board: {peer}/{board_id}")]
    UnknownBoard {
        /// Peer.
        peer: String,
        /// Board id.
        board_id: String,
    },
    /// Column not found.
    #[error("no such column: {0:?}")]
    UnknownColumn(String),
    /// Card not found.
    #[error("no such card: {0}")]
    UnknownCard(u64),
    /// Backend / storage failure.
    #[error("backend: {0}")]
    Backend(String),
}

/// Crate-wide result.
pub type KanbanResult<T> = Result<T, KanbanError>;

/// In-process kanban store. Production deployments wire this through
/// the cross-session memory map for persistence.
#[derive(Debug, Default)]
pub struct KanbanStore {
    inner: Mutex<Inner>,
}

#[derive(Default, Debug)]
struct Inner {
    boards: BTreeMap<(String, String), Board>,
    next_card_id: u64,
}

impl KanbanStore {
    /// Build a fresh empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of boards.
    pub fn board_count(&self) -> usize {
        self.inner.lock().expect("poisoned").boards.len()
    }

    /// Create a new board with the canonical `todo`/`doing`/`done`
    /// columns. Cap-gated.
    pub fn create_board(
        &self,
        grant: CapToken,
        peer: &str,
        board_id: &str,
        now: i64,
    ) -> KanbanResult<BoardReceipt> {
        admit(grant)?;
        let board = Board::default_layout(peer, board_id, now);
        let key = (peer.to_string(), board_id.to_string());
        let mut g = self.inner.lock().expect("poisoned");
        g.boards.insert(key, board);
        Ok(BoardReceipt {
            op: ReceiptOp::CreateBoard,
            board: board_id.into(),
            card: None,
            from: None,
            to: None,
            timestamp: now,
        })
    }

    /// Add a card to a column. Cap-gated. Returns the receipt + the
    /// allocated card id.
    pub fn add_card(
        &self,
        grant: CapToken,
        peer: &str,
        board_id: &str,
        column: &str,
        title: impl Into<String>,
        body: impl Into<String>,
        tags: Vec<String>,
        now: i64,
    ) -> KanbanResult<(BoardReceipt, CardId)> {
        admit(grant)?;
        let mut g = self.inner.lock().expect("poisoned");
        g.next_card_id = g.next_card_id.saturating_add(1);
        let id = CardId(g.next_card_id);
        let key = (peer.to_string(), board_id.to_string());
        let board = g
            .boards
            .get_mut(&key)
            .ok_or_else(|| KanbanError::UnknownBoard {
                peer: peer.into(),
                board_id: board_id.into(),
            })?;
        let col = board
            .columns
            .iter_mut()
            .find(|c| c.name == column)
            .ok_or_else(|| KanbanError::UnknownColumn(column.into()))?;
        col.cards.push(Card {
            id,
            title: title.into(),
            body: body.into(),
            tags,
            created_at: now,
        });
        Ok((
            BoardReceipt {
                op: ReceiptOp::AddCard,
                board: board_id.into(),
                card: Some(id),
                from: None,
                to: Some(column.into()),
                timestamp: now,
            },
            id,
        ))
    }

    /// Move a card from one column to another. Cap-gated.
    pub fn move_card(
        &self,
        grant: CapToken,
        peer: &str,
        board_id: &str,
        from_column: &str,
        to_column: &str,
        card: CardId,
        now: i64,
    ) -> KanbanResult<BoardReceipt> {
        admit(grant)?;
        let mut g = self.inner.lock().expect("poisoned");
        let key = (peer.to_string(), board_id.to_string());
        let board = g
            .boards
            .get_mut(&key)
            .ok_or_else(|| KanbanError::UnknownBoard {
                peer: peer.into(),
                board_id: board_id.into(),
            })?;
        // Take the card out of `from_column`.
        let removed = {
            let col = board
                .columns
                .iter_mut()
                .find(|c| c.name == from_column)
                .ok_or_else(|| KanbanError::UnknownColumn(from_column.into()))?;
            let idx = col
                .cards
                .iter()
                .position(|c| c.id == card)
                .ok_or(KanbanError::UnknownCard(card.0))?;
            col.cards.remove(idx)
        };
        let to = board
            .columns
            .iter_mut()
            .find(|c| c.name == to_column)
            .ok_or_else(|| KanbanError::UnknownColumn(to_column.into()))?;
        to.cards.push(removed);
        Ok(BoardReceipt {
            op: ReceiptOp::MoveCard,
            board: board_id.into(),
            card: Some(card),
            from: Some(from_column.into()),
            to: Some(to_column.into()),
            timestamp: now,
        })
    }

    /// Delete a card from any column. Cap-gated.
    pub fn delete_card(
        &self,
        grant: CapToken,
        peer: &str,
        board_id: &str,
        card: CardId,
        now: i64,
    ) -> KanbanResult<BoardReceipt> {
        admit(grant)?;
        let mut g = self.inner.lock().expect("poisoned");
        let key = (peer.to_string(), board_id.to_string());
        let board = g
            .boards
            .get_mut(&key)
            .ok_or_else(|| KanbanError::UnknownBoard {
                peer: peer.into(),
                board_id: board_id.into(),
            })?;
        for col in &mut board.columns {
            if let Some(idx) = col.cards.iter().position(|c| c.id == card) {
                col.cards.remove(idx);
                return Ok(BoardReceipt {
                    op: ReceiptOp::DeleteCard,
                    board: board_id.into(),
                    card: Some(card),
                    from: Some(col.name.clone()),
                    to: None,
                    timestamp: now,
                });
            }
        }
        Err(KanbanError::UnknownCard(card.0))
    }

    /// Fetch a board snapshot.
    pub fn get_board(&self, peer: &str, board_id: &str) -> Option<Board> {
        let g = self.inner.lock().expect("poisoned");
        g.boards
            .get(&(peer.to_string(), board_id.to_string()))
            .cloned()
    }

    /// List every board for a peer.
    pub fn list_boards(&self, peer: &str) -> Vec<Board> {
        let g = self.inner.lock().expect("poisoned");
        g.boards
            .values()
            .filter(|b| b.peer == peer)
            .cloned()
            .collect()
    }

    /// Drop a board.
    pub fn delete_board(
        &self,
        grant: CapToken,
        peer: &str,
        board_id: &str,
        now: i64,
    ) -> KanbanResult<BoardReceipt> {
        admit(grant)?;
        let mut g = self.inner.lock().expect("poisoned");
        let key = (peer.to_string(), board_id.to_string());
        g.boards
            .remove(&key)
            .ok_or_else(|| KanbanError::UnknownBoard {
                peer: peer.into(),
                board_id: board_id.into(),
            })?;
        Ok(BoardReceipt {
            op: ReceiptOp::DeleteBoard,
            board: board_id.into(),
            card: None,
            from: None,
            to: None,
            timestamp: now,
        })
    }
}

/// The Kanban write cap aliases to `MEMORY_READ` for now (mirrors
/// the `todo:write` parser entry). A dedicated bit lands when
/// operators need finer separation.
fn admit(grant: CapToken) -> KanbanResult<()> {
    if grant.contains(CapToken::MEMORY_READ) {
        Ok(())
    } else {
        Err(KanbanError::AdmitRefused {
            required: CapToken::MEMORY_READ.bits(),
            grant: grant.bits(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> KanbanStore {
        KanbanStore::new()
    }

    #[test]
    fn create_board_lays_out_default_columns() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "alice", "b1", 100)
            .unwrap();
        let board = s.get_board("alice", "b1").unwrap();
        let names: Vec<&str> = board.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["todo", "doing", "done"]);
        assert_eq!(board.created_at, 100);
    }

    #[test]
    fn create_board_refuses_without_cap() {
        let s = store();
        let err = s.create_board(CapToken::BOTTOM, "x", "b", 0).unwrap_err();
        assert!(matches!(err, KanbanError::AdmitRefused { .. }));
    }

    #[test]
    fn add_card_returns_monotonic_ids() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "x", "b", 0).unwrap();
        let (_, a) = s
            .add_card(CapToken::MEMORY_READ, "x", "b", "todo", "ta", "", vec![], 1)
            .unwrap();
        let (_, b) = s
            .add_card(CapToken::MEMORY_READ, "x", "b", "todo", "tb", "", vec![], 2)
            .unwrap();
        assert!(b.0 > a.0);
    }

    #[test]
    fn move_card_relocates_between_columns() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "x", "b", 0).unwrap();
        let (_, id) = s
            .add_card(
                CapToken::MEMORY_READ,
                "x",
                "b",
                "todo",
                "t",
                "body",
                vec![],
                1,
            )
            .unwrap();
        let receipt = s
            .move_card(CapToken::MEMORY_READ, "x", "b", "todo", "doing", id, 2)
            .unwrap();
        assert_eq!(receipt.op, ReceiptOp::MoveCard);
        let board = s.get_board("x", "b").unwrap();
        // todo is empty, doing has the card.
        assert!(board.columns[0].cards.is_empty());
        assert_eq!(board.columns[1].cards.len(), 1);
        assert_eq!(board.columns[1].cards[0].id, id);
    }

    #[test]
    fn delete_card_removes_from_any_column() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "x", "b", 0).unwrap();
        let (_, id) = s
            .add_card(CapToken::MEMORY_READ, "x", "b", "doing", "t", "", vec![], 1)
            .unwrap();
        let receipt = s
            .delete_card(CapToken::MEMORY_READ, "x", "b", id, 2)
            .unwrap();
        assert_eq!(receipt.op, ReceiptOp::DeleteCard);
        assert_eq!(receipt.from.as_deref(), Some("doing"));
        let board = s.get_board("x", "b").unwrap();
        for col in &board.columns {
            assert!(col.cards.is_empty());
        }
    }

    #[test]
    fn delete_unknown_card_returns_error() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "x", "b", 0).unwrap();
        let err = s
            .delete_card(CapToken::MEMORY_READ, "x", "b", CardId(999), 1)
            .unwrap_err();
        assert!(matches!(err, KanbanError::UnknownCard(_)));
    }

    #[test]
    fn move_to_unknown_column_returns_error() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "x", "b", 0).unwrap();
        let (_, id) = s
            .add_card(CapToken::MEMORY_READ, "x", "b", "todo", "t", "", vec![], 1)
            .unwrap();
        let err = s
            .move_card(CapToken::MEMORY_READ, "x", "b", "todo", "fnord", id, 2)
            .unwrap_err();
        assert!(matches!(err, KanbanError::UnknownColumn(_)));
    }

    #[test]
    fn add_card_to_unknown_board_returns_error() {
        let s = store();
        let err = s
            .add_card(
                CapToken::MEMORY_READ,
                "x",
                "no-such-board",
                "todo",
                "t",
                "",
                vec![],
                0,
            )
            .unwrap_err();
        assert!(matches!(err, KanbanError::UnknownBoard { .. }));
    }

    #[test]
    fn list_boards_filters_by_peer() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "alice", "b1", 0)
            .unwrap();
        s.create_board(CapToken::MEMORY_READ, "alice", "b2", 0)
            .unwrap();
        s.create_board(CapToken::MEMORY_READ, "bob", "b1", 0)
            .unwrap();
        let alice = s.list_boards("alice");
        let bob = s.list_boards("bob");
        assert_eq!(alice.len(), 2);
        assert_eq!(bob.len(), 1);
    }

    #[test]
    fn delete_board_drops_it() {
        let s = store();
        s.create_board(CapToken::MEMORY_READ, "x", "b", 0).unwrap();
        assert_eq!(s.board_count(), 1);
        s.delete_board(CapToken::MEMORY_READ, "x", "b", 1).unwrap();
        assert_eq!(s.board_count(), 0);
    }

    #[test]
    fn delete_unknown_board_returns_error() {
        let s = store();
        let err = s
            .delete_board(CapToken::MEMORY_READ, "x", "missing", 0)
            .unwrap_err();
        assert!(matches!(err, KanbanError::UnknownBoard { .. }));
    }

    #[test]
    fn write_ops_refuse_without_cap() {
        let s = store();
        // create_board with no cap refuses.
        assert!(s.create_board(CapToken::BOTTOM, "x", "b", 0).is_err());
        // Create with cap so we can try the other ops too.
        s.create_board(CapToken::MEMORY_READ, "x", "b", 0).unwrap();
        assert!(s
            .add_card(CapToken::BOTTOM, "x", "b", "todo", "t", "", vec![], 1)
            .is_err());
        assert!(s
            .move_card(CapToken::BOTTOM, "x", "b", "todo", "doing", CardId(1), 1)
            .is_err());
        assert!(s
            .delete_card(CapToken::BOTTOM, "x", "b", CardId(1), 1)
            .is_err());
        assert!(s.delete_board(CapToken::BOTTOM, "x", "b", 1).is_err());
    }
}
