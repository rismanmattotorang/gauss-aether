//! Local executor — `tokio::process::Command` wrapper.
//!
//! The default backend; matches the pre-Sprint-6 inline execution
//! behaviour, just lifted through the [`SessionExecutor`] trait so the
//! call site is identical to Docker / SSH / Modal.

use std::time::SystemTime;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;

use crate::types::{
    Backend, ExecError, ExecOutput, ExecRequest, ExecResult, Receipt, SessionExecutor,
};

/// Local executor. Cheap to clone (zero state).
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalExecutor;

impl LocalExecutor {
    /// Build a fresh local executor.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SessionExecutor for LocalExecutor {
    fn backend(&self) -> Backend {
        Backend::Local
    }

    async fn exec(&self, request: ExecRequest) -> ExecResult<(ExecOutput, Receipt)> {
        let argv_len = u32::try_from(request.args.len()).unwrap_or(u32::MAX);
        let mut cmd = tokio::process::Command::new(&request.program);
        cmd.args(&request.args);
        // Clear inherited env then layer the request's env on top —
        // we never leak the host's environment into a session unless
        // the caller explicitly asked for it.
        cmd.env_clear();
        for (k, v) in &request.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &request.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| ExecError::Spawn(format!("{}: {e}", request.program)))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let cap = request.max_output_bytes.unwrap_or(usize::MAX);

        let stdout_task = tokio::spawn(read_capped(stdout, cap));
        let stderr_task = tokio::spawn(read_capped(stderr, cap));
        let status = child.wait().await?;
        let (stdout_buf, stdout_trunc) = stdout_task
            .await
            .map_err(|e| ExecError::Backend(format!("stdout task: {e}")))??;
        let (stderr_buf, stderr_trunc) = stderr_task
            .await
            .map_err(|e| ExecError::Backend(format!("stderr task: {e}")))??;
        let truncated = stdout_trunc || stderr_trunc;
        let exit_code = status.code();

        let output = ExecOutput {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
            truncated,
            backend: Backend::Local,
        };
        let receipt = Receipt {
            backend: Backend::Local,
            program: request.program,
            argv_len,
            exit_code,
            truncated,
            timestamp: now_unix(),
        };
        Ok((output, receipt))
    }
}

async fn read_capped<R: AsyncReadExt + Unpin + Send>(
    reader: Option<R>,
    cap: usize,
) -> ExecResult<(Vec<u8>, bool)> {
    read_capped_pub(reader, cap).await
}

/// Same as `read_capped` but `pub(crate)` so sibling backends can
/// share the implementation.
pub(crate) async fn read_capped_pub<R: AsyncReadExt + Unpin + Send>(
    reader: Option<R>,
    cap: usize,
) -> ExecResult<(Vec<u8>, bool)> {
    let Some(mut r) = reader else {
        return Ok((vec![], false));
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut truncated = false;
    loop {
        let n = r
            .read(&mut chunk)
            .await
            .map_err(|e| ExecError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        if buf.len().saturating_add(n) > cap {
            let remaining = cap.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            // Drain the rest of the reader so the child doesn't
            // block on a full pipe; ignore the data.
            let mut sink = [0u8; 4096];
            while r
                .read(&mut sink)
                .await
                .map_err(|e| ExecError::Io(e.to_string()))?
                > 0
            {}
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok((buf, truncated))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_round_trip() {
        let exec = LocalExecutor::new();
        let req = ExecRequest::new("/bin/sh", vec!["-c".into(), "echo hello-world".into()]);
        let (out, receipt) = exec.exec(req).await.unwrap();
        assert!(out.success());
        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.contains("hello-world"));
        assert!(!out.truncated);
        assert_eq!(receipt.backend, Backend::Local);
    }

    #[tokio::test]
    async fn non_zero_exit_is_surfaced() {
        let exec = LocalExecutor::new();
        let req = ExecRequest::new("/bin/sh", vec!["-c".into(), "exit 7".into()]);
        let (out, _) = exec.exec(req).await.unwrap();
        assert!(!out.success());
        assert_eq!(out.exit_code, Some(7));
    }

    #[tokio::test]
    async fn env_is_isolated_from_host() {
        // Pre-set a host env var; the executor's env_clear should
        // wipe it.
        std::env::set_var("GAUSS_EXEC_TEST_SENTINEL", "host-value");
        let exec = LocalExecutor::new();
        let req = ExecRequest::new(
            "/bin/sh",
            vec![
                "-c".into(),
                "printf '%s' \"${GAUSS_EXEC_TEST_SENTINEL:-(unset)}\"".into(),
            ],
        );
        let (out, _) = exec.exec(req).await.unwrap();
        assert!(out.success());
        assert!(
            out.stdout.contains("(unset)"),
            "stdout was {:?}",
            out.stdout
        );
    }

    #[tokio::test]
    async fn explicit_env_var_is_visible() {
        let exec = LocalExecutor::new();
        let req = ExecRequest::new(
            "/bin/sh",
            vec!["-c".into(), "printf '%s' \"$MY_VAR\"".into()],
        )
        .env("MY_VAR", "explicit");
        let (out, _) = exec.exec(req).await.unwrap();
        assert_eq!(out.stdout, "explicit");
    }

    #[tokio::test]
    async fn cwd_is_respected() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("marker"), b"hi")
            .await
            .unwrap();
        let exec = LocalExecutor::new();
        let req = ExecRequest::new("/bin/sh", vec!["-c".into(), "ls".into()])
            .cwd(dir.path().to_path_buf());
        let (out, _) = exec.exec(req).await.unwrap();
        assert!(out.success());
        assert!(out.stdout.contains("marker"));
    }

    #[tokio::test]
    async fn max_output_truncates_long_runs() {
        let exec = LocalExecutor::new();
        let req = ExecRequest::new(
            "/bin/sh",
            vec!["-c".into(), "yes hello | head -c 8192".into()],
        )
        .max_output(64);
        let (out, _) = exec.exec(req).await.unwrap();
        assert!(out.truncated);
        assert!(out.stdout.len() <= 64);
    }

    #[tokio::test]
    async fn spawn_error_for_unknown_binary() {
        let exec = LocalExecutor::new();
        let req = ExecRequest::new("/does/not/exist", vec![]);
        let err = exec.exec(req).await.unwrap_err();
        assert!(matches!(err, ExecError::Spawn(_)));
    }
}
