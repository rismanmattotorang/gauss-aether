//! `code_execution_tool` — WASM-sandboxed code execution (Sprint 6 §5).
//!
//! Hermes's `code_execution_tool` runs Python in the host process
//! (with operator credentials) or shells out to a Docker container.
//! Either path can starve the host (no fuel cap, no memory cap, no
//! determinism) and the container path requires Docker on PATH.
//!
//! GaussClaw's variant is a **single-binary, fuel-metered WASM
//! sandbox** built on `wasmi` (already in workspace deps). The
//! shipping invariants:
//!
//! - **Fuel metering.** Every invocation has a hard instruction
//!   budget (default 1M); the interpreter traps the moment fuel
//!   runs out, returning [`gauss_core::GaussError::Internal`] with
//!   the fuel diagnostic.
//! - **No host imports.** The linker is empty — the module runs
//!   pure-computation only; it cannot reach the FS, network, or
//!   environment. A future Sprint adds an opt-in `gauss_yield` host
//!   function gated by cap.
//! - **Single-instance per call.** The module is freshly
//!   instantiated for every execution; no mutable state survives
//!   between calls.
//! - **Deterministic.** Given (bytecode, fuel) the trap point is
//!   reproducible across machines — the conformance suite locks the
//!   replay corpus.
//!
//! ## Hermes-superiority axes
//!
//! - **Single static binary.** No Docker required; no Python
//!   interpreter required. The WASM module is the contract.
//! - **Fuel cap.** Hermes's Python execution loops indefinitely
//!   against the host. Ours traps.
//! - **No host imports.** Hermes's tool has access to the parent's
//!   `subprocess.run` family. Ours cannot.

use async_trait::async_trait;
use base64::Engine as _;
use gauss_core::{CapToken, GaussError, GaussResult, ToolId};
use gauss_sandbox::WasmSandbox;
use gauss_traits::{SandboxRequest, SandboxTrait, ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "code_execution"
description = "Execute a WASM module in a fuel-metered sandbox. Returns the integer exit value of the module's `main` export."
usage       = "Args: {wasm_base64: string, fuel?: uint}. Fuel defaults to 1_000_000."
caps        = ["code:execute"]
taint       = "user"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 65536

[schema]
type = "object"
"#;

/// Default fuel budget surfaced when the caller doesn't supply one.
pub const DEFAULT_FUEL: u64 = 1_000_000;

/// `code_execution` tool.
pub struct CodeExecutionTool {
    manifest: ToolManifest,
}

impl CodeExecutionTool {
    /// Build a code-execution tool.
    ///
    /// # Panics
    /// Build-time only.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded toml");
        let manifest = skill
            .compile(ToolId("code_execution".into()))
            .expect("compile");
        Self { manifest }
    }
}

impl Default for CodeExecutionTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for CodeExecutionTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let wasm_b64 = args
            .get("wasm_base64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `wasm_base64`".into()))?;
        let fuel = args
            .get("fuel")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_FUEL);

        let wasm = base64::engine::general_purpose::STANDARD
            .decode(wasm_b64)
            .map_err(|e| GaussError::Internal(format!("base64 decode: {e}")))?;
        if wasm.is_empty() {
            return Err(GaussError::Internal("empty wasm payload".into()));
        }
        let sandbox = WasmSandbox::from_bytes(&wasm)?.with_fuel(fuel);
        let req = SandboxRequest::new(
            self.manifest.id.clone(),
            CapToken::BOTTOM,
            serde_json::Value::Null,
            Vec::new(),
        );
        let outcome = sandbox.exec(req).await?;
        Ok(serde_json::json!({
            "kind":     "code_execution_result",
            "exit":     outcome.exit_code,
            "fuel_cap": fuel,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-compiled WASM module that exports `main` returning the constant 42.
    ///
    /// `wat`:
    /// ```wat
    /// (module
    ///   (func (export "main") (result i32) i32.const 42))
    /// ```
    fn wasm_main_returns_42() -> Vec<u8> {
        // Minimal handcrafted bytecode for the above. Magic + version
        // + type section + function section + export section + code
        // section. Validated against `wasmi 0.46`.
        vec![
            0x00, 0x61, 0x73, 0x6d, // \0asm
            0x01, 0x00, 0x00, 0x00, // version 1
            // type section: 1 type, () -> i32
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // function section: 1 fn, type 0
            0x03, 0x02, 0x01, 0x00, // export section: 1 export "main" fn 0
            0x07, 0x08, 0x01, 0x04, 0x6d, 0x61, 0x69, 0x6e, 0x00, 0x00,
            // code section: 1 body, locals=[], i32.const 42, end
            0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b,
        ]
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[tokio::test]
    async fn executes_minimal_wasm_module_returning_42() {
        let tool = CodeExecutionTool::new();
        let out = tool
            .invoke_raw(serde_json::json!({
                "wasm_base64": b64(&wasm_main_returns_42()),
            }))
            .await
            .unwrap();
        assert_eq!(out["kind"], "code_execution_result");
        assert_eq!(out["exit"], 42);
        assert_eq!(out["fuel_cap"], DEFAULT_FUEL);
    }

    #[tokio::test]
    async fn rejects_empty_payload() {
        let tool = CodeExecutionTool::new();
        let err = tool
            .invoke_raw(serde_json::json!({"wasm_base64": ""}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn rejects_invalid_base64() {
        let tool = CodeExecutionTool::new();
        let err = tool
            .invoke_raw(serde_json::json!({"wasm_base64": "!!!not base64"}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn rejects_invalid_wasm_bytecode() {
        let tool = CodeExecutionTool::new();
        // base64("not wasm")
        let payload = b64(b"not wasm");
        let err = tool
            .invoke_raw(serde_json::json!({"wasm_base64": payload}))
            .await
            .unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn honours_explicit_fuel_cap() {
        let tool = CodeExecutionTool::new();
        let out = tool
            .invoke_raw(serde_json::json!({
                "wasm_base64": b64(&wasm_main_returns_42()),
                "fuel": 5000,
            }))
            .await
            .unwrap();
        assert_eq!(out["fuel_cap"], 5000);
    }

    #[tokio::test]
    async fn missing_wasm_field_rejected() {
        let tool = CodeExecutionTool::new();
        let err = tool.invoke_raw(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[test]
    fn manifest_declares_code_execute_cap() {
        let tool = CodeExecutionTool::new();
        // We share the EXECUTOR_LOCAL cap bit for `code:execute`
        // (see gaussclaw-skill::parse_cap). The point is the cap
        // separation, not the bit number.
        assert_ne!(tool.manifest().cap_required.bits(), 0);
    }
}
