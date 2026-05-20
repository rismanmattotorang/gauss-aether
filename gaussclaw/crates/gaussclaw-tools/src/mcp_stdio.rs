//! Stdio transport for the [`McpClient`] trait — JSON-RPC 2.0 over a
//! spawned child process's stdin/stdout.
//!
//! Sprint 10 §9 of `/ROADMAP.md`. Companion to the existing
//! [`crate::mcp_http::HttpMcpClient`]: identical JSON-RPC wire shape,
//! different transport. The MCP reference implementations (Anthropic
//! `mcp-server-*` packages, the Cline / Continue.dev plugins) ship as
//! stdio binaries — this client is the production path for talking to
//! them.
//!
//! ## Wire format
//!
//! LSP-style framing on top of JSON-RPC 2.0:
//!
//! ```text
//! Content-Length: 76\r\n
//! \r\n
//! {"jsonrpc":"2.0","id":1,"method":"tools/list"}
//! ```
//!
//! No `Content-Type` header is required; servers that emit one have it
//! ignored. We accept LF (`\n`) as well as CRLF (`\r\n`) so a Python
//! stub server doesn't trip the parser.
//!
//! ## Testability
//!
//! The struct exposes two constructors: [`StdioMcpClient::spawn`] for
//! the production path (real child process) and
//! [`StdioMcpClient::with_streams`] for tests (any `AsyncWrite` +
//! `AsyncBufRead`). The conformance suite drives the second form with
//! `tokio::io::duplex()` — no subprocess needed to exercise the full
//! request/response path.
//!
//! ## Security posture
//!
//! Identical to the HTTP transport: every response crosses the HWCA
//! schema gate when the bridge returns it, so the IPI-by-MCP vector is
//! closed by construction. The stdio path additionally inherits the
//! parent's stderr — server logs surface in the operator's terminal
//! rather than being silently swallowed.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::ffi::OsStr;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::mcp::{McpClient, McpError, McpToolDescriptor};

// ─── stdio client ──────────────────────────────────────────────────────────

/// JSON-RPC 2.0 MCP client over a child process's stdio. Cheap to
/// `Arc`-wrap; not `Clone` because the underlying streams are owned
/// once.
pub struct StdioMcpClient {
    server_name: String,
    next_id: AtomicU64,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for StdioMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioMcpClient")
            .field("server_name", &self.server_name)
            .field("next_id", &self.next_id.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

struct Inner {
    write: Box<dyn AsyncWrite + Send + Unpin>,
    read: Box<dyn AsyncBufRead + Send + Unpin>,
    /// Kept alive so the child process doesn't get reaped while the
    /// client is live. `None` for the test constructor.
    _child: Option<Child>,
}

impl StdioMcpClient {
    /// Spawn an MCP server as a child process and wire its stdio.
    ///
    /// The child's `stderr` is inherited from the parent, so server
    /// diagnostics surface in the operator's terminal. `stdin` /
    /// `stdout` are piped and framed.
    ///
    /// # Errors
    /// Returns [`McpError::Transport`] when the binary cannot be
    /// spawned (e.g. not found, permission denied) or when the child
    /// does not expose stdin / stdout pipes.
    pub fn spawn<S, I, A>(server_name: S, program: I, args: &[A]) -> Result<Self, McpError>
    where
        S: Into<String>,
        I: AsRef<OsStr>,
        A: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program);
        cmd.args(args.iter().map(AsRef::as_ref))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Transport(format!("spawn: {e}")))?;
        let stdin: ChildStdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdin".into()))?;
        let stdout: ChildStdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdout".into()))?;
        Ok(Self {
            server_name: server_name.into(),
            next_id: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                write: Box::new(stdin),
                read: Box::new(BufReader::new(stdout)),
                _child: Some(child),
            }),
        })
    }

    /// Build a client over caller-supplied async streams. Used by the
    /// conformance suite to drive the full request/response path
    /// against `tokio::io::duplex()` without spawning a subprocess.
    #[must_use]
    pub fn with_streams<W, R>(server_name: impl Into<String>, write: W, read: R) -> Self
    where
        W: AsyncWrite + Send + Unpin + 'static,
        R: AsyncRead + Send + Unpin + 'static,
    {
        Self {
            server_name: server_name.into(),
            next_id: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                write: Box::new(write),
                read: Box::new(BufReader::new(read)),
                _child: None,
            }),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1)
    }

    /// One round-trip: send a request, read the response, unwrap the
    /// `result` field (or surface the `error` field as
    /// [`McpError::Server`]).
    ///
    /// Holds the inner lock across the await so concurrent calls
    /// serialise on the wire — request and response framing always
    /// interleave correctly.
    async fn do_rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let id = self.next_id();
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut guard = self.inner.lock().await;
        write_message(&mut guard.write, &request).await?;
        let response = read_message(&mut guard.read).await?;
        if response.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
            return Err(McpError::Protocol(format!(
                "response id mismatch (expected {id})"
            )));
        }
        if let Some(err) = response.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown server error");
            return Err(McpError::Server(msg.to_owned()));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| McpError::Protocol("response missing both `result` and `error`".into()))
    }
}

#[async_trait]
impl McpClient for StdioMcpClient {
    fn server_name(&self) -> &str {
        &self.server_name
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let result = self.do_rpc("tools/list", serde_json::Value::Null).await?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                McpError::Protocol("tools/list response missing `tools` array".into())
            })?;
        let mut out = Vec::with_capacity(tools.len());
        for t in tools {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::Protocol("tool descriptor missing `name`".into()))?;
            let description = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let input_schema = t
                .get("inputSchema")
                .or_else(|| t.get("input_schema"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let mut descriptor = McpToolDescriptor::new(name, description);
            descriptor.input_schema = input_schema;
            out.push(descriptor);
        }
        Ok(out)
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        self.do_rpc(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": args }),
        )
        .await
    }
}

// ─── framing helpers (LSP-style Content-Length) ────────────────────────────

/// Write one JSON-RPC message with `Content-Length` framing. CRLF
/// line endings match the LSP / MCP convention; servers that emit LF
/// also parse fine on the read side.
async fn write_message<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    message: &serde_json::Value,
) -> Result<(), McpError> {
    let body = serde_json::to_vec(message)
        .map_err(|e| McpError::Protocol(format!("encode request: {e}")))?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer
        .write_all(header.as_bytes())
        .await
        .map_err(|e| McpError::Transport(format!("write header: {e}")))?;
    writer
        .write_all(&body)
        .await
        .map_err(|e| McpError::Transport(format!("write body: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| McpError::Transport(format!("flush: {e}")))?;
    Ok(())
}

/// Read one framed JSON-RPC message. Returns
/// [`McpError::Protocol`] when the headers are malformed and
/// [`McpError::Transport`] on I/O failure.
async fn read_message<R: AsyncBufRead + Unpin + ?Sized>(
    reader: &mut R,
) -> Result<serde_json::Value, McpError> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| McpError::Transport(format!("read header: {e}")))?;
        if n == 0 {
            return Err(McpError::Transport("eof before headers complete".into()));
        }
        // Strip trailing CRLF / LF.
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            // Blank line — end of headers.
            break;
        }
        let (name, value) = trimmed
            .split_once(':')
            .ok_or_else(|| McpError::Protocol(format!("malformed header line: {trimmed:?}")))?;
        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|e| McpError::Protocol(format!("invalid Content-Length: {e}")))?;
            content_length = Some(parsed);
        }
        // Other headers (Content-Type, …) ignored.
    }
    let len =
        content_length.ok_or_else(|| McpError::Protocol("missing Content-Length header".into()))?;
    let mut body = vec![0u8; len];
    tokio::io::AsyncReadExt::read_exact(reader, &mut body)
        .await
        .map_err(|e| McpError::Transport(format!("read body ({len} bytes): {e}")))?;
    serde_json::from_slice(&body).map_err(|e| McpError::Protocol(format!("invalid json: {e}")))
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncWriteExt};

    /// Drive a fake MCP server on the far end of a duplex stream.
    /// The closure receives each parsed request and returns the
    /// matching response.
    async fn run_fake_server<F>(mut transport: tokio::io::DuplexStream, handler: F)
    where
        F: Fn(serde_json::Value) -> serde_json::Value + Send + 'static,
    {
        let (read, mut write) = tokio::io::split(&mut transport);
        let mut reader = BufReader::new(read);
        while let Ok(req) = read_message(&mut reader).await {
            let resp = handler(req);
            if write_message(&mut write, &resp).await.is_err() {
                break;
            }
        }
    }

    #[tokio::test]
    async fn framing_round_trips_a_message() {
        let (mut a, mut b) = duplex(1024);
        let payload = serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "ping"});
        write_message(&mut a, &payload).await.unwrap();
        let mut br = BufReader::new(&mut b);
        let parsed = read_message(&mut br).await.unwrap();
        assert_eq!(parsed, payload);
    }

    #[tokio::test]
    async fn framing_accepts_lf_only_line_endings() {
        // Python servers often emit LF (`\n`) rather than CRLF. The
        // reader must accept both.
        let (mut a, mut b) = duplex(1024);
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let header = format!("Content-Length: {}\n\n", body.len());
        a.write_all(header.as_bytes()).await.unwrap();
        a.write_all(body).await.unwrap();
        a.flush().await.unwrap();
        let mut br = BufReader::new(&mut b);
        let parsed = read_message(&mut br).await.unwrap();
        assert_eq!(parsed["result"]["ok"], true);
    }

    #[tokio::test]
    async fn framing_rejects_missing_content_length() {
        let (mut a, mut b) = duplex(1024);
        a.write_all(b"X-Other: 1\r\n\r\n{}").await.unwrap();
        a.flush().await.unwrap();
        let mut br = BufReader::new(&mut b);
        let err = read_message(&mut br).await.unwrap_err();
        assert!(matches!(err, McpError::Protocol(m) if m.contains("Content-Length")));
    }

    #[tokio::test]
    async fn list_tools_parses_canonical_response() {
        let (client_io, server_io) = duplex(4096);
        let (read_half, write_half) = tokio::io::split(client_io);
        let client = StdioMcpClient::with_streams("stub", write_half, read_half);
        tokio::spawn(run_fake_server(server_io, |req| {
            assert_eq!(req["method"], "tools/list");
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {
                    "tools": [
                        {"name": "echo", "description": "echo a body", "inputSchema": {"type": "object"}},
                        {"name": "ping", "description": "ping the server"}
                    ]
                }
            })
        }));
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo");
        assert_eq!(tools[0].description, "echo a body");
        assert_eq!(tools[1].name, "ping");
    }

    #[tokio::test]
    async fn call_tool_routes_through_jsonrpc_envelope() {
        let (client_io, server_io) = duplex(4096);
        let (read_half, write_half) = tokio::io::split(client_io);
        let client = StdioMcpClient::with_streams("stub", write_half, read_half);
        tokio::spawn(run_fake_server(server_io, |req| {
            assert_eq!(req["method"], "tools/call");
            assert_eq!(req["params"]["name"], "echo");
            assert_eq!(req["params"]["arguments"]["msg"], "hi");
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {"echoed": req["params"]["arguments"]}
            })
        }));
        let out = client
            .call_tool("echo", serde_json::json!({"msg": "hi"}))
            .await
            .unwrap();
        assert_eq!(out["echoed"]["msg"], "hi");
    }

    #[tokio::test]
    async fn server_error_propagates_as_mcp_server_variant() {
        let (client_io, server_io) = duplex(4096);
        let (read_half, write_half) = tokio::io::split(client_io);
        let client = StdioMcpClient::with_streams("stub", write_half, read_half);
        tokio::spawn(run_fake_server(server_io, |req| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "error": {"code": -32601, "message": "method not found"}
            })
        }));
        let err = client.list_tools().await.unwrap_err();
        match err {
            McpError::Server(m) => assert_eq!(m, "method not found"),
            other => panic!("expected Server, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn id_mismatch_surfaces_as_protocol_error() {
        let (client_io, server_io) = duplex(4096);
        let (read_half, write_half) = tokio::io::split(client_io);
        let client = StdioMcpClient::with_streams("stub", write_half, read_half);
        tokio::spawn(run_fake_server(server_io, |_req| {
            // Reply with a wrong id — the client must refuse rather
            // than silently accepting the wrong response.
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 9999,
                "result": {"tools": []}
            })
        }));
        let err = client.list_tools().await.unwrap_err();
        assert!(matches!(err, McpError::Protocol(m) if m.contains("id mismatch")));
    }

    #[tokio::test]
    async fn missing_result_and_error_surfaces_as_protocol_error() {
        let (client_io, server_io) = duplex(4096);
        let (read_half, write_half) = tokio::io::split(client_io);
        let client = StdioMcpClient::with_streams("stub", write_half, read_half);
        tokio::spawn(run_fake_server(
            server_io,
            |req| serde_json::json!({"jsonrpc": "2.0", "id": req["id"]}),
        ));
        let err = client.list_tools().await.unwrap_err();
        assert!(matches!(err, McpError::Protocol(m) if m.contains("missing both")));
    }

    #[tokio::test]
    async fn concurrent_calls_serialise_on_the_wire() {
        // Two `list_tools` calls in parallel must produce two intact
        // request/response pairs — no frame interleaving on the wire.
        let (client_io, server_io) = duplex(4096);
        let (read_half, write_half) = tokio::io::split(client_io);
        let client =
            std::sync::Arc::new(StdioMcpClient::with_streams("stub", write_half, read_half));
        tokio::spawn(run_fake_server(server_io, |req| {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {"tools": [{"name": format!("t{}", req["id"]), "description": ""}]}
            })
        }));
        let c1 = client.clone();
        let c2 = client.clone();
        let (a, b) = tokio::join!(c1.list_tools(), c2.list_tools());
        assert_eq!(a.unwrap().len(), 1);
        assert_eq!(b.unwrap().len(), 1);
    }

    #[test]
    fn spawn_returns_transport_error_when_binary_missing() {
        let err = StdioMcpClient::spawn(
            "missing",
            "/nonexistent/path/to/mcp-server-binary-9c2f",
            &["--mode=stdio"],
        )
        .unwrap_err();
        match err {
            McpError::Transport(m) => assert!(m.contains("spawn:")),
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_round_trips_against_a_real_child_process() {
        // Drive a real subprocess to prove the spawn path works
        // end-to-end. The child is a tiny `sh` script that:
        //   1. reads `Content-Length: N\r\n\r\n` plus N bytes,
        //   2. parses out the request id with `jq`,
        //   3. writes back a `tools/list` response with that id.
        // The test silently passes when `sh` or `jq` isn't on PATH
        // so it doesn't break CI on minimal containers.
        if which("sh").is_none() || which("jq").is_none() {
            return;
        }
        let script = r#"
read -r header
LEN=$(echo "$header" | sed 's/[^0-9]//g')
read -r _blank
BODY=$(head -c "$LEN")
ID=$(printf '%s' "$BODY" | jq '.id')
RESP=$(printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"sh stub"}]}}' "$ID")
LEN=${#RESP}
printf 'Content-Length: %d\r\n\r\n%s' "$LEN" "$RESP"
"#;
        let client = StdioMcpClient::spawn("sh-stub", "sh", &["-c", script]).expect("spawn");
        let tools = client.list_tools().await.expect("list_tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "ping");
    }

    /// Cross-platform `which` for the spawn-against-real-child test.
    fn which(bin: &str) -> Option<std::path::PathBuf> {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}
