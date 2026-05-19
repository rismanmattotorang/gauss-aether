//! `gauss-worktree` — per-session git worktree isolation.
//!
//! Sprint 6 §8 of `/ROADMAP.md`. Hermes runs every concurrent session
//! against the same working directory; a long-running session's
//! `git checkout` can clobber the parent's state, and two sessions
//! editing the same file race silently. This crate ships per-session
//! isolation through `git worktree add`, with three GaussClaw-only
//! properties Hermes can't match:
//!
//! 1. **Cap-gated.** Worktree mutations require
//!    [`gauss_core::CapToken::WORKTREE_WRITE`]; a session that lost
//!    the cap mid-run can't silently spawn a new worktree.
//! 2. **Audit-aware.** Every `create` / `destroy` returns a
//!    [`WorktreeReceipt`] the caller appends to the chain. The
//!    operator can replay the chain to see exactly which worktrees
//!    each session touched.
//! 3. **Deterministic naming.** Worktree directories live under
//!    `<root>/.gaussclaw/worktrees/<session_id>/`, where
//!    `<session_id>` is the operator-supplied id. Two concurrent
//!    sessions with the same id race-fail explicitly rather than
//!    silently sharing state.
//!
//! ## Hermes-superiority axes
//!
//! - **Concurrency by construction.** Every session gets its own
//!   working tree; the parent never sees the session's in-flight
//!   edits unless the session explicitly merges back. Hermes
//!   serialises every git mutation through the parent worktree.
//! - **Branch isolation.** Each session checks out a fresh branch
//!   (`gaussclaw/sessions/<session_id>`); merging back is a
//!   deliberate operator action.
//! - **Cleanup-on-drop.** The [`WorktreeHandle`] kills its worktree
//!   on drop unless the operator calls `WorktreeHandle::keep()`.

#![allow(
    clippy::doc_markdown,
    clippy::missing_const_for_fn,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_docs_in_private_items,
    clippy::too_long_first_doc_paragraph
)]
#![allow(rustdoc::broken_intra_doc_links)]

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gauss_core::CapToken;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Operator-supplied session identifier — must be a clean
/// filesystem-safe slug (`[A-Za-z0-9_-]+`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    /// Build from a `&str` after validating the slug shape.
    ///
    /// # Errors
    /// Returns [`WorktreeError::InvalidSessionId`] when the input
    /// contains characters outside `[A-Za-z0-9_-]` or is empty.
    pub fn new(v: impl Into<String>) -> Result<Self, WorktreeError> {
        let s: String = v.into();
        if s.is_empty() {
            return Err(WorktreeError::InvalidSessionId(s));
        }
        for c in s.chars() {
            if !c.is_ascii_alphanumeric() && c != '_' && c != '-' {
                return Err(WorktreeError::InvalidSessionId(s));
            }
        }
        Ok(Self(s))
    }

    /// Borrow the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Crate-wide error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WorktreeError {
    /// Caller's cap grant didn't include `WORKTREE_WRITE`.
    #[error("admit refused: required cap 0x{required:016x} not in grant 0x{grant:016x}")]
    AdmitRefused {
        /// Bits required.
        required: u64,
        /// Bits the grant exposes.
        grant: u64,
    },
    /// Session id failed the slug guard.
    #[error("invalid session id: {0:?}")]
    InvalidSessionId(String),
    /// `git` binary not on PATH or not a git working tree.
    #[error("git unavailable: {0}")]
    GitUnavailable(String),
    /// A worktree already exists for the requested session id.
    #[error("worktree already exists for session {0:?}")]
    AlreadyExists(SessionId),
    /// No worktree exists with the requested session id.
    #[error("no worktree for session {0:?}")]
    NotFound(SessionId),
    /// I/O failure.
    #[error("io: {0}")]
    Io(String),
    /// Git command returned non-zero.
    #[error("git: {0}")]
    Git(String),
}

impl From<std::io::Error> for WorktreeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Result alias.
pub type WorktreeResult<T> = Result<T, WorktreeError>;

/// Receipt of one worktree operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeReceipt {
    /// Operation.
    pub op: WorktreeOp,
    /// Session id this acted on.
    pub session: SessionId,
    /// Branch the worktree is/was checked out on.
    pub branch: String,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// UNIX seconds.
    pub timestamp: i64,
}

/// Receipt operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorktreeOp {
    /// `create` — added a new worktree.
    Create,
    /// `destroy` — pruned an existing worktree.
    Destroy,
}

/// Live handle to a created worktree. Drop kills the worktree
/// unless [`Self::keep()`] has been called.
#[derive(Debug)]
pub struct WorktreeHandle {
    session: SessionId,
    branch: String,
    path: PathBuf,
    repo_root: PathBuf,
    keep: bool,
    git_bin: String,
}

impl WorktreeHandle {
    /// Absolute path to the worktree directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Branch the worktree is checked out on.
    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Session id.
    #[must_use]
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Suppress the drop-time cleanup. The worktree persists until
    /// the operator runs `git worktree remove`.
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for WorktreeHandle {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        // Best-effort cleanup; we can't await in `drop` so we shell
        // out synchronously via `std::process::Command`. Failures
        // are logged but not surfaced — the handle is going away
        // either way.
        let res = std::process::Command::new(&self.git_bin)
            .args([
                "worktree",
                "remove",
                "--force",
                &self.path.to_string_lossy(),
            ])
            .current_dir(&self.repo_root)
            .output();
        if let Err(e) = res {
            tracing::warn!(
                worktree = %self.path.display(),
                session = %self.session.as_str(),
                error = %e,
                "best-effort worktree cleanup failed"
            );
        }
    }
}

/// Worktree manager. Cheap to clone — holds only a repo root + git binary path.
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    repo_root: PathBuf,
    git_bin: String,
}

impl WorktreeManager {
    /// Build a manager rooted at a git working tree.
    #[must_use]
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            git_bin: "git".into(),
        }
    }

    /// Override the `git` binary path (mainly for testing).
    #[must_use]
    pub fn with_git_bin(mut self, git_bin: impl Into<String>) -> Self {
        self.git_bin = git_bin.into();
        self
    }

    /// Where worktrees for this manager land.
    #[must_use]
    pub fn worktree_root(&self) -> PathBuf {
        self.repo_root.join(".gaussclaw/worktrees")
    }

    /// Compute the canonical path for a session.
    #[must_use]
    pub fn worktree_path(&self, session: &SessionId) -> PathBuf {
        self.worktree_root().join(session.as_str())
    }

    /// Compute the canonical branch name for a session.
    #[must_use]
    pub fn worktree_branch(&self, session: &SessionId) -> String {
        format!("gaussclaw/sessions/{}", session.as_str())
    }

    /// Create a fresh worktree.
    pub async fn create(
        &self,
        grant: CapToken,
        session: SessionId,
    ) -> WorktreeResult<(WorktreeHandle, WorktreeReceipt)> {
        if !grant.contains(CapToken::WORKTREE_WRITE) {
            return Err(WorktreeError::AdmitRefused {
                required: CapToken::WORKTREE_WRITE.bits(),
                grant: grant.bits(),
            });
        }
        self.require_git_repo().await?;
        let path = self.worktree_path(&session);
        let branch = self.worktree_branch(&session);
        if tokio::fs::metadata(&path).await.is_ok() {
            return Err(WorktreeError::AlreadyExists(session));
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // `git worktree add -b <branch> <path>` creates a new branch
        // from HEAD and checks it out into `path`.
        self.run_git(&["worktree", "add", "-b", &branch, &path.to_string_lossy()])
            .await?;
        let handle = WorktreeHandle {
            session: session.clone(),
            branch: branch.clone(),
            path: path.clone(),
            repo_root: self.repo_root.clone(),
            keep: false,
            git_bin: self.git_bin.clone(),
        };
        let receipt = WorktreeReceipt {
            op: WorktreeOp::Create,
            session,
            branch,
            path,
            timestamp: now_unix(),
        };
        Ok((handle, receipt))
    }

    /// Drop an existing worktree. Returns a receipt.
    pub async fn destroy(
        &self,
        grant: CapToken,
        session: SessionId,
    ) -> WorktreeResult<WorktreeReceipt> {
        if !grant.contains(CapToken::WORKTREE_WRITE) {
            return Err(WorktreeError::AdmitRefused {
                required: CapToken::WORKTREE_WRITE.bits(),
                grant: grant.bits(),
            });
        }
        let path = self.worktree_path(&session);
        if tokio::fs::metadata(&path).await.is_err() {
            return Err(WorktreeError::NotFound(session));
        }
        let branch = self.worktree_branch(&session);
        self.run_git(&["worktree", "remove", "--force", &path.to_string_lossy()])
            .await?;
        Ok(WorktreeReceipt {
            op: WorktreeOp::Destroy,
            session,
            branch,
            path,
            timestamp: now_unix(),
        })
    }

    /// List every session that currently has a worktree under this
    /// manager's root.
    pub async fn list_sessions(&self) -> WorktreeResult<Vec<SessionId>> {
        let root = self.worktree_root();
        let mut out: Vec<SessionId> = Vec::new();
        let Ok(mut rd) = tokio::fs::read_dir(&root).await else {
            return Ok(out);
        };
        while let Some(entry) = rd.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(sess) = SessionId::new(name) {
                        out.push(sess);
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    async fn run_git(&self, args: &[&str]) -> WorktreeResult<String> {
        let out = tokio::process::Command::new(&self.git_bin)
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| WorktreeError::GitUnavailable(format!("spawn: {e}")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(WorktreeError::Git(format!("git {args:?}: {stderr}")));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().into())
    }

    async fn require_git_repo(&self) -> WorktreeResult<()> {
        match self.run_git(&["rev-parse", "--is-inside-work-tree"]).await {
            Ok(ans) if ans == "true" => Ok(()),
            Ok(other) => Err(WorktreeError::GitUnavailable(format!(
                "not inside a git work tree: {other}"
            ))),
            // A non-zero exit from `git rev-parse` outside a repo is
            // semantically "git unavailable", not a backend error.
            Err(WorktreeError::Git(detail)) => Err(WorktreeError::GitUnavailable(detail)),
            Err(e) => Err(e),
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init_repo(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
            // Some hosting environments inject a global gpg/code-sign
            // hook via `commit.gpgsign` or `gpg.program`. Force the
            // test repo to use no signing.
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "tag.gpgsign", "false"],
            vec!["config", "gpg.program", "true"],
        ] {
            let out = tokio::process::Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .await
                .expect("git init");
            assert!(out.status.success(), "git {args:?} failed");
        }
        // Need at least one commit for `worktree add` to attach a
        // new branch.
        tokio::fs::write(dir.join("README.md"), b"# test")
            .await
            .unwrap();
        for args in [
            vec!["add", "README.md"],
            vec![
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--no-gpg-sign",
                "-m",
                "init",
            ],
        ] {
            let out = tokio::process::Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .await
                .expect("git");
            assert!(
                out.status.success(),
                "git {args:?} failed: stderr={}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn session_id_accepts_slug() {
        assert!(SessionId::new("abc-123_def").is_ok());
        assert!(SessionId::new("a").is_ok());
    }

    #[test]
    fn session_id_rejects_path_traversal() {
        assert!(SessionId::new("..").is_err());
        assert!(SessionId::new("a/b").is_err());
        assert!(SessionId::new("a.b").is_err());
        assert!(SessionId::new("").is_err());
        assert!(SessionId::new("\n").is_err());
    }

    #[tokio::test]
    async fn create_refuses_without_cap() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let mgr = WorktreeManager::new(dir.path());
        let sess = SessionId::new("a").unwrap();
        let err = mgr.create(CapToken::BOTTOM, sess).await.unwrap_err();
        assert!(matches!(err, WorktreeError::AdmitRefused { .. }));
    }

    #[tokio::test]
    async fn create_then_destroy_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let mgr = WorktreeManager::new(dir.path());
        let sess = SessionId::new("session1").unwrap();
        let (mut handle, receipt) = mgr
            .create(CapToken::WORKTREE_WRITE, sess.clone())
            .await
            .unwrap();
        assert_eq!(receipt.op, WorktreeOp::Create);
        assert!(handle.path().exists());
        assert_eq!(handle.branch(), "gaussclaw/sessions/session1");
        // Tell the handle not to clean up on drop so we can `destroy` explicitly.
        handle.keep();
        let drop_receipt = mgr
            .destroy(CapToken::WORKTREE_WRITE, sess.clone())
            .await
            .unwrap();
        assert_eq!(drop_receipt.op, WorktreeOp::Destroy);
        assert!(!mgr.worktree_path(&sess).exists());
    }

    #[tokio::test]
    async fn create_twice_with_same_session_id_fails_second() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let mgr = WorktreeManager::new(dir.path());
        let sess = SessionId::new("once").unwrap();
        let (mut h, _) = mgr
            .create(CapToken::WORKTREE_WRITE, sess.clone())
            .await
            .unwrap();
        h.keep();
        let err = mgr
            .create(CapToken::WORKTREE_WRITE, sess)
            .await
            .unwrap_err();
        assert!(matches!(err, WorktreeError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn destroy_unknown_returns_not_found() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let mgr = WorktreeManager::new(dir.path());
        let sess = SessionId::new("ghost").unwrap();
        let err = mgr
            .destroy(CapToken::WORKTREE_WRITE, sess)
            .await
            .unwrap_err();
        assert!(matches!(err, WorktreeError::NotFound(_)));
    }

    #[tokio::test]
    async fn list_sessions_reflects_creates() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let mgr = WorktreeManager::new(dir.path());
        for n in ["alpha", "beta"] {
            let sess = SessionId::new(n).unwrap();
            let (mut h, _) = mgr.create(CapToken::WORKTREE_WRITE, sess).await.unwrap();
            h.keep();
        }
        let sessions = mgr.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|s| s.as_str() == "alpha"));
        assert!(sessions.iter().any(|s| s.as_str() == "beta"));
    }

    #[tokio::test]
    async fn drop_cleans_up_by_default() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path()).await;
        let mgr = WorktreeManager::new(dir.path());
        let sess = SessionId::new("ephemeral").unwrap();
        let path = mgr.worktree_path(&sess);
        {
            let (_handle, _) = mgr
                .create(CapToken::WORKTREE_WRITE, sess.clone())
                .await
                .unwrap();
            assert!(path.exists());
        } // handle drops here
          // Give the synchronous cleanup a moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!path.exists(), "worktree should be cleaned up on drop");
    }

    #[tokio::test]
    async fn create_outside_git_repo_fails() {
        let dir = tempfile::tempdir().unwrap(); // no git init
        let mgr = WorktreeManager::new(dir.path());
        let sess = SessionId::new("a").unwrap();
        let err = mgr
            .create(CapToken::WORKTREE_WRITE, sess)
            .await
            .unwrap_err();
        assert!(matches!(err, WorktreeError::GitUnavailable(_)));
    }

    #[test]
    fn worktree_path_uses_session_subdirectory() {
        let mgr = WorktreeManager::new("/repo");
        let sess = SessionId::new("xyz").unwrap();
        let p = mgr.worktree_path(&sess);
        assert!(p.to_string_lossy().ends_with(".gaussclaw/worktrees/xyz"));
    }
}
