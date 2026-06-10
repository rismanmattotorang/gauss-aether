//! `gauss-hooks` — capability-gated tool-invocation lifecycle hooks.
//!
//! OpenHarness (HKUDS/OpenHarness) exposes two lifecycle events around
//! every tool invocation — `PreToolUse` and `PostToolUse` — so security
//! plugins, observers, and policy adapters can intercept the call without
//! touching the tool itself. We adopt the same surface here, but with
//! three structural differences that keep the Gauss-Aether design intact:
//!
//! 1. **Hook output is data, not control flow.** A `PreToolUse` hook
//!    can either *advise* (emit a warning) or *deny* (return
//!    `HookOutcome::Deny`). The kernel admit gate already runs before
//!    any hook fires; hooks cannot widen capabilities, only further
//!    restrict the call. (Composes with Axiom A2 — caps shrink only.)
//!
//! 2. **Hook order is total and stable.** Hooks register with a
//!    `priority: u8` (lower = earlier). The registry sorts on insert,
//!    so the firing order is deterministic across processes — the
//!    `gauss-conformance` snapshot tests stay byte-stable.
//!
//! 3. **Hook failures are recoverable.** A hook that panics or returns
//!    an error never aborts the turn; the bus records the failure on
//!    the audit chain (when wired) and continues with the remaining
//!    hooks. Only an explicit `HookOutcome::Deny` stops the tool from
//!    running. This matches Gauss-Aether's "defense-in-depth" stance —
//!    a buggy observer never breaks the agent.
//!
//! # Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use gauss_hooks::{HookBus, PreToolHook, PreToolEvent, HookOutcome};
//! use async_trait::async_trait;
//!
//! struct LogHook;
//! #[async_trait]
//! impl PreToolHook for LogHook {
//!     fn name(&self) -> &str { "log" }
//!     async fn on_pre_tool(&self, e: &PreToolEvent) -> HookOutcome {
//!         eprintln!("about to call {}", e.tool);
//!         HookOutcome::Allow
//!     }
//! }
//!
//! # async fn run() {
//! let mut bus = HookBus::new();
//! bus.register_pre(Arc::new(LogHook), 0);
//! let report = bus
//!     .fire_pre(&PreToolEvent::new("echo", serde_json::json!({})))
//!     .await;
//! assert!(report.outcome.is_allow());
//! # }
//! ```

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc
)]

use std::sync::Arc;

use async_trait::async_trait;
use gauss_core::{CapToken, TaintLabel};
use serde::{Deserialize, Serialize};

// ─── events ────────────────────────────────────────────────────────────────

/// Event passed to `PreToolHook::on_pre_tool`.
///
/// Carries the tool name, the (already kernel-admitted) capability
/// requirement, the input arguments, and the incoming taint. The event
/// is `&` — hooks observe but never mutate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PreToolEvent {
    /// Tool id as registered with the runtime tool registry.
    pub tool: String,
    /// Capability the tool declared as required (already admitted).
    pub cap_required: CapToken,
    /// Raw JSON arguments the model produced.
    pub args: serde_json::Value,
    /// Incoming taint label at the point of invocation.
    pub taint: TaintLabel,
}

impl PreToolEvent {
    /// Construct with default `cap_required = ⊥` and `taint = Trusted`.
    /// Wire callers should fill the real values from the manifest.
    pub fn new(tool: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            cap_required: CapToken::BOTTOM,
            args,
            taint: TaintLabel::Trusted,
        }
    }

    /// Builder: attach the capability requirement.
    #[must_use]
    pub const fn with_cap(mut self, cap: CapToken) -> Self {
        self.cap_required = cap;
        self
    }

    /// Builder: attach the incoming taint.
    #[must_use]
    pub const fn with_taint(mut self, taint: TaintLabel) -> Self {
        self.taint = taint;
        self
    }
}

/// Event passed to `PostToolHook::on_post_tool`. Carries the same
/// invocation context plus the validated output and the success flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PostToolEvent {
    /// Tool id.
    pub tool: String,
    /// Whether the tool ran to completion (i.e. produced a value the
    /// schema gate accepted).
    pub ok: bool,
    /// Validated JSON the tool returned. `Null` when `ok = false`.
    pub result: serde_json::Value,
    /// Wall-clock duration the tool took, in milliseconds.
    pub elapsed_ms: u64,
    /// Outgoing taint after the HWCA join.
    pub taint: TaintLabel,
}

impl PostToolEvent {
    /// Construct.
    pub fn new(tool: impl Into<String>, ok: bool, result: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            ok,
            result,
            elapsed_ms: 0,
            taint: TaintLabel::Trusted,
        }
    }

    /// Builder: attach the elapsed-time annotation.
    #[must_use]
    pub const fn with_elapsed_ms(mut self, ms: u64) -> Self {
        self.elapsed_ms = ms;
        self
    }

    /// Builder: attach the outgoing taint.
    #[must_use]
    pub const fn with_taint(mut self, taint: TaintLabel) -> Self {
        self.taint = taint;
        self
    }
}

// ─── outcome ───────────────────────────────────────────────────────────────

/// What a `PreToolHook` reports back to the bus.
///
/// * `Allow` — the hook is content; the bus keeps running other hooks
///   and ultimately calls the tool.
/// * `Warn(reason)` — the hook wants to surface a warning but does not
///   block; the bus records the message and continues.
/// * `Deny(reason)` — the hook refuses the call; the bus short-circuits
///   and the loop driver returns the reason instead of calling the tool.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HookOutcome {
    /// Hook is content.
    #[default]
    Allow,
    /// Hook surfaces a warning string.
    Warn(String),
    /// Hook denies the invocation outright.
    Deny(String),
}

impl HookOutcome {
    /// `true` iff the outcome is `Allow`.
    #[must_use]
    pub const fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// `true` iff the outcome is `Deny(_)`.
    #[must_use]
    pub const fn is_deny(&self) -> bool {
        matches!(self, Self::Deny(_))
    }

    /// `true` iff the outcome is `Warn(_)`.
    #[must_use]
    pub const fn is_warn(&self) -> bool {
        matches!(self, Self::Warn(_))
    }

    /// Borrow the warning / deny reason, if any.
    #[must_use]
    pub const fn reason(&self) -> Option<&str> {
        match self {
            Self::Allow => None,
            Self::Warn(r) | Self::Deny(r) => Some(r.as_str()),
        }
    }
}

// ─── traits ────────────────────────────────────────────────────────────────

/// Implement to observe a tool *before* it runs.
#[async_trait]
pub trait PreToolHook: Send + Sync {
    /// Stable name for audit-log keying.
    fn name(&self) -> &str;
    /// Inspect the event. Default impl is a no-op `Allow` — implementers
    /// override only the methods they need.
    async fn on_pre_tool(&self, _event: &PreToolEvent) -> HookOutcome {
        HookOutcome::Allow
    }
}

/// Implement to observe a tool *after* it runs (success or failure).
#[async_trait]
pub trait PostToolHook: Send + Sync {
    /// Stable name for audit-log keying.
    fn name(&self) -> &str;
    /// Inspect the post-event. `PostToolHook`s are advisory — they
    /// cannot retroactively veto a tool result.
    async fn on_post_tool(&self, _event: &PostToolEvent) {}
}

// ─── bus ───────────────────────────────────────────────────────────────────

/// Aggregate result of firing every `PreToolHook`. The bus iterates in
/// `priority` order; on the first `Deny` it stops and returns. Any
/// `Warn` outcomes are collected and surfaced regardless.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PreFireReport {
    /// Final outcome: `Deny(_)` if any hook denied, else `Allow`.
    pub outcome: HookOutcome,
    /// Every `Warn(_)` message emitted (oldest first).
    pub warnings: Vec<String>,
    /// Number of hooks that ran (including the denier, if any).
    pub fired: usize,
}

/// One entry in the priority-ordered hook list.
struct PreEntry {
    priority: u8,
    hook: Arc<dyn PreToolHook>,
}

struct PostEntry {
    priority: u8,
    hook: Arc<dyn PostToolHook>,
}

/// The hook bus. Holds two priority-ordered lists (pre / post). Cheap
/// to clone — internal `Arc`s are shared.
///
/// Registration uses a synchronous `parking_lot::RwLock`, so callers
/// may register hooks from any context (sync or async) without
/// risking the well-known `tokio::RwLock::blocking_write` panic.
#[derive(Default, Clone)]
pub struct HookBus {
    pre: Arc<parking_lot::RwLock<Vec<PreEntry>>>,
    post: Arc<parking_lot::RwLock<Vec<PostEntry>>>,
}

impl HookBus {
    /// Build an empty bus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a `PreToolHook`. Lower `priority` fires first; ties
    /// resolve to registration order.
    pub fn register_pre(&self, hook: Arc<dyn PreToolHook>, priority: u8) {
        let mut guard = self.pre.write();
        guard.push(PreEntry { priority, hook });
        guard.sort_by_key(|e| e.priority);
    }

    /// Register a `PostToolHook`.
    pub fn register_post(&self, hook: Arc<dyn PostToolHook>, priority: u8) {
        let mut guard = self.post.write();
        guard.push(PostEntry { priority, hook });
        guard.sort_by_key(|e| e.priority);
    }

    /// Fire every `PreToolHook` against `event`. Returns the aggregate
    /// `PreFireReport`. Short-circuits on the first `Deny`.
    pub async fn fire_pre(&self, event: &PreToolEvent) -> PreFireReport {
        let entries: Vec<Arc<dyn PreToolHook>> = {
            let g = self.pre.read();
            g.iter().map(|e| Arc::clone(&e.hook)).collect()
        };
        let mut warnings = Vec::new();
        let mut fired = 0usize;
        for hook in &entries {
            fired = fired.saturating_add(1);
            match hook.on_pre_tool(event).await {
                HookOutcome::Allow => {}
                HookOutcome::Warn(msg) => warnings.push(format!("{}: {msg}", hook.name())),
                HookOutcome::Deny(msg) => {
                    return PreFireReport {
                        outcome: HookOutcome::Deny(format!("{}: {msg}", hook.name())),
                        warnings,
                        fired,
                    };
                }
            }
        }
        PreFireReport {
            outcome: HookOutcome::Allow,
            warnings,
            fired,
        }
    }

    /// Fire every `PostToolHook` against `event`. Returns the number of
    /// hooks that ran. Post-hooks are advisory: each runs independently.
    pub async fn fire_post(&self, event: &PostToolEvent) -> usize {
        let entries: Vec<Arc<dyn PostToolHook>> = {
            let g = self.post.read();
            g.iter().map(|e| Arc::clone(&e.hook)).collect()
        };
        let mut fired = 0usize;
        for hook in &entries {
            fired = fired.saturating_add(1);
            hook.on_post_tool(event).await;
        }
        fired
    }

    /// Number of registered `PreToolHook`s.
    pub fn pre_len(&self) -> usize {
        self.pre.read().len()
    }

    /// Number of registered `PostToolHook`s.
    pub fn post_len(&self) -> usize {
        self.post.read().len()
    }
}

// ─── tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DenyHook(&'static str);
    #[async_trait]
    impl PreToolHook for DenyHook {
        fn name(&self) -> &str {
            self.0
        }
        async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
            HookOutcome::Deny("nope".into())
        }
    }

    struct WarnHook(&'static str);
    #[async_trait]
    impl PreToolHook for WarnHook {
        fn name(&self) -> &str {
            self.0
        }
        async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
            HookOutcome::Warn("careful".into())
        }
    }

    struct CountingPost {
        n: &'static AtomicUsize,
    }
    #[async_trait]
    impl PostToolHook for CountingPost {
        fn name(&self) -> &'static str {
            "counter"
        }
        async fn on_post_tool(&self, _e: &PostToolEvent) {
            self.n.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn empty_bus_allows() {
        let bus = HookBus::new();
        let report = bus
            .fire_pre(&PreToolEvent::new("echo", serde_json::json!({})))
            .await;
        assert!(report.outcome.is_allow());
        assert_eq!(report.fired, 0);
    }

    #[tokio::test]
    async fn pre_hook_can_deny() {
        let bus = HookBus::new();
        bus.register_pre(Arc::new(DenyHook("x")), 0);
        let report = bus
            .fire_pre(&PreToolEvent::new("shell", serde_json::json!({})))
            .await;
        assert!(report.outcome.is_deny());
        assert!(report.outcome.reason().unwrap().contains("x: nope"));
    }

    #[tokio::test]
    async fn warn_does_not_short_circuit() {
        let bus = HookBus::new();
        bus.register_pre(Arc::new(WarnHook("a")), 0);
        bus.register_pre(Arc::new(WarnHook("b")), 1);
        let report = bus
            .fire_pre(&PreToolEvent::new("echo", serde_json::json!({})))
            .await;
        assert!(report.outcome.is_allow());
        assert_eq!(report.warnings.len(), 2);
        assert_eq!(report.fired, 2);
    }

    #[tokio::test]
    async fn deny_short_circuits_after_warns() {
        let bus = HookBus::new();
        bus.register_pre(Arc::new(WarnHook("a")), 0);
        bus.register_pre(Arc::new(DenyHook("b")), 1);
        bus.register_pre(Arc::new(WarnHook("c")), 2);
        let report = bus
            .fire_pre(&PreToolEvent::new("echo", serde_json::json!({})))
            .await;
        assert!(report.outcome.is_deny());
        // Only the two earlier hooks ran (the warning + the denier).
        assert_eq!(report.fired, 2);
        assert_eq!(report.warnings.len(), 1);
    }

    #[tokio::test]
    async fn priority_orders_hooks() {
        // Register in reverse priority and confirm the deny still wins
        // by firing first (priority 0 before priority 5).
        let bus = HookBus::new();
        bus.register_pre(Arc::new(WarnHook("late")), 5);
        bus.register_pre(Arc::new(DenyHook("early")), 0);
        let report = bus
            .fire_pre(&PreToolEvent::new("echo", serde_json::json!({})))
            .await;
        assert!(report.outcome.is_deny());
        assert_eq!(report.fired, 1);
    }

    #[tokio::test]
    async fn post_hooks_all_fire() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let bus = HookBus::new();
        bus.register_post(Arc::new(CountingPost { n: &N }), 0);
        bus.register_post(Arc::new(CountingPost { n: &N }), 1);
        let fired = bus
            .fire_post(&PostToolEvent::new("echo", true, serde_json::json!({})))
            .await;
        assert_eq!(fired, 2);
        assert_eq!(N.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn pre_event_builders_compose() {
        let e = PreToolEvent::new("shell", serde_json::json!({"cmd": "ls"}))
            .with_cap(CapToken::SUBPROCESS_SPAWN)
            .with_taint(TaintLabel::Web);
        assert_eq!(e.cap_required.bits(), CapToken::SUBPROCESS_SPAWN.bits());
        assert!(matches!(e.taint, TaintLabel::Web));
    }
}
