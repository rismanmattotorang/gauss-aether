//! L3a — Linux user namespaces via bubblewrap (`bwrap`).
//!
//! Sprint 9 of "Wire the Loop". The layer now actually runs the
//! request's command line inside a deny-by-default `bwrap` confinement
//! when [`SandboxRequest::argv`] is set:
//!
//! - Fresh user / PID / IPC / cgroup / UTS namespaces.
//! - Empty `/` (tmpfs) — only the explicit bind-mounts are visible.
//! - Read-only bind mounts of `/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`,
//!   plus any caller-requested paths from [`SandboxRequest::bind_ro`].
//! - Fresh `procfs` at `/proc` and `devtmpfs` at `/dev`.
//! - **No network namespace inherited** — `--unshare-net` cuts the
//!   subprocess off from the host stack entirely. Tools that need
//!   network must declare a network capability the kernel admits
//!   *before* the sandbox runs.
//! - Cleared environment (`--clearenv`) so host secrets in
//!   `$ANTHROPIC_API_KEY` etc. never leak across the boundary.
//! - Hard kill on parent exit (`--die-with-parent`).
//!
//! When [`SandboxRequest::argv`] is `None`, the layer keeps its
//! historical probe-only behaviour: invoke `bwrap --version` to
//! confirm the binary is reachable, fail loudly otherwise, succeed
//! with an empty stdout otherwise. The L1 + L2 layers (which don't
//! spawn a subprocess) feed L3 a None-argv request to exercise the
//! cap → class mapping check without actually fork-execing.

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult};
use gauss_traits::{SandboxClass, SandboxLayer, SandboxOutcome, SandboxRequest, SandboxTrait};
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

/// Default read-only bind mounts. Every L3 invocation gets these so
/// the subprocess can resolve a typical Linux binary's dynamic libs
/// without an explicit request.
const DEFAULT_BIND_RO: &[&str] = &["/usr", "/lib", "/lib64", "/bin", "/sbin"];

/// Bubblewrap layer.
#[derive(Debug, Clone, Default)]
pub struct BwrapSandbox {
    /// Optional override for the `bwrap` binary location.
    binary: Option<String>,
}

impl BwrapSandbox {
    /// Build a default `bwrap` wrapper looked up on `$PATH`.
    #[must_use]
    pub const fn new() -> Self {
        Self { binary: None }
    }

    /// Override the binary path (useful in tests / hermetic envs).
    #[must_use]
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: Some(binary.into()),
        }
    }

    fn binary_path(&self) -> &str {
        self.binary.as_deref().unwrap_or("bwrap")
    }
}

/// Construct the bubblewrap argv for `request`. Pure function — the
/// returned slice is independent of any subprocess-spawning machinery
/// so the deny-by-default profile stays under test even on hosts
/// without `bwrap` installed (CI, macOS, hermetic builds).
///
/// Returns the argv to pass to `bwrap`; the caller prepends the binary
/// path. Returns `None` when the request has no `argv` — that's the
/// probe-only path.
#[must_use]
pub fn build_bwrap_argv(request: &SandboxRequest) -> Option<Vec<String>> {
    let argv = request.argv.as_ref()?;
    let mut out: Vec<String> = Vec::with_capacity(32 + argv.len());

    // ── Namespace isolation ───────────────────────────────────────────
    // Every "unshare" flag turns one host resource off. The combined
    // set leaves the subprocess in a fresh user / PID / IPC / cgroup /
    // UTS world. `--unshare-net` cuts the network stack.
    out.push("--unshare-user".into());
    out.push("--unshare-ipc".into());
    out.push("--unshare-pid".into());
    out.push("--unshare-uts".into());
    out.push("--unshare-cgroup-try".into());
    out.push("--unshare-net".into());

    // ── Lifecycle ─────────────────────────────────────────────────────
    out.push("--die-with-parent".into());
    out.push("--new-session".into());

    // ── Environment / IDs ─────────────────────────────────────────────
    // Strip the host environment so secrets don't leak; nobody:nogroup
    // (65534) so the subprocess never appears as a privileged uid.
    out.push("--clearenv".into());
    out.push("--uid".into());
    out.push("65534".into());
    out.push("--gid".into());
    out.push("65534".into());

    // ── Filesystem ────────────────────────────────────────────────────
    // Empty root, then bind in the standard read-only set so dynamic
    // linkers + libc + coreutils binaries resolve. Mount /proc + /dev
    // fresh inside the sandbox.
    out.push("--proc".into());
    out.push("/proc".into());
    out.push("--dev".into());
    out.push("/dev".into());
    out.push("--tmpfs".into());
    out.push("/tmp".into());

    for &mount in DEFAULT_BIND_RO {
        out.push("--ro-bind-try".into());
        out.push(mount.into());
        out.push(mount.into());
    }
    for extra in &request.bind_ro {
        // Refuse relative paths — bwrap will refuse them too, but a
        // clearer message at argv-build time avoids a confusing
        // upstream error.
        if !extra.starts_with('/') {
            // Skip rather than panic; the build is a pure function and
            // a malformed mount shouldn't kill the whole sandbox.
            // Operators see the bind list in the request anyway.
            continue;
        }
        out.push("--ro-bind-try".into());
        out.push(extra.clone());
        out.push(extra.clone());
    }

    // ── Program ───────────────────────────────────────────────────────
    out.push("--".into());
    for arg in argv {
        out.push(arg.clone());
    }
    Some(out)
}

#[async_trait]
impl SandboxTrait for BwrapSandbox {
    fn class(&self, _cap: CapToken) -> SandboxClass {
        SandboxClass::L3
    }

    async fn exec(&self, request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        match build_bwrap_argv(&request) {
            // Argv-present path: real subprocess confinement.
            Some(argv) => {
                let mut child = Command::new(self.binary_path())
                    .args(&argv)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| {
                        GaussError::Io(format!("bwrap spawn at '{}': {e}", self.binary_path()))
                    })?;
                // Feed stdin (caller's bytes) and then drop the writer
                // so the child sees EOF.
                if !request.stdin.is_empty() {
                    if let Some(mut sin) = child.stdin.take() {
                        sin.write_all(&request.stdin)
                            .await
                            .map_err(|e| GaussError::Io(format!("bwrap stdin: {e}")))?;
                        drop(sin);
                    }
                }
                let output = child
                    .wait_with_output()
                    .await
                    .map_err(|e| GaussError::Io(format!("bwrap wait: {e}")))?;
                let code = output.status.code().unwrap_or(-1);
                Ok(SandboxOutcome::new(
                    output.stdout,
                    vec![SandboxLayer::Namespace],
                    code,
                ))
            }
            // Argv-absent path: probe-only — keeps the legacy
            // "fail loudly when bwrap is missing" contract that the
            // composite executor relies on for cap-class refusal.
            None => {
                let status = Command::new(self.binary_path())
                    .arg("--version")
                    .output()
                    .await;
                match status {
                    Ok(out) if out.status.success() => Ok(SandboxOutcome::ok(
                        Vec::new(),
                        vec![SandboxLayer::Namespace],
                    )),
                    Ok(out) => Err(GaussError::Io(format!(
                        "bwrap --version failed: status={:?} stderr={}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr),
                    ))),
                    Err(e) => Err(GaussError::Io(format!(
                        "bwrap not available at '{}': {e}",
                        self.binary_path()
                    ))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;

    fn bwrap_available() -> bool {
        std::process::Command::new("bwrap")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn missing_binary_yields_clear_io_error() {
        let sb = BwrapSandbox::with_binary("/no/such/binary/bwrap");
        let err = sb
            .exec(SandboxRequest::new(
                ToolId("ns".into()),
                CapToken::SUBPROCESS_SPAWN,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
            .expect_err("missing binary must error");
        match err {
            GaussError::Io(msg) => assert!(msg.contains("bwrap"), "msg: {msg}"),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn class_is_l3() {
        let sb = BwrapSandbox::default();
        assert_eq!(sb.class(CapToken::SUBPROCESS_SPAWN), SandboxClass::L3);
    }

    // ─── Sprint 9 — deny-by-default argv construction ─────────────────

    #[test]
    fn build_bwrap_argv_returns_none_when_no_argv() {
        let req = SandboxRequest::new(
            ToolId("probe".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        );
        assert!(build_bwrap_argv(&req).is_none());
    }

    #[test]
    fn built_argv_isolates_every_namespace() {
        let req = SandboxRequest::new(
            ToolId("t".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/true".into()]);
        let argv = build_bwrap_argv(&req).expect("argv-present path");
        for required in [
            "--unshare-user",
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-net",
            "--unshare-cgroup-try",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
        ] {
            assert!(
                argv.iter().any(|a| a == required),
                "argv missing {required}: {argv:?}"
            );
        }
    }

    #[test]
    fn built_argv_drops_to_nobody_uid_and_gid() {
        let req = SandboxRequest::new(
            ToolId("t".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/true".into()]);
        let argv = build_bwrap_argv(&req).unwrap();
        // Look for the `--uid 65534` and `--gid 65534` adjacent pairs.
        let positions: Vec<usize> = argv
            .iter()
            .enumerate()
            .filter_map(|(i, a)| if a == "--uid" || a == "--gid" { Some(i) } else { None })
            .collect();
        for i in positions {
            assert_eq!(argv[i + 1], "65534");
        }
    }

    #[test]
    fn built_argv_mounts_standard_ro_paths() {
        let req = SandboxRequest::new(
            ToolId("t".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/true".into()]);
        let argv = build_bwrap_argv(&req).unwrap();
        // Every default mount should appear after a `--ro-bind-try`.
        for &mount in &["/usr", "/lib", "/bin"] {
            let mut found = false;
            for w in argv.windows(3) {
                if w[0] == "--ro-bind-try" && w[1] == mount && w[2] == mount {
                    found = true;
                    break;
                }
            }
            assert!(found, "expected --ro-bind-try {mount} {mount} in {argv:?}");
        }
    }

    #[test]
    fn built_argv_honours_extra_bind_ro() {
        let req = SandboxRequest::new(
            ToolId("t".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/cat".into(), "/data/in".into()])
        .with_bind_ro(vec!["/data".into()]);
        let argv = build_bwrap_argv(&req).unwrap();
        let mut found = false;
        for w in argv.windows(3) {
            if w[0] == "--ro-bind-try" && w[1] == "/data" && w[2] == "/data" {
                found = true;
                break;
            }
        }
        assert!(found, "caller-supplied bind not in argv: {argv:?}");
    }

    #[test]
    fn built_argv_skips_relative_bind_ro_paths() {
        let req = SandboxRequest::new(
            ToolId("t".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/true".into()])
        .with_bind_ro(vec!["relative/path".into(), "/abs/path".into()]);
        let argv = build_bwrap_argv(&req).unwrap();
        // Absolute survives.
        assert!(argv.iter().any(|a| a == "/abs/path"));
        // Relative skipped.
        assert!(!argv.iter().any(|a| a == "relative/path"));
    }

    #[test]
    fn built_argv_terminates_options_before_program() {
        let req = SandboxRequest::new(
            ToolId("t".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/echo".into(), "hi".into()]);
        let argv = build_bwrap_argv(&req).unwrap();
        let dash_dash_pos = argv.iter().position(|a| a == "--").expect("-- terminator");
        // Everything after `--` is the program + args, in order.
        assert_eq!(&argv[dash_dash_pos + 1..], &["/bin/echo", "hi"]);
    }

    // ─── End-to-end tests (gated on bwrap availability) ──────────────

    #[tokio::test]
    async fn argv_present_runs_true_and_captures_exit_code() {
        if !bwrap_available() {
            eprintln!("bwrap not installed — skipping end-to-end test");
            return;
        }
        let sb = BwrapSandbox::default();
        let req = SandboxRequest::new(
            ToolId("e2e".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/true".into()]);
        let out = sb.exec(req).await.expect("exec");
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty());
    }

    #[tokio::test]
    async fn argv_present_captures_stdout_from_echo() {
        if !bwrap_available() {
            eprintln!("bwrap not installed — skipping end-to-end test");
            return;
        }
        let sb = BwrapSandbox::default();
        let req = SandboxRequest::new(
            ToolId("e2e".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec!["/bin/echo".into(), "hello".into()]);
        let out = sb.exec(req).await.expect("exec");
        assert_eq!(out.exit_code, 0);
        let s = String::from_utf8(out.stdout).unwrap();
        assert_eq!(s.trim_end(), "hello");
    }

    #[tokio::test]
    async fn network_namespace_is_unshared_so_dns_fails() {
        if !bwrap_available() {
            eprintln!("bwrap not installed — skipping end-to-end test");
            return;
        }
        // `getent ahosts <name>` opens a socket; with no network
        // namespace it will fail. We assert the exit code is non-zero
        // — which proves the sandbox really cut the network.
        let sb = BwrapSandbox::default();
        let req = SandboxRequest::new(
            ToolId("e2e".into()),
            CapToken::SUBPROCESS_SPAWN,
            serde_json::Value::Null,
            Vec::new(),
        )
        .with_argv(vec![
            "/usr/bin/getent".into(),
            "ahosts".into(),
            "example.com".into(),
        ]);
        let out = sb.exec(req).await.expect("exec");
        assert_ne!(
            out.exit_code, 0,
            "network namespace should make DNS fail inside the sandbox"
        );
    }
}
