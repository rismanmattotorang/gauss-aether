//! HTTP transport for the [`McpClient`] trait.
//!
//! OpenHarness (HKUDS/OpenHarness) ships an HTTP-transport MCP client
//! that speaks the JSON-RPC 2.0 dialect of the Model Context Protocol.
//! This module ports the same surface to GaussClaw, but layered on
//! top of the existing [`HttpClient`] trait already used by
//! `http_get` / `http_post` — so we inherit:
//!
//! * cap-gating via the tool registry (`network:http_post`),
//! * header allowlist and body-size cap via [`HttpToolPolicy`],
//! * deterministic mocking via [`MockHttpClient`],
//! * audit-log integration via the existing turn policy.
//!
//! No new HTTP-client dependency is pulled in. The crate stays
//! `reqwest`-free; operators inject the same transport they already
//! use for `http_*` tools.
//!
//! ## Wire format (simplified MCP)
//!
//! Two JSON-RPC methods are spoken:
//!
//! * `tools/list` — request body `{"jsonrpc":"2.0","id":1,"method":"tools/list"}`,
//!   response result `{"tools":[{"name":"…","description":"…","inputSchema":{...}}]}`.
//! * `tools/call` — request body
//!   `{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"…","arguments":{...}}}`,
//!   response result is the raw JSON to feed back to the agent.
//!
//! The transport is intentionally minimal: it does NOT speak the full
//! MCP capability-negotiation handshake (servers can require it; we
//! treat the negotiation as a deployment concern and document the
//! gap). Production deployments that need full MCP compatibility wire
//! a richer client through the same [`McpClient`] trait.
//!
//! ## Security posture
//!
//! Every response is parsed and then crosses the HWCA schema gate
//! when the bridge's [`ToolTrait::invoke_raw`] returns it. The schema
//! gate filters instruction substrings (`SchemaGuards::strict`), so
//! the IPI-by-MCP vector is closed by construction.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::http::{HttpClient, HttpClientError, HttpMethod, HttpRequest, HttpToolPolicy};
use crate::mcp::{McpClient, McpError, McpToolDescriptor};

/// One MCP-over-HTTP client.
///
/// Cheap to clone — internal state is shared via `Arc`.
#[derive(Clone)]
pub struct HttpMcpClient {
    /// Display name surfaced through the bridge id (`mcp:<name>:<tool>`).
    server_name: String,
    /// Absolute JSON-RPC endpoint URL.
    endpoint: String,
    /// Transport.
    http: Arc<dyn HttpClient>,
    /// Operator policy applied to every outbound request (header
    /// allowlist, body cap, URL scheme guard).
    policy: HttpToolPolicy,
    /// Static request headers (e.g. `Authorization: Bearer …`).
    /// Subject to the allowlist filter just like every other header.
    headers: BTreeMap<String, String>,
}

impl HttpMcpClient {
    /// Build a new client.
    pub fn new(
        server_name: impl Into<String>,
        endpoint: impl Into<String>,
        http: Arc<dyn HttpClient>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            endpoint: endpoint.into(),
            http,
            policy: HttpToolPolicy::default(),
            headers: BTreeMap::new(),
        }
    }

    /// Builder: swap the [`HttpToolPolicy`].
    #[must_use]
    pub fn with_policy(mut self, policy: HttpToolPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Builder: add a static header (overwriting if already present).
    /// Use for `Authorization`, `X-API-Key`, etc. The header is still
    /// subject to the policy's allowlist filter.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

#[async_trait]
impl McpClient for HttpMcpClient {
    fn server_name(&self) -> &str {
        &self.server_name
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
        });
        let resp = self.do_rpc(body).await?;
        let tools = resp
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
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        });
        self.do_rpc(body).await
    }
}

impl HttpMcpClient {
    /// Send one JSON-RPC request and return the `result` field of the
    /// response (already unwrapped). Wire `error` is mapped to
    /// [`McpError::Server`]; transport failure maps to
    /// [`McpError::Transport`].
    async fn do_rpc(&self, body: serde_json::Value) -> Result<serde_json::Value, McpError> {
        let body_str = body.to_string();
        let mut headers = self.headers.clone();
        headers
            .entry("content-type".to_owned())
            .or_insert_with(|| "application/json".to_owned());
        headers
            .entry("accept".to_owned())
            .or_insert_with(|| "application/json".to_owned());
        let req = HttpRequest {
            method: HttpMethod::Post,
            url: self.endpoint.clone(),
            headers,
            body: Some(body_str),
        };
        let req = self.policy.filter(req).map_err(map_http_err)?;
        let resp = self.http.request(req).await.map_err(map_http_err)?;
        if resp.truncated {
            // A truncated MCP response would be silently parsed as
            // incomplete JSON — flag explicitly so the caller can
            // raise the body cap.
            return Err(McpError::Protocol(format!(
                "response body truncated (status {})",
                resp.status
            )));
        }
        let parsed: serde_json::Value = serde_json::from_str(&resp.body)
            .map_err(|e| McpError::Protocol(format!("invalid json: {e}")))?;
        if let Some(err) = parsed.get("error") {
            let msg = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown server error");
            return Err(McpError::Server(msg.to_owned()));
        }
        let result = parsed.get("result").cloned().ok_or_else(|| {
            McpError::Protocol("response missing both `result` and `error`".into())
        })?;
        Ok(result)
    }
}

fn map_http_err(e: HttpClientError) -> McpError {
    match e {
        HttpClientError::Transport(s) => McpError::Transport(s),
        HttpClientError::Status { status, body } => {
            McpError::Server(format!("HTTP {status}: {body}"))
        }
        HttpClientError::PolicyDenied(s) => McpError::Transport(format!("policy: {s}")),
        HttpClientError::NotConfigured => McpError::Transport("HttpClient not configured".into()),
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpBridge;
    use std::sync::Mutex;

    /// In-test HTTP server. Records every request and returns a
    /// scripted JSON response.
    struct ScriptedHttp {
        responses: Mutex<Vec<String>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl ScriptedHttp {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().rev().map(str::to_owned).collect()),
                seen: Mutex::new(Vec::new()),
            }
        }
        fn seen(&self) -> Vec<HttpRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HttpClient for ScriptedHttp {
        async fn request(
            &self,
            req: HttpRequest,
        ) -> Result<crate::http::HttpResponse, HttpClientError> {
            self.seen.lock().unwrap().push(req);
            let body = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| "{}".into());
            Ok(crate::http::HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body,
                truncated: false,
            })
        }
    }

    fn make_client(http: Arc<dyn HttpClient>) -> HttpMcpClient {
        HttpMcpClient::new("scripted", "https://example.test/mcp", http)
    }

    #[tokio::test]
    async fn list_tools_parses_jsonrpc_result() {
        let http = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[
                {"name":"ping","description":"reply pong","inputSchema":{"type":"object"}},
                {"name":"echo","description":"echo back","inputSchema":{"type":"object"}}
            ]}}"#,
        ]));
        let client = make_client(http);
        let tools = client.list_tools().await.expect("list");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "ping");
        assert_eq!(tools[1].description, "echo back");
    }

    #[tokio::test]
    async fn list_tools_handles_snake_case_input_schema_alias() {
        let http = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[
                {"name":"x","description":"","input_schema":{"type":"object","title":"x"}}
            ]}}"#,
        ]));
        let client = make_client(http);
        let tools = client.list_tools().await.expect("list");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].input_schema.get("title").and_then(|v| v.as_str()),
            Some("x")
        );
    }

    #[tokio::test]
    async fn call_tool_forwards_arguments_in_rpc_body() {
        let http = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":2,"result":{"ok":true,"echoed":{"hi":"there"}}}"#,
        ]));
        let scripted = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":2,"result":{"ok":true,"echoed":{"hi":"there"}}}"#,
        ]));
        let client = make_client(scripted.clone());
        let result = client
            .call_tool("echo", serde_json::json!({ "hi": "there" }))
            .await
            .expect("call");
        assert_eq!(result["echoed"]["hi"], "there");
        let _ = http; // keep references symmetric for readers

        // Verify the request body actually included the args.
        let seen = scripted.seen();
        let body = seen[0].body.as_deref().unwrap_or("");
        assert!(body.contains("\"name\":\"echo\""));
        assert!(body.contains("\"hi\":\"there\""));
    }

    #[tokio::test]
    async fn jsonrpc_error_maps_to_mcp_server_error() {
        let http = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"method not found"}}"#,
        ]));
        let client = make_client(http);
        let err = client
            .call_tool("missing", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::Server(_)));
        assert!(err.to_string().contains("method not found"));
    }

    #[tokio::test]
    async fn invalid_json_response_maps_to_protocol_error() {
        let http = Arc::new(ScriptedHttp::new(vec!["not json at all"]));
        let client = make_client(http);
        let err = client.list_tools().await.unwrap_err();
        assert!(matches!(err, McpError::Protocol(_)));
    }

    #[tokio::test]
    async fn missing_result_and_error_is_protocol_error() {
        let http = Arc::new(ScriptedHttp::new(vec![r#"{"jsonrpc":"2.0","id":1}"#]));
        let client = make_client(http);
        let err = client.list_tools().await.unwrap_err();
        assert!(matches!(err, McpError::Protocol(_)));
    }

    #[tokio::test]
    async fn with_header_attaches_authorization() {
        let http = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#,
        ]));
        let scripted = http.clone();
        // The default HttpToolPolicy drops non-allowlisted headers, so
        // operators must add Authorization (and content-type / accept,
        // which the client sets automatically) to the allowlist before
        // they survive `policy.filter`.
        let policy = HttpToolPolicy {
            header_allowlist: vec![
                "authorization".into(),
                "content-type".into(),
                "accept".into(),
            ],
            ..HttpToolPolicy::default()
        };
        let client = make_client(http)
            .with_policy(policy)
            .with_header("Authorization", "Bearer xyz");
        let _ = client.list_tools().await.expect("list");
        let seen = scripted.seen();
        assert!(seen
            .first()
            .is_some_and(|r| r.headers.values().any(|v| v.contains("Bearer xyz"))));
    }

    /// Without the header allowlist, the default policy strips
    /// `Authorization` — defence-in-depth against accidental
    /// credential leakage. We document that as observable behaviour:
    /// the request still completes, but the header is absent.
    #[tokio::test]
    async fn default_policy_strips_unallowed_headers() {
        let http = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#,
        ]));
        let scripted = http.clone();
        let client = make_client(http).with_header("Authorization", "Bearer xyz");
        let _ = client.list_tools().await.expect("list");
        let seen = scripted.seen();
        // No header survived the default-policy filter.
        assert!(seen
            .first()
            .is_some_and(|r| !r.headers.values().any(|v| v.contains("Bearer xyz"))));
    }

    /// End-to-end: HTTP-backed MCP client + bridge → real `ToolTrait`
    /// that dispatches RPC under the cover.
    #[tokio::test]
    async fn bridge_dispatches_through_http_client() {
        let http = Arc::new(ScriptedHttp::new(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[
                {"name":"echo","description":"","inputSchema":{"type":"object"}}
            ]}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"echoed":"hi"}}"#,
        ]));
        let client = Arc::new(make_client(http));
        let tools = McpBridge::new(client).build().await.unwrap();
        assert_eq!(tools.len(), 1);
        let out = tools[0]
            .invoke_raw(serde_json::json!({ "msg": "hi" }))
            .await
            .expect("dispatch");
        assert_eq!(out["echoed"], "hi");
    }
}
