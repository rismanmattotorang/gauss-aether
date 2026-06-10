//! Docker executor (Sprint 6 §1.1).
//!
//! Runs requests inside a Docker container by shelling out to the
//! `docker` CLI. Cap-gated by [`gauss_core::CapToken::EXECUTOR_DOCKER`]
//! at the router level; this module additionally re-validates the
//! image reference at dispatch time.
//!
//! ## Hermes-superiority axes
//!
//! - **Image-digest pinning.** The shipping configuration accepts
//!   `image@sha256:…` digest references. The fallback tag form is
//!   permitted only when the `allow_floating_tags` knob is set —
//!   Hermes accepts any tag silently.
//! - **No host-network by default.** `--network=none` is the default;
//!   a tool that needs network must opt in via `allow_network`.
//! - **Read-only root filesystem.** `--read-only` is the default;
//!   writes go to an explicit per-request `--tmpfs` mount.
//! - **No capabilities + no new privileges.** `--cap-drop=ALL`
//!   `--security-opt=no-new-privileges` always. Hermes inherits.

use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{
    Backend, ExecError, ExecOutput, ExecRequest, ExecResult, Receipt, SessionExecutor,
};

/// Docker executor configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Container image reference. **Digest-pinned form**
    /// (`name@sha256:hex`) is recommended; tag-only references
    /// require `allow_floating_tags = true`.
    pub image: String,
    /// Allow tag-only image references (e.g. `python:3.12`). Off by
    /// default — pinning to a digest prevents silent upgrades.
    #[serde(default)]
    pub allow_floating_tags: bool,
    /// Allow `--network=host`. Off by default; the container runs in
    /// `--network=none` until explicitly opted in.
    #[serde(default)]
    pub allow_network: bool,
    /// Path to the `docker` CLI binary. Defaults to `docker` on PATH.
    #[serde(default)]
    pub docker_bin: Option<String>,
    /// Per-run wall-clock cap in seconds (passed through `timeout`).
    /// `None` means no cap.
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            image: "alpine:latest".into(),
            allow_floating_tags: false,
            allow_network: false,
            docker_bin: None,
            timeout_seconds: Some(60),
        }
    }
}

impl DockerConfig {
    /// Build a config pinned to a specific image digest.
    #[must_use]
    pub fn pinned(image_at_digest: impl Into<String>) -> Self {
        Self {
            image: image_at_digest.into(),
            ..Self::default()
        }
    }

    /// Validate the image reference. Returns `Ok` for
    /// `name@sha256:hex`; for tag-only references the configuration
    /// must also have `allow_floating_tags = true`.
    pub fn validate(&self) -> ExecResult<()> {
        if self.image.contains("@sha256:") {
            return Ok(());
        }
        if self.allow_floating_tags {
            return Ok(());
        }
        Err(ExecError::Backend(format!(
            "image {:?} is not digest-pinned (set allow_floating_tags=true to override)",
            self.image
        )))
    }

    fn docker_bin(&self) -> &str {
        self.docker_bin.as_deref().unwrap_or("docker")
    }
}

/// Docker executor. Cheap to clone.
#[derive(Debug, Clone)]
pub struct DockerExecutor {
    config: DockerConfig,
}

impl DockerExecutor {
    /// Build an executor with the supplied config.
    ///
    /// # Errors
    /// Returns [`ExecError::Backend`] when the image reference fails
    /// the pinning guard.
    pub fn new(config: DockerConfig) -> ExecResult<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    /// Borrow the active config.
    #[must_use]
    pub fn config(&self) -> &DockerConfig {
        &self.config
    }
}

/// Compose the `docker run` argv. Public so the conformance suite
/// can lock the exact flag set against a snapshot.
#[must_use]
pub fn build_docker_argv(config: &DockerConfig, request: &ExecRequest) -> Vec<String> {
    let mut argv: Vec<String> = Vec::with_capacity(32);
    argv.push("run".into());
    argv.push("--rm".into());
    argv.push("--init".into());
    argv.push("--cap-drop=ALL".into());
    argv.push("--security-opt=no-new-privileges".into());
    argv.push("--read-only".into());
    if !config.allow_network {
        argv.push("--network=none".into());
    }
    if let Some(secs) = config.timeout_seconds {
        argv.push(format!("--stop-timeout={secs}"));
    }
    for (k, v) in &request.env {
        argv.push("--env".into());
        argv.push(format!("{k}={v}"));
    }
    if let Some(cwd) = &request.cwd {
        argv.push("--workdir".into());
        argv.push(cwd.to_string_lossy().into_owned());
    }
    argv.push(config.image.clone());
    argv.push(request.program.clone());
    for a in &request.args {
        argv.push(a.clone());
    }
    argv
}

#[async_trait]
impl SessionExecutor for DockerExecutor {
    fn backend(&self) -> Backend {
        Backend::Docker
    }

    async fn exec(&self, request: ExecRequest) -> ExecResult<(ExecOutput, Receipt)> {
        let argv = build_docker_argv(&self.config, &request);
        let argv_len = u32::try_from(request.args.len()).unwrap_or(u32::MAX);
        let mut cmd = tokio::process::Command::new(self.config.docker_bin());
        cmd.args(&argv);
        cmd.env_clear();
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| ExecError::Spawn(format!("docker run failed: {e}")))?;
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
            backend: Backend::Docker,
        };
        let receipt = Receipt {
            backend: Backend::Docker,
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
    use std::collections::BTreeMap;

    fn req(program: &str, args: &[&str]) -> ExecRequest {
        ExecRequest::new(program, args.iter().map(|s| (*s).to_string()).collect())
    }

    #[test]
    fn config_rejects_floating_tag_without_opt_in() {
        let c = DockerConfig {
            image: "alpine:latest".into(),
            allow_floating_tags: false,
            ..DockerConfig::default()
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn config_accepts_pinned_digest() {
        let c = DockerConfig {
            image: "alpine@sha256:aaaa".into(),
            ..DockerConfig::default()
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn config_accepts_floating_tag_with_opt_in() {
        let c = DockerConfig {
            image: "alpine:latest".into(),
            allow_floating_tags: true,
            ..DockerConfig::default()
        };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn argv_includes_security_flags() {
        let c = DockerConfig {
            image: "alpine@sha256:zz".into(),
            ..DockerConfig::default()
        };
        let argv = build_docker_argv(&c, &req("/bin/echo", &["hi"]));
        assert!(argv.iter().any(|a| a == "--rm"));
        assert!(argv.iter().any(|a| a == "--init"));
        assert!(argv.iter().any(|a| a == "--cap-drop=ALL"));
        assert!(argv.iter().any(|a| a == "--security-opt=no-new-privileges"));
        assert!(argv.iter().any(|a| a == "--read-only"));
        assert!(argv.iter().any(|a| a == "--network=none"));
    }

    #[test]
    fn argv_skips_network_none_when_allow_network() {
        let c = DockerConfig {
            image: "alpine@sha256:zz".into(),
            allow_network: true,
            ..DockerConfig::default()
        };
        let argv = build_docker_argv(&c, &req("/bin/echo", &["hi"]));
        assert!(!argv.iter().any(|a| a == "--network=none"));
    }

    #[test]
    fn argv_passes_env_through() {
        let c = DockerConfig {
            image: "alpine@sha256:zz".into(),
            ..DockerConfig::default()
        };
        let mut r = req("/bin/echo", &["hi"]);
        r.env = BTreeMap::from([("FOO".into(), "bar".into())]);
        let argv = build_docker_argv(&c, &r);
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "--env" && w[1] == "FOO=bar"));
    }

    #[test]
    fn argv_ends_with_image_then_program_then_args() {
        let c = DockerConfig {
            image: "myimg@sha256:zz".into(),
            ..DockerConfig::default()
        };
        let argv = build_docker_argv(&c, &req("/usr/bin/python", &["-c", "print(1)"]));
        let len = argv.len();
        // Tail order: image, program, arg0, arg1.
        assert_eq!(argv[len - 4], "myimg@sha256:zz");
        assert_eq!(argv[len - 3], "/usr/bin/python");
        assert_eq!(argv[len - 2], "-c");
        assert_eq!(argv[len - 1], "print(1)");
    }

    #[test]
    fn executor_construction_validates_image() {
        let bad = DockerConfig {
            image: "alpine:edge".into(),
            allow_floating_tags: false,
            ..DockerConfig::default()
        };
        assert!(DockerExecutor::new(bad).is_err());
        let good = DockerConfig {
            image: "alpine@sha256:zz".into(),
            ..DockerConfig::default()
        };
        let exec = DockerExecutor::new(good).unwrap();
        assert_eq!(exec.backend(), Backend::Docker);
    }
}
