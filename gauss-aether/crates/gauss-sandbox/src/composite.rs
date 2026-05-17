//! Composite sandbox executor (paper §IX, Theorem T10).
//!
//! The composite holds an ordered list of inner layers and dispatches every
//! request through all layers whose bit is set in the capability's required
//! [`SandboxClass`]. The conformance suite (`CONF-T10-*`) verifies that the
//! `layers_invoked` set returned by the composite covers the required class.

use async_trait::async_trait;
use gauss_core::{CapToken, GaussError, GaussResult, RefusalReason};
use gauss_traits::{
    min_sandbox_for, SandboxClass, SandboxLayer, SandboxOutcome, SandboxRequest, SandboxTrait,
};

use crate::wasm::WasmSandbox;

/// Composite sandbox combining one or more inner layers.
///
/// Construction is through [`CompositeSandboxBuilder`], which enforces the
/// rule that the WASM layer (L1) is always present.
pub struct CompositeSandbox {
    layers: Vec<Box<dyn SandboxTrait>>,
}

impl core::fmt::Debug for CompositeSandbox {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CompositeSandbox")
            .field("layer_count", &self.layers.len())
            .finish()
    }
}

/// Builder for [`CompositeSandbox`]. Starts with the mandatory L1 WASM layer.
pub struct CompositeSandboxBuilder {
    layers: Vec<Box<dyn SandboxTrait>>,
}

impl CompositeSandboxBuilder {
    /// Begin a build with the supplied WASM sandbox as L1.
    #[must_use]
    pub fn with_wasm(wasm: WasmSandbox) -> Self {
        Self {
            layers: vec![Box::new(wasm)],
        }
    }

    /// Append another layer (Landlock / Bubblewrap / Seccomp / Seatbelt).
    #[must_use]
    pub fn push<L: SandboxTrait + 'static>(mut self, layer: L) -> Self {
        self.layers.push(Box::new(layer));
        self
    }

    /// Finalise the composite.
    #[must_use]
    pub fn build(self) -> CompositeSandbox {
        CompositeSandbox {
            layers: self.layers,
        }
    }
}

impl CompositeSandbox {
    /// Convenience: a single-layer L1-only composite.
    #[must_use]
    pub fn wasm_only(wasm: WasmSandbox) -> Self {
        CompositeSandboxBuilder::with_wasm(wasm).build()
    }

    /// Number of layers in the stack.
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

#[async_trait]
impl SandboxTrait for CompositeSandbox {
    fn class(&self, cap: CapToken) -> SandboxClass {
        // The composite's effective class is the UNION of every inner layer's
        // class for the same cap. (The conformance check then asks: is this
        // union ⊇ min_sandbox_for(cap)?)
        let mut acc = SandboxClass::NONE;
        for layer in &self.layers {
            acc = acc.union(layer.class(cap));
        }
        acc
    }

    async fn exec(&self, request: SandboxRequest) -> GaussResult<SandboxOutcome> {
        let required = min_sandbox_for(request.cap);
        let provided = self.class(request.cap);
        if !covers(provided, required) {
            // Insufficient stack for the requested cap class — refuse hard.
            // This is a capability-side denial: the operator provisioned a
            // weaker sandbox than the cap demands. The taint side does not
            // apply at this layer (the kernel already approved the taint).
            return Err(GaussError::Denied {
                reason: RefusalReason::cap_only(),
            });
        }

        // Phase 3: we run every layer's `exec` in source order, collecting
        // their `layers_invoked` and propagating the first failure verbatim.
        // The WASM layer always runs first because it is the only layer that
        // actually executes guest code; the OS layers below confine the host
        // process the WASM interpreter runs inside. In Phase 4 the layered
        // exec moves into a single subprocess that bwrap launches; Phase 3
        // keeps them as composable units for testability.
        let mut layers_invoked: Vec<SandboxLayer> = Vec::new();
        let mut stdout: Vec<u8> = Vec::new();
        let mut exit_code: i32 = 0;
        for layer in &self.layers {
            let outcome = layer.exec(request.clone()).await?;
            layers_invoked.extend(outcome.layers_invoked);
            // The WASM layer is authoritative for stdout / exit_code.
            if stdout.is_empty() {
                stdout = outcome.stdout;
            }
            // Non-zero exit short-circuits the rest of the stack: a failure
            // in any layer is a failure of the composite.
            if outcome.exit_code != 0 {
                exit_code = outcome.exit_code;
                break;
            }
        }

        // Sanity check: the layers we actually invoked MUST cover the
        // required class. (This guards against an implementor that reports
        // class(cap) but doesn't actually invoke its layer at exec time.)
        let invoked_class = layers_to_class(&layers_invoked);
        if !covers(invoked_class, required) {
            return Err(GaussError::Internal(format!(
                "composite sandbox layer mismatch: required {:08b}, invoked {:08b}",
                required.bits(),
                invoked_class.bits(),
            )));
        }

        Ok(SandboxOutcome::new(stdout, layers_invoked, exit_code))
    }
}

/// True iff `provided` covers every bit in `required`.
#[must_use]
const fn covers(provided: SandboxClass, required: SandboxClass) -> bool {
    (provided.bits() & required.bits()) == required.bits()
}

/// Helper: project a layer-invocation list back to a `SandboxClass`.
fn layers_to_class(layers: &[SandboxLayer]) -> SandboxClass {
    let mut acc = SandboxClass::NONE;
    for layer in layers {
        match layer {
            SandboxLayer::Wasm => acc = acc.union(SandboxClass::L1),
            SandboxLayer::Landlock => {
                // Landlock alone gives us L1 | L2 (it adds bit 1 to L1).
                acc = acc.union(SandboxClass::L2);
            }
            SandboxLayer::Namespace | SandboxLayer::Seccomp => acc = acc.union(SandboxClass::L3),
            SandboxLayer::Tee => acc = acc.union(SandboxClass::L4),
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::WasmSandbox;
    use gauss_core::ToolId;

    // The smallest WASM module that exports a main returning i32 = 0.
    fn return_0_module() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x05, 0x01, 0x60, 0x00, 0x01,
            0x7f, 0x03, 0x02, 0x01, 0x00, 0x07, 0x08, 0x01, 0x04, 0x6d, 0x61, 0x69, 0x6e, 0x00,
            0x00, 0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x00, 0x0b,
        ]
    }

    #[tokio::test]
    async fn wasm_only_runs_a_read_only_cap() {
        let wasm = WasmSandbox::from_bytes(&return_0_module()).unwrap();
        let composite = CompositeSandbox::wasm_only(wasm);
        let out = composite
            .exec(SandboxRequest::new(
                ToolId("read".into()),
                CapToken::FILESYSTEM_READ,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.layers_invoked, vec![SandboxLayer::Wasm]);
    }

    #[tokio::test]
    async fn wasm_only_refuses_a_write_cap_that_requires_l2() {
        let wasm = WasmSandbox::from_bytes(&return_0_module()).unwrap();
        let composite = CompositeSandbox::wasm_only(wasm);
        // Network GET requires L2 (per min_sandbox_for); WASM-only is L1.
        let err = composite
            .exec(SandboxRequest::new(
                ToolId("fetch".into()),
                CapToken::NETWORK_GET,
                serde_json::Value::Null,
                Vec::new(),
            ))
            .await
            .expect_err("L2 required, only L1 provided — must deny");
        match err {
            GaussError::Denied { reason } => assert!(reason.cap_bit),
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[test]
    fn covers_helper_is_correct() {
        // L3 ⊇ L2 ⊇ L1.
        assert!(covers(SandboxClass::L1, SandboxClass::L1));
        assert!(covers(SandboxClass::L2, SandboxClass::L1));
        assert!(covers(SandboxClass::L3, SandboxClass::L2));
        assert!(covers(SandboxClass::L4, SandboxClass::L3));
        // L1 does NOT cover L2.
        assert!(!covers(SandboxClass::L1, SandboxClass::L2));
        assert!(!covers(SandboxClass::L2, SandboxClass::L3));
    }
}
