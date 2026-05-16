//! L1 — WebAssembly sandbox via `wasmi` (pure-Rust interpreter).
//!
//! Phase 3 ships the wasmi backend; Phase 10 will add a wasmtime backend
//! under feature `wasm-wasmtime` for production-grade JIT performance. The
//! trait surface is identical so swap is a build-flag flip (ADR-0009).
//!
//! ## Confinement guarantees
//!
//! * **Fuel metering** — each invocation runs with a finite *fuel* budget
//!   (~1M instructions by default) supplied via `Store::set_fuel`. The
//!   interpreter traps the moment the budget runs out, returning `Io` to the
//!   caller. This corresponds to the paper's "fuel + epoch" discipline.
//! * **No host imports beyond the gauss-defined ABI** — Phase 3 ships an
//!   empty linker, so a tool can perform pure-computation only. Phase 4
//!   (HWCA) will expose a tiny `gauss_yield` host fn for structured output;
//!   the schema gate sits between the WASM guest and the parent context.
//! * **Single-instance per invocation** — the module is freshly instantiated
//!   for every `exec` call; no mutable state survives between tool calls.

#![cfg(feature = "wasm-wasmi")]

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult};
use gauss_traits::{SandboxClass, SandboxLayer, SandboxOutcome, SandboxRequest, SandboxTrait};
use wasmi::{Engine, Module, Store};

/// Default fuel budget per invocation (~1M instructions). Configurable by
/// the tool manifest in Phase 4; Phase 3 uses the global default.
pub const DEFAULT_FUEL: u64 = 1_000_000;

/// WASM sandbox over wasmi 0.46.
pub struct WasmSandbox {
    /// The wasmi engine. Constructed once and shared across invocations.
    engine: Engine,
    /// Compiled module bytecode. Phase 3 ships a single registered module;
    /// Phase 4 introduces a `ToolId`-keyed registry.
    module: Module,
    /// Fuel budget per invocation.
    fuel: u64,
}

impl core::fmt::Debug for WasmSandbox {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `engine` and `module` are opaque wasmi handles; the only useful
        // field to print is the fuel budget. We list all fields here so the
        // `manual_debug_impl_includes_all_fields` lint accepts the impl.
        f.debug_struct("WasmSandbox")
            .field("fuel", &self.fuel)
            .field("engine", &"<wasmi::Engine>")
            .field("module", &"<wasmi::Module>")
            .finish()
    }
}

impl WasmSandbox {
    /// Compile a `.wasm` binary and wrap it in a sandbox.
    ///
    /// # Errors
    /// Returns [`GaussError::Internal`] if the bytecode fails to parse /
    /// validate (the engine refuses malformed modules; we forward the
    /// diagnostic verbatim).
    pub fn from_bytes(wasm: &[u8]) -> GaussResult<Self> {
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, wasm)
            .map_err(|e| GaussError::Internal(format!("wasm parse: {e}")))?;
        Ok(Self {
            engine,
            module,
            fuel: DEFAULT_FUEL,
        })
    }

    /// Override the per-invocation fuel budget. Convenience for tests.
    #[must_use]
    pub const fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = fuel;
        self
    }
}

#[async_trait]
impl SandboxTrait for WasmSandbox {
    fn class(&self, _cap: CapToken) -> SandboxClass {
        SandboxClass::L1
    }

    async fn exec(&self, request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        // wasmi is synchronous; we run on a blocking thread so we don't
        // monopolise the Tokio worker.
        let engine = self.engine.clone();
        let module = self.module.clone();
        let fuel = self.fuel;
        let tool_id = request.tool.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<i32, String> {
            let mut store: Store<()> = Store::new(&engine, ());
            store
                .set_fuel(fuel)
                .map_err(|e| format!("fuel init: {e}"))?;
            let linker = wasmi::Linker::<()>::new(&engine);
            let pre = linker
                .instantiate(&mut store, &module)
                .map_err(|e| format!("instantiate: {e}"))?;
            let instance = pre
                .ensure_no_start(&mut store)
                .map_err(|e| format!("start fn forbidden: {e}"))?;
            // Optional `main` export — return the i32 result if present.
            if let Ok(main) = instance.get_typed_func::<(), i32>(&store, "main") {
                let r = main
                    .call(&mut store, ())
                    .map_err(|e| format!("wasm trap: {e}"))?;
                Ok(r)
            } else {
                // No `main` export — module is valid but inert; return 0.
                Ok(0)
            }
        })
        .await
        .map_err(|e| GaussError::Internal(format!("wasm join: {e}")))?
        .map_err(GaussError::Io)?;

        tracing::trace!(tool = %tool_id.0, exit = result, "wasm sandbox executed");

        Ok(SandboxOutcome::new(
            Vec::new(),
            vec![SandboxLayer::Wasm],
            result,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::ToolId;

    /// The smallest valid WASM module: just a magic+version header plus an
    /// empty type section. wasmi accepts this as a no-op module.
    ///
    /// We use a hand-written "return 42 from `main`" module instead so we
    /// can verify the call path.
    fn return_42_module() -> Vec<u8> {
        // (module
        //   (func $main (result i32)
        //     i32.const 42)
        //   (export "main" (func $main)))
        //
        // Hand-assembled WAT-equivalent bytes:
        wat_to_wasm(
            r#"
            (module
              (func $main (result i32) i32.const 42)
              (export "main" (func $main)))
            "#,
        )
    }

    /// Compile WAT to WASM bytes using wasmi's built-in support if available;
    /// otherwise embed a precompiled module. wasmi 0.46 does not ship WAT
    /// support directly, so we keep a precompiled blob.
    fn wat_to_wasm(_wat: &str) -> Vec<u8> {
        // Precompiled output of the WAT above.
        // (module
        //   (func (result i32) i32.const 42)
        //   (export "main" (func 0)))
        vec![
            0x00, 0x61, 0x73, 0x6d, // magic
            0x01, 0x00, 0x00, 0x00, // version
            // Type section: 1 type, () -> (i32)
            0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f,
            // Function section: 1 function, type 0
            0x03, 0x02, 0x01, 0x00, // Export section: "main" -> func 0
            0x07, 0x08, 0x01, 0x04, 0x6d, 0x61, 0x69, 0x6e, 0x00, 0x00,
            // Code section: 1 function, i32.const 42 + end
            0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b,
        ]
    }

    /// An infinite-loop module to exercise the fuel-exhaustion path.
    fn infinite_loop_module() -> Vec<u8> {
        // (module
        //   (func $main (loop $L br $L))
        //   (export "main" (func $main)))
        vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // Type: () -> ()
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // Function: 1 func of type 0
            0x03, 0x02, 0x01, 0x00, // Export: "main" -> func 0
            0x07, 0x08, 0x01, 0x04, 0x6d, 0x61, 0x69, 0x6e, 0x00, 0x00,
            // Code: loop (block) br 0 end
            0x0a, 0x09, 0x01, 0x07, 0x00, 0x03, 0x40, // loop (void)
            0x0c, 0x00, // br 0
            0x0b, // end loop
            0x0b, // end func
        ]
    }

    #[tokio::test]
    async fn return_42_runs_and_reports_layer() {
        let bytes = return_42_module();
        let sb = WasmSandbox::from_bytes(&bytes).expect("module parses");
        let outcome = sb
            .exec(SandboxRequest::new(
                ToolId("answer".into()),
                CapToken::FILESYSTEM_READ,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
            .unwrap();
        // Note: the `main` export returns () in our infinite-loop test
        // and i32 in the return-42 test. We exposed only the i32 variant in
        // the executor; the return-42 path should yield 42 verbatim.
        assert_eq!(outcome.exit_code, 42);
        assert_eq!(outcome.layers_invoked, vec![SandboxLayer::Wasm]);
    }

    #[tokio::test]
    async fn fuel_exhaustion_traps() {
        let bytes = infinite_loop_module();
        // Use only the i32-returning fast-path skip for this test: the module
        // exports `main` with signature `() -> ()`, which fails the typed-func
        // lookup, so the executor returns 0 without ever entering the loop.
        // To actually test fuel exhaustion we lower the budget to a small
        // number AND need a module whose `main` returns i32 and contains an
        // infinite loop. Hand-roll one:
        // (module (func $main (result i32) (loop $L br $L) i32.const 0)
        //   (export "main" (func 0)))
        let infinite_i32 = vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7f, // () -> i32
            0x03, 0x02, 0x01, 0x00, 0x07, 0x08, 0x01, 0x04, 0x6d, 0x61, 0x69, 0x6e, 0x00, 0x00,
            0x0a, 0x0b, 0x01, 0x09, 0x00, 0x03, 0x40, 0x0c, 0x00, 0x0b, // loop / br 0 / end
            0x41, 0x00, // i32.const 0
            0x0b, // end func
        ];
        // Silence the "unused" warning on `bytes` from the earlier helper.
        let _ = bytes;
        let sb = WasmSandbox::from_bytes(&infinite_i32)
            .expect("module parses")
            .with_fuel(64); // tiny budget — must trap before the loop completes
        let err = sb
            .exec(SandboxRequest::new(
                ToolId("loop".into()),
                CapToken::FILESYSTEM_READ,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
            .expect_err("fuel exhaustion must trap");
        match err {
            GaussError::Io(msg) => assert!(
                msg.contains("trap") || msg.contains("fuel") || msg.contains("out of"),
                "expected fuel/trap diagnostic, got {msg}"
            ),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_bytecode() {
        let err = WasmSandbox::from_bytes(b"not wasm").unwrap_err();
        match err {
            GaussError::Internal(msg) => assert!(msg.contains("wasm parse")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }
}
