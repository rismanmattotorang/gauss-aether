//! MCP (Model Context Protocol) bridge — expose remote MCP servers'
//! tools as local [`ToolTrait`]s.
//!
//! OpenHarness (HKUDS/OpenHarness) ships an MCP client that connects
//! to tool-only MCP servers over HTTP and surfaces each remote tool
//! to the agent loop. This module ports the same surface to GaussClaw
//! without breaking the existing kernel/HWCA design:
//!
//! * The transport is a trait ([`McpClient`]). Real HTTP transports
//!   live behind a feature flag and ship in a follow-on; the in-memory
//!   [`MockMcpClient`] is enough to wire the bridge end-to-end in tests.
//! * Each remote tool becomes one [`McpToolBridge`] that implements
//!   [`ToolTrait`]. The bridge carries the same `ToolManifest`
//!   discipline as every first-party tool — caps, schema-gate guards,
//!   output JSON Schema — so the HWCA worker still validates output
//!   before it crosses back to the parent context (Axiom A7).
//! * The bridge taints every output as [`TaintLabel::Web`] by default
//!   (remote MCP servers are an untrusted IPI source) and ALL bridges
//!   declare [`CapToken::NETWORK_POST`] — the kernel admit gate
//!   refuses the call when the session's grant doesn't include it.
//!   The session operator can tighten further per-server in config.
//!
//! ## Why a bridge, not a separate tool path?
//!
//! Keeping MCP tools behind the same [`ToolTrait`] means the existing
//! `AgentLoop` dispatches them unchanged: pre-hooks fire, the HWCA
//! schema gate validates the result, the audit chain records the
//! receipt. MCP becomes "another tool source" rather than a new
//! execution surface — small, additive, no new attack surface.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_traits::{OutputSchema, SchemaGuards, ToolManifest, ToolTrait};
use serde::{Deserialize, Serialize};

// ─── transport trait ──────────────────────────────────────────────────────

/// One remote tool as advertised by an MCP server. Mirrors the MCP
/// `tools/list` response shape, simplified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct McpToolDescriptor {
    /// Server-side tool name (used as the dispatch key).
    pub name: String,
    /// Free-form description shown to the model.
    pub description: String,
    /// JSON Schema for the tool's input arguments. The bridge does NOT
    /// validate input against this — the schema gate validates *output*
    /// only. This field is retained for completeness and so callers
    /// that want input validation can layer it externally.
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

impl McpToolDescriptor {
    /// Convenience constructor.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: serde_json::Value::Null,
        }
    }
}

/// MCP transport. Implement once per transport (HTTP, stdio, …).
#[async_trait]
pub trait McpClient: Send + Sync {
    /// Human-readable server identifier — appears in audit-chain rows.
    fn server_name(&self) -> &str;

    /// List the tools the server exposes. Called once at bridge
    /// construction time; the bridge caches the result.
    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError>;

    /// Dispatch one tool call. The server returns raw JSON; the
    /// bridge's [`ToolTrait::invoke_raw`] returns it unchanged so the
    /// HWCA schema gate can validate it next.
    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpError>;
}

// ─── errors ───────────────────────────────────────────────────────────────

/// MCP transport / dispatch failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum McpError {
    /// Network or transport-layer failure.
    #[error("transport: {0}")]
    Transport(String),
    /// Server-side error (the request reached the server but it
    /// refused the call).
    #[error("server: {0}")]
    Server(String),
    /// Local protocol-violation — the response was syntactically valid
    /// but failed semantic checks.
    #[error("protocol: {0}")]
    Protocol(String),
    /// Requested tool is not in the server's catalogue.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}

impl From<McpError> for GaussError {
    fn from(e: McpError) -> Self {
        Self::Internal(format!("mcp: {e}"))
    }
}

// ─── bridge ───────────────────────────────────────────────────────────────

/// Adapter from one remote MCP tool to one local [`ToolTrait`]. Created
/// in batches by [`McpBridge::build`].
pub struct McpToolBridge {
    manifest: ToolManifest,
    remote_name: String,
    client: Arc<dyn McpClient>,
}

impl McpToolBridge {
    /// The remote tool name on the upstream server. Distinct from
    /// `manifest.id` because we prefix it with the server name to
    /// keep the local registry collision-free.
    #[must_use]
    pub fn remote_name(&self) -> &str {
        &self.remote_name
    }
}

#[async_trait]
impl ToolTrait for McpToolBridge {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        self.client
            .call_tool(&self.remote_name, args)
            .await
            .map_err(Into::into)
    }
}

/// Builds one bridge per remote tool the server advertises.
pub struct McpBridge {
    client: Arc<dyn McpClient>,
}

impl McpBridge {
    /// Wrap a client.
    #[must_use]
    pub fn new(client: Arc<dyn McpClient>) -> Self {
        Self { client }
    }

    /// Discover the server's tool catalogue and synthesise one
    /// [`McpToolBridge`] per remote tool.
    ///
    /// Each bridge's local id is `mcp:<server>:<tool>` — the prefix
    /// keeps the local registry unambiguous when multiple servers are
    /// loaded.
    pub async fn build(&self) -> Result<Vec<Arc<dyn ToolTrait>>, McpError> {
        let descriptors = self.client.list_tools().await?;
        let server = self.client.server_name();
        let mut out: Vec<Arc<dyn ToolTrait>> = Vec::with_capacity(descriptors.len());
        for d in descriptors {
            let local_id = format!("mcp:{server}:{}", d.name);
            // Default output taint = Web (paper §A6); guards.strict by
            // default so IPI substrings cannot ride MCP responses into
            // the parent context.
            let manifest = ToolManifest::new(
                ToolId(local_id),
                CapToken::NETWORK_POST,
                false, // reversible: assume false — operator overrides per server
                OutputSchema::new(default_permissive_schema(), 65_536),
                SchemaGuards::strict(),
            );
            out.push(Arc::new(McpToolBridge {
                manifest,
                remote_name: d.name,
                client: Arc::clone(&self.client),
            }));
        }
        Ok(out)
    }
}

fn default_permissive_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object" })
}

// ─── mock client (for tests + dry-run) ────────────────────────────────────

/// In-memory mock MCP client. Tests use this to drive the bridge
/// without standing up a real server.
///
/// Production replaces it with an HTTP or stdio implementation; the
/// trait surface stays identical.
pub struct MockMcpClient {
    server_name: String,
    catalogue: Vec<McpToolDescriptor>,
    handlers: BTreeMap<String, Box<dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync>>,
}

impl MockMcpClient {
    /// Build an empty mock with the given server name.
    #[must_use]
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            catalogue: Vec::new(),
            handlers: BTreeMap::new(),
        }
    }

    /// Add a tool to the mock catalogue. `handler` is called whenever
    /// the bridge invokes the tool; it returns the raw JSON the
    /// upstream server would have returned.
    #[must_use]
    pub fn with_tool(
        mut self,
        descriptor: McpToolDescriptor,
        handler: impl Fn(serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
    ) -> Self {
        self.handlers
            .insert(descriptor.name.clone(), Box::new(handler));
        self.catalogue.push(descriptor);
        self
    }
}

#[async_trait]
impl McpClient for MockMcpClient {
    fn server_name(&self) -> &str {
        &self.server_name
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        Ok(self.catalogue.clone())
    }

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| McpError::UnknownTool(name.to_owned()))?;
        Ok(handler(args))
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mock() -> MockMcpClient {
        MockMcpClient::new("test-server")
            .with_tool(
                McpToolDescriptor::new("ping", "Reply with pong."),
                |_args| serde_json::json!({ "ok": true, "pong": true }),
            )
            .with_tool(
                McpToolDescriptor::new("echo", "Echo the input back."),
                |args| serde_json::json!({ "echoed": args }),
            )
    }

    #[tokio::test]
    async fn bridge_builds_one_tool_per_remote() {
        let client: Arc<dyn McpClient> = Arc::new(make_mock());
        let bridge = McpBridge::new(client);
        let tools = bridge.build().await.expect("build");
        assert_eq!(tools.len(), 2);
        let ids: Vec<&str> = tools.iter().map(|t| t.manifest().id.0.as_str()).collect();
        assert!(ids.contains(&"mcp:test-server:ping"));
        assert!(ids.contains(&"mcp:test-server:echo"));
    }

    #[tokio::test]
    async fn bridge_invokes_remote_tool() {
        let client: Arc<dyn McpClient> = Arc::new(make_mock());
        let tools = McpBridge::new(client).build().await.unwrap();
        let echo = tools
            .iter()
            .find(|t| t.manifest().id.0 == "mcp:test-server:echo")
            .unwrap();
        let out = echo
            .invoke_raw(serde_json::json!({ "msg": "hi" }))
            .await
            .unwrap();
        assert_eq!(out["echoed"]["msg"], "hi");
    }

    #[tokio::test]
    async fn unknown_remote_tool_errors_through_gauss_error() {
        struct Stub;
        #[async_trait]
        impl McpClient for Stub {
            fn server_name(&self) -> &'static str {
                "stub"
            }
            async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
                Ok(vec![McpToolDescriptor::new("only", "")])
            }
            async fn call_tool(
                &self,
                _name: &str,
                _args: serde_json::Value,
            ) -> Result<serde_json::Value, McpError> {
                Err(McpError::UnknownTool("missing".into()))
            }
        }
        let tools = McpBridge::new(Arc::new(Stub)).build().await.unwrap();
        let only = tools.first().unwrap();
        let err = only.invoke_raw(serde_json::json!({})).await.unwrap_err();
        // McpError::UnknownTool maps to GaussError::Internal via From.
        assert!(matches!(err, GaussError::Internal(_)));
        if let GaussError::Internal(msg) = err {
            assert!(msg.contains("mcp:"));
            assert!(msg.contains("unknown tool"));
        }
    }

    #[tokio::test]
    async fn manifest_declares_network_cap_and_strict_guards() {
        let tools = McpBridge::new(Arc::new(make_mock())).build().await.unwrap();
        let t = tools.first().unwrap();
        // Every bridge demands NETWORK_POST; no MCP call rides without
        // an explicit network grant.
        assert_eq!(
            t.manifest().cap_required.bits(),
            CapToken::NETWORK_POST.bits()
        );
        // Strict schema-gate guards (IPI defence) on by default.
        assert!(t.manifest().guards.no_instruction_substrings);
    }

    #[tokio::test]
    async fn transport_failure_propagates_as_gauss_internal() {
        struct Down;
        #[async_trait]
        impl McpClient for Down {
            fn server_name(&self) -> &'static str {
                "down"
            }
            async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
                Ok(vec![McpToolDescriptor::new("x", "")])
            }
            async fn call_tool(
                &self,
                _n: &str,
                _a: serde_json::Value,
            ) -> Result<serde_json::Value, McpError> {
                Err(McpError::Transport("connection refused".into()))
            }
        }
        let tools = McpBridge::new(Arc::new(Down)).build().await.unwrap();
        let err = tools[0]
            .invoke_raw(serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    /// Empty catalogue is fine — `build` returns Ok(vec![]).
    #[tokio::test]
    async fn empty_catalogue_returns_empty_vec() {
        let client = Arc::new(MockMcpClient::new("empty"));
        let tools = McpBridge::new(client).build().await.unwrap();
        assert!(tools.is_empty());
    }
}
