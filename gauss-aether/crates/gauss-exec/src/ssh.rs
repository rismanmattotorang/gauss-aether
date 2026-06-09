//! SSH executor (Sprint 6 §1.2).
//!
//! Runs requests on a remote host by shelling out to the `ssh` CLI.
//! Cap-gated by [`gauss_core::CapToken::EXECUTOR_SSH`].
//!
//! ## Hermes-superiority axes
//!
//! - **Strict host-key checking.** `StrictHostKeyChecking=yes` is the
//!   default; Hermes ships `accept-new`, which silently trusts
//!   first-time hosts. Operators that explicitly opt into TOFU set
//!   `allow_first_use = true`.
//! - **No agent forwarding.** `ForwardAgent=no` always — a compromised
//!   remote host can't pivot back through the local agent. Hermes
//!   inherits the operator's `~/.ssh/config`.
//! - **No X11 forwarding.** `ForwardX11=no` always.
//! - **Connection-time bounded.** `ConnectTimeout` is set; a hung DNS
//!   resolution can't stall the agent loop.

use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{
    Backend, ExecError, ExecOutput, ExecRequest, ExecResult, Receipt, SessionExecutor,
};

/// SSH executor configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshConfig {
    /// Remote target — `user@host` or `host`.
    pub target: String,
    /// Optional non-default port.
    #[serde(default)]
    pub port: Option<u16>,
    /// Identity file (`-i …`) — explicit private-key path.
    #[serde(default)]
    pub identity_file: Option<String>,
    /// Allow first-use trust (`StrictHostKeyChecking=accept-new`).
    /// Off by default; we refuse unknown hosts so the operator has to
    /// curate the `known_hosts` file once.
    #[serde(default)]
    pub allow_first_use: bool,
    /// Connect timeout in seconds.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: u32,
    /// Path to the `ssh` binary.
    #[serde(default)]
    pub ssh_bin: Option<String>,
}

const fn default_connect_timeout() -> u32 {
    10
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            target: "user@localhost".into(),
            port: None,
            identity_file: None,
            allow_first_use: false,
            connect_timeout_seconds: default_connect_timeout(),
            ssh_bin: None,
        }
    }
}

impl SshConfig {
    fn ssh_bin(&self) -> &str {
        self.ssh_bin.as_deref().unwrap_or("ssh")
    }
}

/// SSH executor. Cheap to clone.
#[derive(Debug, Clone)]
pub struct SshExecutor {
    config: SshConfig,
}

impl SshExecutor {
    /// Build an executor over a configured target.
    #[must_use]
    pub fn new(config: SshConfig) -> Self {
        Self { config }
    }

    /// Borrow the config.
    #[must_use]
    pub fn config(&self) -> &SshConfig {
        &self.config
    }
}

/// Compose the `ssh` argv. Public for snapshot testing.
#[must_use]
pub fn build_ssh_argv(config: &SshConfig, request: &ExecRequest) -> Vec<String> {
    let mut argv: Vec<String> = Vec::with_capacity(16);
    argv.push("-o".into());
    if config.allow_first_use {
        argv.push("StrictHostKeyChecking=accept-new".into());
    } else {
        argv.push("StrictHostKeyChecking=yes".into());
    }
    argv.push("-o".into());
    argv.push("ForwardAgent=no".into());
    argv.push("-o".into());
    argv.push("ForwardX11=no".into());
    argv.push("-o".into());
    argv.push(format!("ConnectTimeout={}", config.connect_timeout_seconds));
    argv.push("-o".into());
    argv.push("BatchMode=yes".into());
    if let Some(port) = config.port {
        argv.push("-p".into());
        argv.push(port.to_string());
    }
    if let Some(path) = &config.identity_file {
        argv.push("-i".into());
        argv.push(path.clone());
    }
    argv.push(config.target.clone());
    // Compose the remote command line. We do NOT do shell escaping
    // here — the operator must pre-sanitise their argv. Pass the
    // request env explicitly via `env KEY=VALUE … program args`.
    let mut remote: Vec<String> = Vec::new();
    if let Some(cwd) = &request.cwd {
        remote.push(format!("cd {} && ", shell_quote(&cwd.to_string_lossy())));
    }
    if !request.env.is_empty() {
        remote.push("env".into());
        for (k, v) in &request.env {
            remote.push(format!("{k}={}", shell_quote(v)));
        }
    }
    remote.push(shell_quote(&request.program));
    for a in &request.args {
        remote.push(shell_quote(a));
    }
    argv.push(remote.join(" "));
    argv
}

/// Quote a string for safe inclusion in a remote shell command line.
/// Wraps in single quotes and escapes embedded single quotes.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[async_trait]
impl SessionExecutor for SshExecutor {
    fn backend(&self) -> Backend {
        Backend::Ssh
    }

    async fn exec(&self, request: ExecRequest) -> ExecResult<(ExecOutput, Receipt)> {
        let argv_len = u32::try_from(request.args.len()).unwrap_or(u32::MAX);
        let argv = build_ssh_argv(&self.config, &request);
        let mut cmd = tokio::process::Command::new(self.config.ssh_bin());
        cmd.args(&argv);
        cmd.env_clear();
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| ExecError::Spawn(format!("ssh spawn failed: {e}")))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let cap = request.max_output_bytes.unwrap_or(usize::MAX);

        let stdout_task = tokio::spawn(crate::local::read_capped_pub(stdout, cap));
        let stderr_task = tokio::spawn(crate::local::read_capped_pub(stderr, cap));
        let status = crate::local::wait_with_timeout(&mut child, request.timeout).await?;
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
            backend: Backend::Ssh,
        };
        let receipt = Receipt {
            backend: Backend::Ssh,
            program: request.program,
            argv_len,
            exit_code,
            truncated,
            timestamp: now_unix(),
        };
        Ok((output, receipt))
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

    fn req(program: &str, args: &[&str]) -> ExecRequest {
        ExecRequest::new(program, args.iter().map(|s| (*s).to_string()).collect())
    }

    #[test]
    fn argv_default_uses_strict_host_key_checking_yes() {
        let argv = build_ssh_argv(&SshConfig::default(), &req("echo", &["ok"]));
        // The "-o" + "StrictHostKeyChecking=yes" pair appears.
        let stricts: Vec<&String> = argv
            .iter()
            .filter(|s| s.contains("StrictHostKeyChecking="))
            .collect();
        assert_eq!(stricts.len(), 1);
        assert_eq!(stricts[0], "StrictHostKeyChecking=yes");
    }

    #[test]
    fn argv_opt_in_first_use_uses_accept_new() {
        let c = SshConfig {
            allow_first_use: true,
            ..SshConfig::default()
        };
        let argv = build_ssh_argv(&c, &req("echo", &["ok"]));
        assert!(argv.iter().any(|s| s == "StrictHostKeyChecking=accept-new"));
    }

    #[test]
    fn argv_always_disables_agent_and_x11_forwarding() {
        let argv = build_ssh_argv(&SshConfig::default(), &req("echo", &["ok"]));
        assert!(argv.iter().any(|s| s == "ForwardAgent=no"));
        assert!(argv.iter().any(|s| s == "ForwardX11=no"));
    }

    #[test]
    fn argv_uses_batch_mode_and_connect_timeout() {
        let argv = build_ssh_argv(&SshConfig::default(), &req("echo", &["ok"]));
        assert!(argv.iter().any(|s| s == "BatchMode=yes"));
        assert!(argv.iter().any(|s| s.starts_with("ConnectTimeout=")));
    }

    #[test]
    fn argv_passes_identity_file_when_set() {
        let c = SshConfig {
            identity_file: Some("/keys/id_ed25519".into()),
            ..SshConfig::default()
        };
        let argv = build_ssh_argv(&c, &req("echo", &["ok"]));
        let i = argv.iter().position(|s| s == "-i").expect("identity flag");
        assert_eq!(argv[i + 1], "/keys/id_ed25519");
    }

    #[test]
    fn argv_passes_port_when_set() {
        let c = SshConfig {
            port: Some(2222),
            ..SshConfig::default()
        };
        let argv = build_ssh_argv(&c, &req("echo", &["ok"]));
        let i = argv.iter().position(|s| s == "-p").expect("port flag");
        assert_eq!(argv[i + 1], "2222");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quote() {
        assert_eq!(shell_quote("don't"), "'don'\\''t'");
        assert_eq!(shell_quote("abc"), "'abc'");
    }

    #[test]
    fn remote_command_includes_quoted_program() {
        let argv = build_ssh_argv(&SshConfig::default(), &req("echo", &["hello world"]));
        let remote = argv.last().expect("remote line");
        assert!(remote.contains("'echo'"));
        assert!(remote.contains("'hello world'"));
    }

    #[test]
    fn remote_command_chains_cwd_with_env() {
        let mut r = req("ls", &[]);
        r.cwd = Some(std::path::PathBuf::from("/srv/app"));
        r.env.insert("KEY".into(), "value".into());
        let argv = build_ssh_argv(&SshConfig::default(), &r);
        let remote = argv.last().unwrap();
        assert!(remote.contains("cd '/srv/app'"));
        assert!(remote.contains("env"));
        assert!(remote.contains("KEY='value'"));
    }
}
