//! Sprint 9 deferred tools — `web_fetch`, `web_search`,
//! `send_message`, `pdf_extract`, `mcp_invoke`, `terminal`.
//!
//! Each tool follows the existing `ToolTrait` pattern. Together they
//! close most of the Sprint 7 §4 deferred list:
//!
//! - [`WebFetchTool`] — HTTP GET + simple text-extraction (HTML
//!   tag stripping). Cap-gated `cap:network:http_get`.
//! - [`WebSearchTool`] — pluggable search via [`SearchProvider`]
//!   trait + `MockSearchProvider` for tests. Real backends slot
//!   in via plugin crates.
//! - [`SendMessageTool`] — typed dispatch to a registered channel.
//!   Cap-gated `cap:network:http_post`.
//! - [`PdfExtractTool`] — minimal PDF text extraction. Zero-dep
//!   fallback: walks `BT … ET` blocks for printable strings.
//! - [`McpInvokeTool`] — generic dynamic dispatch to any MCP server
//!   registered in an [`McpServerRegistry`]. Differs from the
//!   discovery-based [`crate::mcp::McpBridge`] which pre-builds one
//!   tool per remote-server-tool at startup; `mcp_invoke` is the
//!   late-binding sibling.
//! - [`TerminalTool`] — PTY-backed interactive shell tool. Defines
//!   a [`PtyBackend`] trait; a `portable-pty`-backed real impl slots
//!   in via a follow-on crate (the trait surface stays
//!   compositional / feature-flag-free in this base catalogue).
//!
//! All six follow GaussClaw's structural pattern (cap-gated,
//! adversarial-taint by default on network inbound, schema-guarded
//! output).

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use serde::{Deserialize, Serialize};

use crate::http::{HttpClient, HttpClientError, HttpMethod, HttpRequest, HttpToolPolicy};

// ─── web_fetch ─────────────────────────────────────────────────────────────

const WEB_FETCH_MANIFEST: &str = r#"
name        = "web_fetch"
description = "Fetch a URL and return a text-extracted body. HTML tags stripped; plain text preserved."
usage       = "Args: {url: string, max_chars?: uint}. Returns {status, text, truncated}."
caps        = ["network:http_get"]
taint       = "web"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// `web_fetch` tool — HTTP GET + plain-text extraction.
pub struct WebFetchTool {
    manifest: ToolManifest,
    client: Arc<dyn HttpClient>,
    policy: HttpToolPolicy,
}

impl WebFetchTool {
    /// Build a tool over a shared HTTP client.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(client: Arc<dyn HttpClient>) -> Self {
        let skill = SkillManifest::from_toml(WEB_FETCH_MANIFEST).expect("toml");
        let manifest = skill.compile(ToolId("web_fetch".into())).expect("compile");
        Self {
            manifest,
            client,
            policy: HttpToolPolicy::default(),
        }
    }
}

#[async_trait]
impl ToolTrait for WebFetchTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `url`".into()))?;
        let max_chars = args
            .get("max_chars")
            .and_then(serde_json::Value::as_u64)
            .map_or(8192, |n| n as usize);
        let req = HttpRequest {
            method: HttpMethod::Get,
            url: url.into(),
            headers: std::collections::BTreeMap::new(),
            body: None,
        };
        let req = self
            .policy
            .filter(req)
            .map_err(|e: HttpClientError| GaussError::Internal(format!("policy: {e}")))?;
        let resp = self
            .client
            .request(req)
            .await
            .map_err(|e| GaussError::Internal(format!("http: {e}")))?;
        let resp = self.policy.truncate(resp);
        let text = strip_html(&resp.body);
        let (text, capped) = cap_chars(&text, max_chars);
        Ok(serde_json::json!({
            "kind":      "web_fetch_result",
            "status":    resp.status,
            "text":      text,
            "truncated": resp.truncated || capped,
        }))
    }
}

/// Strip HTML tags from `body` and collapse whitespace. Zero-dep
/// approximation — sufficient for the agent-loop "read me this
/// page" use case.
#[must_use]
pub fn strip_html(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_tag = false;
    let mut prev_space = false;
    for c in body.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if c == '>' {
            in_tag = false;
            continue;
        }
        if in_tag {
            continue;
        }
        let space = c.is_whitespace();
        if space {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim_end().to_string()
}

fn cap_chars(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        return (s.to_string(), false);
    }
    (s.chars().take(max).collect(), true)
}

// ─── web_search ────────────────────────────────────────────────────────────

const WEB_SEARCH_MANIFEST: &str = r#"
name        = "web_search"
description = "Query a pluggable search backend and return ranked result snippets."
usage       = "Args: {query: string, limit?: uint}. Returns {results: [{title, url, snippet}]}."
caps        = ["network:http_get"]
taint       = "web"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// One search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResult {
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Short snippet / summary.
    pub snippet: String,
}

/// Pluggable search backend. Production deployments wire in
/// SerpAPI / Tavily / Brave Search; tests use [`MockSearchProvider`].
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Run a query and return at most `limit` results.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String>;
}

/// Deterministic mock provider for tests + the conformance suite.
pub struct MockSearchProvider {
    canned: std::sync::Mutex<Vec<SearchResult>>,
}

impl MockSearchProvider {
    /// Build a mock with the supplied canned results.
    #[must_use]
    pub fn new(results: Vec<SearchResult>) -> Self {
        Self {
            canned: std::sync::Mutex::new(results),
        }
    }
}

#[async_trait]
impl SearchProvider for MockSearchProvider {
    async fn search(&self, _query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let g = self.canned.lock().expect("poisoned");
        Ok(g.iter().take(limit).cloned().collect())
    }
}

/// `web_search` tool.
pub struct WebSearchTool {
    manifest: ToolManifest,
    provider: Arc<dyn SearchProvider>,
}

impl WebSearchTool {
    /// Build a tool over a search provider.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(provider: Arc<dyn SearchProvider>) -> Self {
        let skill = SkillManifest::from_toml(WEB_SEARCH_MANIFEST).expect("toml");
        let manifest = skill.compile(ToolId("web_search".into())).expect("compile");
        Self { manifest, provider }
    }
}

#[async_trait]
impl ToolTrait for WebSearchTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `query`".into()))?;
        let limit = args
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(5usize, |n| n as usize)
            .min(20);
        let results = self
            .provider
            .search(query, limit)
            .await
            .map_err(|e| GaussError::Internal(format!("search: {e}")))?;
        Ok(serde_json::json!({
            "kind":    "web_search_results",
            "query":   query,
            "results": results,
        }))
    }
}

// ─── send_message ──────────────────────────────────────────────────────────

const SEND_MESSAGE_MANIFEST: &str = r#"
name        = "send_message"
description = "Dispatch an outbound message through a registered channel adapter (slack, discord, telegram, etc.)."
usage       = "Args: {channel: string, recipient: string, body: string}. Returns {kind: 'message_queued'}."
caps        = ["network:http_post"]
taint       = "user"
reversible  = false
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// Outbound-message sink. Production wires this through
/// `gaussclaw-channels::ChannelTrait::send`; tests use the mock.
#[async_trait]
pub trait MessageSink: Send + Sync {
    /// Queue one outbound message on the named channel.
    async fn dispatch(&self, channel: &str, recipient: &str, body: &str) -> Result<(), String>;
}

/// Mock sink that records every dispatched message in-process.
pub struct MockMessageSink {
    log: std::sync::Mutex<Vec<(String, String, String)>>,
}

impl MockMessageSink {
    /// Build an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            log: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Borrow the per-dispatch log (`(channel, recipient, body)`).
    #[must_use]
    pub fn log(&self) -> Vec<(String, String, String)> {
        self.log.lock().expect("poisoned").clone()
    }
}

impl Default for MockMessageSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MessageSink for MockMessageSink {
    async fn dispatch(&self, channel: &str, recipient: &str, body: &str) -> Result<(), String> {
        self.log
            .lock()
            .expect("poisoned")
            .push((channel.into(), recipient.into(), body.into()));
        Ok(())
    }
}

/// `send_message` tool.
pub struct SendMessageTool {
    manifest: ToolManifest,
    sink: Arc<dyn MessageSink>,
}

impl SendMessageTool {
    /// Build a tool over a message sink.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(sink: Arc<dyn MessageSink>) -> Self {
        let skill = SkillManifest::from_toml(SEND_MESSAGE_MANIFEST).expect("toml");
        let manifest = skill
            .compile(ToolId("send_message".into()))
            .expect("compile");
        Self { manifest, sink }
    }
}

#[async_trait]
impl ToolTrait for SendMessageTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let channel = args
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `channel`".into()))?;
        let recipient = args
            .get("recipient")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `recipient`".into()))?;
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `body`".into()))?;
        self.sink
            .dispatch(channel, recipient, body)
            .await
            .map_err(|e| GaussError::Internal(format!("send: {e}")))?;
        Ok(serde_json::json!({
            "kind":      "message_queued",
            "channel":   channel,
            "recipient": recipient,
        }))
    }
}

// ─── pdf_extract ───────────────────────────────────────────────────────────

const PDF_EXTRACT_MANIFEST: &str = r#"
name        = "pdf_extract"
description = "Extract printable text from a base64-encoded PDF blob. Walks BT/ET blocks for plain-text strings; no font handling."
usage       = "Args: {pdf_base64: string, max_chars?: uint}. Returns {text, truncated}."
caps        = []
taint       = "user"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// `pdf_extract` tool.
pub struct PdfExtractTool {
    manifest: ToolManifest,
}

impl PdfExtractTool {
    /// Build.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(PDF_EXTRACT_MANIFEST).expect("toml");
        let manifest = skill
            .compile(ToolId("pdf_extract".into()))
            .expect("compile");
        Self { manifest }
    }
}

impl Default for PdfExtractTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for PdfExtractTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let b64 = args
            .get("pdf_base64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `pdf_base64`".into()))?;
        let max_chars = args
            .get("max_chars")
            .and_then(serde_json::Value::as_u64)
            .map_or(8192, |n| n as usize);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| GaussError::Internal(format!("base64: {e}")))?;
        let text = extract_pdf_text(&bytes);
        let (text, capped) = cap_chars(&text, max_chars);
        Ok(serde_json::json!({
            "kind":      "pdf_extract_result",
            "text":      text,
            "truncated": capped,
        }))
    }
}

/// Extract printable text from a PDF byte buffer. Walks `BT … ET`
/// text-blocks for parenthesised string literals (the most common
/// PDF text encoding). Returns plain UTF-8.
#[must_use]
pub fn extract_pdf_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let s = String::from_utf8_lossy(bytes);
    // Walk every `BT … ET` block.
    let mut cursor = 0usize;
    while let Some(start) = s[cursor..].find("BT") {
        let block_start = cursor.saturating_add(start);
        let Some(end_offset) = s[block_start..].find("ET") else {
            break;
        };
        let block_end = block_start.saturating_add(end_offset);
        let block = &s[block_start..block_end];
        // Pull (literal-string) sequences out.
        let mut bi = 0usize;
        while let Some(open) = block[bi..].find('(') {
            let after = bi.saturating_add(open).saturating_add(1);
            let close = block[after..].find(')');
            if let Some(c) = close {
                let lit = &block[after..after.saturating_add(c)];
                if !lit.is_empty() {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(lit);
                }
                bi = after.saturating_add(c).saturating_add(1);
            } else {
                break;
            }
        }
        cursor = block_end.saturating_add(2);
    }
    out
}

// ─── mcp_invoke ────────────────────────────────────────────────────────────

const MCP_INVOKE_MANIFEST: &str = r#"
name        = "mcp_invoke"
description = "Dispatch a typed JSON-RPC 2.0 call to a registered MCP server. Picks the server by name from the in-process registry."
usage       = "Args: {server: string, tool: string, args: object}. Returns {result: any}."
caps        = ["network:http_post"]
taint       = "web"
reversible  = false
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// Registry of named MCP clients. `mcp_invoke` picks the right
/// transport for a given `{server, ...}` payload by looking up the
/// server name here. Production deployments register one transport
/// per MCP server; tests use the in-process mock.
#[derive(Default)]
pub struct McpServerRegistry {
    clients: std::sync::Mutex<std::collections::BTreeMap<String, Arc<dyn crate::mcp::McpClient>>>,
}

impl std::fmt::Debug for McpServerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = self.clients.lock().expect("poisoned");
        f.debug_struct("McpServerRegistry")
            .field("servers", &g.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl McpServerRegistry {
    /// Build an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `client` under `name`. Subsequent registrations under
    /// the same name overwrite the prior client.
    pub fn register(&self, name: impl Into<String>, client: Arc<dyn crate::mcp::McpClient>) {
        self.clients
            .lock()
            .expect("poisoned")
            .insert(name.into(), client);
    }

    /// List the names of every registered server.
    #[must_use]
    pub fn server_names(&self) -> Vec<String> {
        self.clients
            .lock()
            .expect("poisoned")
            .keys()
            .cloned()
            .collect()
    }

    fn get(&self, name: &str) -> Option<Arc<dyn crate::mcp::McpClient>> {
        self.clients.lock().expect("poisoned").get(name).cloned()
    }
}

/// `mcp_invoke` tool — typed dispatch to a named MCP server.
pub struct McpInvokeTool {
    manifest: ToolManifest,
    registry: Arc<McpServerRegistry>,
}

impl McpInvokeTool {
    /// Build a tool over a server registry.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(registry: Arc<McpServerRegistry>) -> Self {
        let skill = SkillManifest::from_toml(MCP_INVOKE_MANIFEST).expect("toml");
        let manifest = skill.compile(ToolId("mcp_invoke".into())).expect("compile");
        Self { manifest, registry }
    }
}

#[async_trait]
impl ToolTrait for McpInvokeTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let server = args
            .get("server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `server`".into()))?;
        let tool = args
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `tool`".into()))?;
        let call_args = args.get("args").cloned().unwrap_or(serde_json::Value::Null);
        let client = self
            .registry
            .get(server)
            .ok_or_else(|| GaussError::Internal(format!("mcp: unknown server `{server}`")))?;
        let result = client
            .call_tool(tool, call_args)
            .await
            .map_err(|e| GaussError::Internal(format!("mcp: {e}")))?;
        Ok(serde_json::json!({
            "kind":   "mcp_invoke_result",
            "server": server,
            "tool":   tool,
            "result": result,
        }))
    }
}

// ─── terminal (PTY) ────────────────────────────────────────────────────────

const TERMINAL_MANIFEST: &str = r#"
name        = "terminal"
description = "Run a command in a PTY-backed shell and return its captured output."
usage       = "Args: {command: string, timeout_ms?: uint}. Returns {exit_code, stdout, stderr, duration_ms}."
caps        = ["executor:local"]
taint       = "user"
reversible  = false
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// One PTY session result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PtyResult {
    /// Exit status (`None` if the command was killed by signal or
    /// the backend cannot report it).
    pub exit_code: Option<i32>,
    /// Captured stdout (subject to the policy's `max_string_len`).
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// PTY transport. Production wires this through `portable-pty`; tests
/// use the in-process [`MockPtyBackend`]. The trait stays
/// dependency-free so the base tools crate doesn't drag a PTY library
/// into every consumer.
#[async_trait]
pub trait PtyBackend: Send + Sync {
    /// Run `command` in a PTY and return the captured result. The
    /// backend is responsible for enforcing `timeout_ms`; it should
    /// kill the child process and return whatever output was
    /// captured if the timeout fires.
    async fn run(&self, command: &str, timeout_ms: u64) -> Result<PtyResult, String>;
}

/// Deterministic mock backend keyed on the command string. Tests
/// register canned `(command, result)` pairs; an unmatched command
/// returns a default error.
pub struct MockPtyBackend {
    canned: std::sync::Mutex<std::collections::BTreeMap<String, PtyResult>>,
}

impl MockPtyBackend {
    /// Build an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            canned: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    /// Register a canned response for `command`.
    pub fn expect(&self, command: impl Into<String>, result: PtyResult) {
        self.canned
            .lock()
            .expect("poisoned")
            .insert(command.into(), result);
    }
}

impl Default for MockPtyBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PtyBackend for MockPtyBackend {
    async fn run(&self, command: &str, _timeout_ms: u64) -> Result<PtyResult, String> {
        let g = self.canned.lock().expect("poisoned");
        g.get(command)
            .cloned()
            .ok_or_else(|| format!("no canned response for command `{command}`"))
    }
}

/// `terminal` tool — PTY-backed shell command execution.
pub struct TerminalTool {
    manifest: ToolManifest,
    backend: Arc<dyn PtyBackend>,
    default_timeout_ms: u64,
}

impl TerminalTool {
    /// Build a tool over a PTY backend with a 30s default timeout.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new(backend: Arc<dyn PtyBackend>) -> Self {
        let skill = SkillManifest::from_toml(TERMINAL_MANIFEST).expect("toml");
        let manifest = skill.compile(ToolId("terminal".into())).expect("compile");
        Self {
            manifest,
            backend,
            default_timeout_ms: 30_000,
        }
    }

    /// Override the default timeout (used when args do not specify
    /// `timeout_ms`).
    #[must_use]
    pub fn with_default_timeout_ms(mut self, ms: u64) -> Self {
        self.default_timeout_ms = ms;
        self
    }
}

#[async_trait]
impl ToolTrait for TerminalTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `command`".into()))?;
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(self.default_timeout_ms);
        let result = self
            .backend
            .run(command, timeout_ms)
            .await
            .map_err(|e| GaussError::Internal(format!("pty: {e}")))?;
        Ok(serde_json::json!({
            "kind":        "terminal_result",
            "exit_code":   result.exit_code,
            "stdout":      result.stdout,
            "stderr":      result.stderr,
            "duration_ms": result.duration_ms,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpResponse, MockHttpClient};

    // ─── web_fetch ────────────────────────────────────────────────

    fn mock_http(url: &str, body: &str) -> Arc<dyn HttpClient> {
        let m = MockHttpClient::new();
        m.expect(
            HttpMethod::Get,
            url,
            HttpResponse {
                status: 200,
                headers: std::collections::BTreeMap::new(),
                body: body.into(),
                truncated: false,
            },
        );
        Arc::new(m)
    }

    #[tokio::test]
    async fn web_fetch_strips_html_tags() {
        let client = mock_http(
            "https://example.com/x",
            "<html><body><h1>Hi</h1><p>world</p></body></html>",
        );
        let t = WebFetchTool::new(client);
        let out = t
            .invoke_raw(serde_json::json!({"url": "https://example.com/x"}))
            .await
            .unwrap();
        assert_eq!(out["kind"], "web_fetch_result");
        let text = out["text"].as_str().unwrap();
        assert!(text.contains("Hi"));
        assert!(text.contains("world"));
        assert!(!text.contains("<h1>"));
    }

    #[tokio::test]
    async fn web_fetch_caps_max_chars() {
        let client = mock_http("https://example.com/big", "abcdefghij".repeat(100).as_str());
        let t = WebFetchTool::new(client);
        let out = t
            .invoke_raw(serde_json::json!({"url": "https://example.com/big", "max_chars": 50}))
            .await
            .unwrap();
        assert_eq!(out["truncated"], true);
        assert!(out["text"].as_str().unwrap().len() <= 50);
    }

    #[tokio::test]
    async fn web_fetch_rejects_missing_url() {
        let t = WebFetchTool::new(Arc::new(crate::http::UnconfiguredHttpClient));
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn strip_html_collapses_whitespace() {
        assert_eq!(strip_html("<p>  hello   <br>world </p>"), "hello world");
    }

    // ─── web_search ───────────────────────────────────────────────

    #[tokio::test]
    async fn web_search_returns_canned_results() {
        let provider = Arc::new(MockSearchProvider::new(vec![
            SearchResult {
                title: "Rust".into(),
                url: "https://www.rust-lang.org".into(),
                snippet: "A language empowering everyone".into(),
            },
            SearchResult {
                title: "Cargo".into(),
                url: "https://doc.rust-lang.org/cargo".into(),
                snippet: "The Rust package manager".into(),
            },
        ]));
        let t = WebSearchTool::new(provider);
        let out = t
            .invoke_raw(serde_json::json!({"query": "rust"}))
            .await
            .unwrap();
        let results = out["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["title"], "Rust");
    }

    #[tokio::test]
    async fn web_search_honours_limit() {
        let provider = Arc::new(MockSearchProvider::new(vec![
            SearchResult {
                title: "a".into(),
                url: "u".into(),
                snippet: "s".into(),
            },
            SearchResult {
                title: "b".into(),
                url: "u".into(),
                snippet: "s".into(),
            },
            SearchResult {
                title: "c".into(),
                url: "u".into(),
                snippet: "s".into(),
            },
        ]));
        let t = WebSearchTool::new(provider);
        let out = t
            .invoke_raw(serde_json::json!({"query": "x", "limit": 1}))
            .await
            .unwrap();
        assert_eq!(out["results"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn web_search_rejects_missing_query() {
        let t = WebSearchTool::new(Arc::new(MockSearchProvider::new(vec![])));
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    // ─── send_message ─────────────────────────────────────────────

    #[tokio::test]
    async fn send_message_dispatches_to_sink() {
        let sink = Arc::new(MockMessageSink::new());
        let t = SendMessageTool::new(sink.clone());
        let out = t
            .invoke_raw(serde_json::json!({
                "channel": "slack",
                "recipient": "#general",
                "body": "hello"
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "message_queued");
        let log = sink.log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0], ("slack".into(), "#general".into(), "hello".into()));
    }

    #[tokio::test]
    async fn send_message_rejects_missing_fields() {
        let t = SendMessageTool::new(Arc::new(MockMessageSink::new()));
        assert!(t
            .invoke_raw(serde_json::json!({"channel": "x"}))
            .await
            .is_err());
        assert!(t
            .invoke_raw(serde_json::json!({"recipient": "y"}))
            .await
            .is_err());
        assert!(t
            .invoke_raw(serde_json::json!({"body": "z"}))
            .await
            .is_err());
    }

    // ─── pdf_extract ──────────────────────────────────────────────

    /// A minimal PDF byte sequence containing one text block.
    fn sample_pdf_bytes() -> Vec<u8> {
        b"%PDF-1.4\n1 0 obj\n<<>>\nstream\nBT (Hello world) Tj ET\nendstream\nendobj".to_vec()
    }

    #[test]
    fn extract_pdf_text_pulls_strings_from_bt_blocks() {
        let bytes = sample_pdf_bytes();
        let text = extract_pdf_text(&bytes);
        assert!(text.contains("Hello world"));
    }

    #[test]
    fn extract_pdf_text_handles_multiple_blocks() {
        let bytes = b"BT (line one) Tj ET BT (line two) Tj ET";
        let text = extract_pdf_text(bytes);
        assert!(text.contains("line one"));
        assert!(text.contains("line two"));
    }

    #[test]
    fn extract_pdf_text_empty_for_non_pdf() {
        assert_eq!(extract_pdf_text(b"not a pdf at all"), "");
    }

    #[tokio::test]
    async fn pdf_extract_tool_round_trips() {
        let bytes = sample_pdf_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let t = PdfExtractTool::new();
        let out = t
            .invoke_raw(serde_json::json!({"pdf_base64": b64}))
            .await
            .unwrap();
        assert_eq!(out["kind"], "pdf_extract_result");
        assert!(out["text"].as_str().unwrap().contains("Hello world"));
    }

    #[tokio::test]
    async fn pdf_extract_rejects_bad_base64() {
        let t = PdfExtractTool::new();
        let err = t
            .invoke_raw(serde_json::json!({"pdf_base64": "!!!"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    // ─── mcp_invoke ───────────────────────────────────────────────

    #[tokio::test]
    async fn mcp_invoke_routes_to_registered_server() {
        use crate::mcp::{McpToolDescriptor, MockMcpClient};
        let registry = Arc::new(McpServerRegistry::new());
        let mock = MockMcpClient::new("local").with_tool(
            McpToolDescriptor::new("echo", "echo the body back"),
            |args| serde_json::json!({"echoed": args}),
        );
        registry.register("local", Arc::new(mock));
        let t = McpInvokeTool::new(registry.clone());
        let out = t
            .invoke_raw(serde_json::json!({
                "server": "local",
                "tool":   "echo",
                "args":   {"hi": 1}
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "mcp_invoke_result");
        assert_eq!(out["server"], "local");
        assert_eq!(out["tool"], "echo");
        assert_eq!(out["result"]["echoed"]["hi"], 1);
    }

    #[tokio::test]
    async fn mcp_invoke_rejects_unknown_server() {
        let registry = Arc::new(McpServerRegistry::new());
        let t = McpInvokeTool::new(registry);
        let err = t
            .invoke_raw(serde_json::json!({
                "server": "missing",
                "tool":   "x",
                "args":   {}
            }))
            .await
            .unwrap_err();
        match err {
            GaussError::Internal(m) => assert!(m.contains("unknown server")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mcp_invoke_rejects_missing_fields() {
        let registry = Arc::new(McpServerRegistry::new());
        let t = McpInvokeTool::new(registry);
        assert!(t
            .invoke_raw(serde_json::json!({"server": "x"}))
            .await
            .is_err());
        assert!(t
            .invoke_raw(serde_json::json!({"tool": "y"}))
            .await
            .is_err());
    }

    #[test]
    fn mcp_registry_lists_registered_servers() {
        let registry = McpServerRegistry::new();
        assert_eq!(registry.server_names().len(), 0);
        let mock = crate::mcp::MockMcpClient::new("alpha");
        registry.register("alpha", Arc::new(mock));
        let names = registry.server_names();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0], "alpha");
    }

    // ─── terminal ─────────────────────────────────────────────────

    fn canned_pty(stdout: &str, exit: i32) -> PtyResult {
        PtyResult {
            exit_code: Some(exit),
            stdout: stdout.into(),
            stderr: String::new(),
            duration_ms: 1,
        }
    }

    #[tokio::test]
    async fn terminal_runs_through_backend() {
        let backend = MockPtyBackend::new();
        backend.expect("echo hi", canned_pty("hi\n", 0));
        let t = TerminalTool::new(Arc::new(backend));
        let out = t
            .invoke_raw(serde_json::json!({"command": "echo hi"}))
            .await
            .unwrap();
        assert_eq!(out["kind"], "terminal_result");
        assert_eq!(out["exit_code"], 0);
        assert_eq!(out["stdout"], "hi\n");
    }

    #[tokio::test]
    async fn terminal_propagates_nonzero_exit() {
        let backend = MockPtyBackend::new();
        backend.expect("false", canned_pty("", 1));
        let t = TerminalTool::new(Arc::new(backend));
        let out = t
            .invoke_raw(serde_json::json!({"command": "false"}))
            .await
            .unwrap();
        assert_eq!(out["exit_code"], 1);
    }

    #[tokio::test]
    async fn terminal_rejects_missing_command() {
        let t = TerminalTool::new(Arc::new(MockPtyBackend::new()));
        let err = t.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn terminal_propagates_backend_error() {
        let t = TerminalTool::new(Arc::new(MockPtyBackend::new()));
        let err = t
            .invoke_raw(serde_json::json!({"command": "unknown"}))
            .await
            .unwrap_err();
        match err {
            GaussError::Internal(m) => assert!(m.contains("pty:")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn terminal_manifest_demands_executor_local_cap() {
        let t = TerminalTool::new(Arc::new(MockPtyBackend::new()));
        assert_eq!(
            t.manifest().cap_required.bits(),
            gauss_core::CapToken::EXECUTOR_LOCAL.bits()
        );
    }
}
