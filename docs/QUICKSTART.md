# Gauss-Aether — Quick Start Guide

A working embed from scratch in **fifteen minutes**. We'll build a
turn engine that runs a tool action, the SAG approves it, the receipt
chain signs the result, and the canvas reflects the outcome.

## Prerequisites

- Rust 1.83+ (the workspace MSRV; install via `rustup`).
- Optional: bubblewrap (`bwrap`) for the Linux L3a sandbox layer.
- Optional: SurrealDB binary if you want the persistent backend instead
  of the default `kv-mem`.

## Step 1 — Add Gauss-Aether to your Cargo.toml

```toml
[dependencies]
gauss-core      = { git = "https://github.com/rismanmattotorang/gauss-aether" }
gauss-kernel    = { git = "https://github.com/rismanmattotorang/gauss-aether" }
gauss-memory    = { git = "https://github.com/rismanmattotorang/gauss-aether" }
gauss-provider  = { git = "https://github.com/rismanmattotorang/gauss-aether" }
gauss-turn      = { git = "https://github.com/rismanmattotorang/gauss-aether" }
gauss-audit     = { git = "https://github.com/rismanmattotorang/gauss-aether" }
gauss-sag       = { git = "https://github.com/rismanmattotorang/gauss-aether" }
gauss-traits    = { git = "https://github.com/rismanmattotorang/gauss-aether" }
tokio           = { version = "1", features = ["macros", "rt-multi-thread"] }
serde_json      = "1"
```

> Once published to crates.io the `git = ...` lines become `version =
> "1.0"` — the trait surface is locked at 1.0 per ADR-0014.

## Step 2 — Wire the engine

```rust
use std::sync::Arc;
use gauss_audit::{Ed25519Signer, ReceiptSigner};
use gauss_core::{CapToken, Observation, ObservationSource, TaintLabel, TurnId};
use gauss_kernel::PrivilegedKernel;
use gauss_memory::SurrealMemory;
use gauss_provider::ToyProvider;
use gauss_sag::{ApprovalGate, AutoApprove, default_decision_table};
use gauss_turn::{TurnEngine, TurnInput};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Trinity Memory — SurrealDB embedded engine (kv-mem default).
    let memory = Arc::new(SurrealMemory::open_in_memory().await?);

    // 2. Privileged kernel with full capability grant. Production
    //    deployments call `contract(...)` to shrink the grant before
    //    accepting traffic.
    let kernel = Arc::new(PrivilegedKernel::new(CapToken::TOP));

    // 3. The policy. Real deployments wire an Anthropic / OpenAI / etc.
    //    `Provider` impl; we use the deterministic toy provider here.
    let provider = Arc::new(ToyProvider::always_text(
        "hello from gauss-aether!"
    ));

    // 4. The SAG approval gate. Production deployments wire a real
    //    `ApprovalSurface` (Telegram, Slack, …) here; we use AutoApprove
    //    so the quickstart doesn't need a human.
    let sag = Arc::new(ApprovalGate::new(
        default_decision_table(),
        AutoApprove::new("operator"),
    ));

    // 5. (Optional) signer — every committed turn produces a SignedReceipt.
    let signer = Arc::new(ReceiptSigner::<Ed25519Signer>::new(
        Ed25519Signer::from_seed([0xAB; 32]),
    ));
    // Note: the engine takes a type-erased `DynSigningBackend`; convert
    // via `Arc::new(ReceiptSigner::new(Box::new(Ed25519Signer::from_seed(...))))`
    // in production. See `gauss-turn::engine::DynSigningBackend`.

    // 6. Assemble.
    let engine = TurnEngine::new(kernel, memory.clone(), provider)
        .with_sag(sag);

    // 7. Build a turn input.
    let obs = Observation::new(
        ObservationSource::User { channel: "quickstart".into() },
        TaintLabel::User,
        serde_json::json!({"body": "hello"}),
    );
    let input = TurnInput { id: TurnId::new(1), obs };

    // 8. Run.
    let summary = engine.run_turn(input).await?;

    println!("✓ committed {} actions", summary.action_count);
    println!("✓ chain head = 0x{}", hex::encode(summary.chain_head.digest));
    println!("✓ chain length = {}", summary.chain_head.length);
    if !summary.sag_decisions.is_empty() {
        println!("✓ SAG decisions: {:#?}", summary.sag_decisions);
    }
    Ok(())
}
```

Run it:

```bash
cargo run
```

Expected output:

```text
✓ committed 1 actions
✓ chain head = 0x<hex>
✓ chain length = 1
```

## Step 3 — Enable the composite sandbox

Tool actions that need filesystem / network access run through the
Phase-3 composite sandbox. Add `gauss-sandbox` and wire it via
`TurnEngine::with_all`:

```rust
use gauss_sandbox::{CompositeSandbox, WasmSandbox};
use gauss_traits::SandboxTrait;

let wasm = WasmSandbox::from_bytes(&include_bytes!("../my_tool.wasm")[..])?;
let sandbox: Arc<dyn SandboxTrait> = Arc::new(CompositeSandbox::wasm_only(wasm));

let engine = TurnEngine::with_all(
    kernel.clone(),
    memory.clone(),
    provider.clone(),
    sandbox,
    signer.clone(),
).with_sag(sag.clone());
```

The kernel's `min_sandbox_for(cap)` picks the minimum class for each
capability; the composite refuses tools whose declared cap exceeds the
configured layers (paper §IX.B Theorem T10).

## Step 4 — Approve via a real surface

Swap `AutoApprove` for a `ChannelSurface` to drive approval through an
`mpsc` channel — or wire a production adapter (Telegram, Slack, etc.):

```rust
use gauss_sag::{ApprovalDecision, ChannelSurface};

let (surface, mut req_rx) = ChannelSurface::new(16);
let sender = surface.sender();

// Spawn the operator side — in production this is a chat-bot adapter.
tokio::spawn(async move {
    while let Some(req) = req_rx.recv().await {
        println!("approve: {:?}?", req.action);
        let _ = sender.send(ApprovalDecision::Approved {
            approver: "ops".into(),
        }).await;
    }
});

let sag = Arc::new(ApprovalGate::new(default_decision_table(), surface));
```

Set a custom deadline via `.with_deadline(...)`; defaults to 5 min
per SPECS §XI.C.

## Step 5 — Verify the chain

The Phase-5 verifier API runs offline:

```rust
use gauss_audit::{verify_chain, verify_receipt};

let receipt = summary.receipt.as_ref().expect("signer wired");
verify_receipt(receipt, &canonical_payload_bytes)?;
```

For whole-chain verification:

```rust
verify_chain(&receipts, &payloads, Some(expected_final_head))?;
```

## Step 6 — Surface a live canvas

The Phase-9 canvas is the typed widget tree user-facing surfaces
consume:

```rust
use gauss_canvas::{Canvas, CanvasNode, CanvasUpdate, InMemoryCanvas, NodeId, WidgetKind};

let canvas = InMemoryCanvas::default();
let mut subscriber = canvas.subscribe();

canvas.apply(CanvasUpdate::Insert {
    node: CanvasNode::leaf(
        NodeId::new("turn-1"),
        WidgetKind::Text,
        serde_json::json!({ "body": "turn committed" }),
    ),
    parent: None,
}).await?;

while let Ok(update) = subscriber.recv().await {
    println!("canvas event: {update:?}");
}
```

## Step 7 — Wire the gateway

`gauss-gateway` ships the wire shapes; the actual `axum` server is a
Phase-11 additive crate. To roll your own server:

```rust
use gauss_gateway::{TurnRequest, TurnResponse};

async fn handle_turn(req: TurnRequest, engine: Arc<TurnEngine<...>>) -> TurnResponse {
    let obs = Observation::new(/* ... */);
    let summary = engine.run_turn(TurnInput { id: req.turn_id, obs }).await.unwrap();
    TurnResponse::ok(
        req.turn_id,
        /* actions */ vec![],
        hex::encode(summary.chain_head.digest),
        summary.chain_head.length,
    )
}
```

## Where to next

- [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) — crate-by-crate tour.
- [`docs/CONTRIBUTING.md`](CONTRIBUTING.md) — plugin authoring + `specT` style guide.
- [`docs/SECURITY.md`](SECURITY.md) — threat model.
- [`docs/adr/`](adr/) — 16 ADRs documenting every architecture decision.

You can also poke at the conformance suite — it's the executable
contract for every axiom and theorem:

```bash
cargo test --workspace --no-fail-fast
```
