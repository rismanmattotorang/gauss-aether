//! Plugin → `HookBus` runtime wiring.
//!
//! [`PluginManifest`](crate::PluginManifest) declares hooks the
//! plugin promises to install (`hooks: Vec<HookDeclaration>`). This
//! module ships the bridge: a [`HookFactory`] trait that resolves
//! each declaration to a concrete callback, plus
//! [`crate::PluginRegistry::register_hooks`] which walks every loaded
//! plugin and registers each declared hook with a live
//! [`gauss_hooks::HookBus`] using the declared priority.
//!
//! ## Honouring `target_tools`
//!
//! When `HookDeclaration::target_tools` is non-empty, the hook should
//! only see events whose `tool` is in the list. The bus has no
//! per-tool filter of its own, so we wrap the factory-supplied hook
//! in a [`TargetFilterPreHook`] / [`TargetFilterPostHook`] adapter
//! that returns `Allow` (or skips, respectively) for unrelated tools.
//!
//! ## Fail-closed on unknown ids
//!
//! [`HookFactory::build`] returns `Err` for unknown hook ids; the
//! registration walk surfaces the error and refuses partial
//! registration. Operators see a clean diagnostic instead of a
//! silent half-loaded plugin.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::missing_errors_doc
)]

use std::sync::Arc;

use async_trait::async_trait;
use gauss_hooks::{HookBus, HookOutcome, PostToolEvent, PostToolHook, PreToolEvent, PreToolHook};

use crate::{HookDeclaration, HookLifecycle, PluginError, PluginRegistry, PluginResult};

/// One built hook — the concrete callback the factory returned for a
/// given [`HookDeclaration`]. The variant selects which bus list the
/// registration walk appends to.
pub enum BuiltHook {
    /// Pre-tool hook (may `Warn` or `Deny`).
    Pre(Arc<dyn PreToolHook>),
    /// Post-tool hook (advisory).
    Post(Arc<dyn PostToolHook>),
}

impl BuiltHook {
    /// `true` if this is a pre-tool hook.
    #[must_use]
    pub const fn is_pre(&self) -> bool {
        matches!(self, Self::Pre(_))
    }
    /// `true` if this is a post-tool hook.
    #[must_use]
    pub const fn is_post(&self) -> bool {
        matches!(self, Self::Post(_))
    }
}

/// Resolves a [`HookDeclaration`] to a concrete callback.
///
/// Each plugin implementation supplies one of these. The
/// [`PluginRegistry::register_hooks`] walk calls
/// `build(decl)` for every declaration on every loaded plugin and
/// registers the result with the bus.
///
/// Implementations are expected to *fail closed* — return
/// [`PluginError::Backend`] for ids they don't recognise so the
/// caller can refuse to start with a half-loaded plugin set.
pub trait HookFactory: Send + Sync {
    /// Resolve one declaration. The returned hook's lifecycle MUST
    /// match `decl.lifecycle`; the registration walk verifies and
    /// refuses to register mismatched results.
    fn build(&self, decl: &HookDeclaration) -> PluginResult<BuiltHook>;
}

// ─── target-tool filters ──────────────────────────────────────────────────

/// Wraps a [`PreToolHook`] so it only fires for tools in
/// `targets`. For other tools the wrapper returns
/// [`HookOutcome::Allow`] without calling the inner hook — the
/// inner cap-check / policy never sees events it doesn't care
/// about, which keeps audit-log noise down.
pub struct TargetFilterPreHook {
    inner: Arc<dyn PreToolHook>,
    targets: Vec<String>,
    label: String,
}

impl TargetFilterPreHook {
    /// Build the filter wrapper. `targets` must be non-empty; the
    /// caller short-circuits to the raw hook when the declaration's
    /// `target_tools` is empty.
    pub fn new(inner: Arc<dyn PreToolHook>, targets: Vec<String>) -> Self {
        let label = format!("{}[targets={}]", inner.name(), targets.join(","));
        Self {
            inner,
            targets,
            label,
        }
    }
}

#[async_trait]
impl PreToolHook for TargetFilterPreHook {
    fn name(&self) -> &str {
        &self.label
    }
    async fn on_pre_tool(&self, event: &PreToolEvent) -> HookOutcome {
        if self.targets.iter().any(|t| t == &event.tool) {
            self.inner.on_pre_tool(event).await
        } else {
            HookOutcome::Allow
        }
    }
}

/// Wraps a [`PostToolHook`] so it only fires for tools in `targets`.
pub struct TargetFilterPostHook {
    inner: Arc<dyn PostToolHook>,
    targets: Vec<String>,
    label: String,
}

impl TargetFilterPostHook {
    /// Build the filter wrapper. `targets` must be non-empty.
    pub fn new(inner: Arc<dyn PostToolHook>, targets: Vec<String>) -> Self {
        let label = format!("{}[targets={}]", inner.name(), targets.join(","));
        Self {
            inner,
            targets,
            label,
        }
    }
}

#[async_trait]
impl PostToolHook for TargetFilterPostHook {
    fn name(&self) -> &str {
        &self.label
    }
    async fn on_post_tool(&self, event: &PostToolEvent) {
        if self.targets.iter().any(|t| t == &event.tool) {
            self.inner.on_post_tool(event).await;
        }
    }
}

// ─── registry method ──────────────────────────────────────────────────────

/// Summary of a registration walk.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HookRegistrationReport {
    /// How many `PreToolHook`s were registered.
    pub pre_registered: usize,
    /// How many `PostToolHook`s were registered.
    pub post_registered: usize,
    /// How many declarations were skipped because their plugin was
    /// loaded with `enabled = false`.
    pub skipped_disabled: usize,
}

impl PluginRegistry {
    /// Walk every loaded plugin and register its declared hooks with
    /// the given [`HookBus`]. The `factory` resolves declaration ids
    /// to concrete callbacks.
    ///
    /// Skips plugins where `enabled == false`. Stops on the first
    /// factory failure so the caller never gets a half-loaded bus.
    /// Each registered hook honours the declaration's `priority` and
    /// `target_tools` filter.
    ///
    /// # Errors
    ///
    /// * [`PluginError::Backend`] if the factory rejects an id.
    /// * [`PluginError::InvalidManifest`] if a returned [`BuiltHook`]
    ///   doesn't match the declaration's lifecycle.
    pub fn register_hooks(
        &self,
        bus: &HookBus,
        factory: &dyn HookFactory,
    ) -> PluginResult<HookRegistrationReport> {
        let mut report = HookRegistrationReport::default();
        for plugin in self.list() {
            if !plugin.enabled {
                report.skipped_disabled = report
                    .skipped_disabled
                    .saturating_add(plugin.manifest.hooks.len());
                continue;
            }
            for decl in &plugin.manifest.hooks {
                let built = factory.build(decl)?;
                match (&built, decl.lifecycle) {
                    (BuiltHook::Pre(_), HookLifecycle::PreTool)
                    | (BuiltHook::Post(_), HookLifecycle::PostTool) => {}
                    _ => {
                        return Err(PluginError::InvalidManifest(format!(
                            "hook `{}` lifecycle mismatch: declared {:?} but factory returned {}",
                            decl.id,
                            decl.lifecycle,
                            if built.is_pre() { "Pre" } else { "Post" },
                        )));
                    }
                }
                match built {
                    BuiltHook::Pre(hook) => {
                        let registered: Arc<dyn PreToolHook> = if decl.target_tools.is_empty() {
                            hook
                        } else {
                            Arc::new(TargetFilterPreHook::new(hook, decl.target_tools.clone()))
                        };
                        bus.register_pre(registered, decl.priority);
                        report.pre_registered = report.pre_registered.saturating_add(1);
                    }
                    BuiltHook::Post(hook) => {
                        let registered: Arc<dyn PostToolHook> = if decl.target_tools.is_empty() {
                            hook
                        } else {
                            Arc::new(TargetFilterPostHook::new(hook, decl.target_tools.clone()))
                        };
                        bus.register_post(registered, decl.priority);
                        report.post_registered = report.post_registered.saturating_add(1);
                    }
                }
            }
        }
        Ok(report)
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LoadedPlugin, PluginKind, PluginManifest};
    use gauss_core::CapToken;
    use gauss_hooks::HookOutcome;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── fixtures ─────────────────────────────────────────────────────────

    struct NoOpPre(&'static str);
    #[async_trait]
    impl PreToolHook for NoOpPre {
        fn name(&self) -> &str {
            self.0
        }
        async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
            HookOutcome::Allow
        }
    }

    struct DenyAll(&'static str);
    #[async_trait]
    impl PreToolHook for DenyAll {
        fn name(&self) -> &str {
            self.0
        }
        async fn on_pre_tool(&self, _e: &PreToolEvent) -> HookOutcome {
            HookOutcome::Deny("nope".into())
        }
    }

    struct CountingPost {
        n: &'static AtomicUsize,
        name: &'static str,
    }
    #[async_trait]
    impl PostToolHook for CountingPost {
        fn name(&self) -> &str {
            self.name
        }
        async fn on_post_tool(&self, _e: &PostToolEvent) {
            self.n.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Test factory: maps "noop" → NoOpPre, "deny" → DenyAll,
    /// "counter" → CountingPost. Anything else → Backend error.
    struct TestFactory {
        counter: &'static AtomicUsize,
    }
    impl HookFactory for TestFactory {
        fn build(&self, decl: &HookDeclaration) -> PluginResult<BuiltHook> {
            match decl.id.as_str() {
                "noop" => Ok(BuiltHook::Pre(Arc::new(NoOpPre("noop")))),
                "deny" => Ok(BuiltHook::Pre(Arc::new(DenyAll("deny")))),
                "counter" => Ok(BuiltHook::Post(Arc::new(CountingPost {
                    n: self.counter,
                    name: "counter",
                }))),
                // Deliberately return Pre when a Post was declared
                // so we can test the lifecycle-mismatch refusal.
                "wrong-lifecycle" => Ok(BuiltHook::Pre(Arc::new(NoOpPre("wrong")))),
                other => Err(PluginError::Backend(format!("unknown hook id `{other}`"))),
            }
        }
    }

    fn make_plugin(name: &str, hooks: Vec<HookDeclaration>, enabled: bool) -> LoadedPlugin {
        let manifest = PluginManifest {
            name: name.to_owned(),
            version: "0.1.0".to_owned(),
            kind: PluginKind::Standalone,
            description: String::new(),
            caps: vec![],
            entry: String::new(),
            tags: vec![],
            hooks,
        };
        let provenance = manifest.provenance_digest().expect("provenance");
        LoadedPlugin {
            manifest,
            provenance,
            enabled,
            manifest_path: None,
        }
    }

    // ── tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn register_hooks_registers_pre_and_post() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "p1",
            vec![
                HookDeclaration {
                    id: "noop".into(),
                    lifecycle: HookLifecycle::PreTool,
                    priority: 50,
                    target_tools: vec![],
                    description: String::new(),
                },
                HookDeclaration {
                    id: "counter".into(),
                    lifecycle: HookLifecycle::PostTool,
                    priority: 100,
                    target_tools: vec![],
                    description: String::new(),
                },
            ],
            true,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        let factory = TestFactory { counter: &N };
        let report = reg.register_hooks(&bus, &factory).unwrap();
        assert_eq!(report.pre_registered, 1);
        assert_eq!(report.post_registered, 1);
        assert_eq!(bus.pre_len(), 1);
        assert_eq!(bus.post_len(), 1);
    }

    #[tokio::test]
    async fn disabled_plugin_is_skipped() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "p1",
            vec![HookDeclaration {
                id: "noop".into(),
                lifecycle: HookLifecycle::PreTool,
                priority: 0,
                target_tools: vec![],
                description: String::new(),
            }],
            false,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        let report = reg
            .register_hooks(&bus, &TestFactory { counter: &N })
            .unwrap();
        assert_eq!(report.pre_registered, 0);
        assert_eq!(report.skipped_disabled, 1);
        assert_eq!(bus.pre_len(), 0);
    }

    #[tokio::test]
    async fn unknown_hook_id_fails_closed() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "p1",
            vec![HookDeclaration {
                id: "ghost".into(),
                lifecycle: HookLifecycle::PreTool,
                priority: 0,
                target_tools: vec![],
                description: String::new(),
            }],
            true,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        let err = reg
            .register_hooks(&bus, &TestFactory { counter: &N })
            .unwrap_err();
        match err {
            PluginError::Backend(msg) => assert!(msg.contains("ghost")),
            other => panic!("expected Backend, got {other:?}"),
        }
        assert_eq!(bus.pre_len(), 0, "no partial registration");
    }

    #[tokio::test]
    async fn lifecycle_mismatch_is_refused() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "p1",
            vec![HookDeclaration {
                // Manifest declares PostTool but our test factory
                // returns Pre for this id → mismatch.
                id: "wrong-lifecycle".into(),
                lifecycle: HookLifecycle::PostTool,
                priority: 0,
                target_tools: vec![],
                description: String::new(),
            }],
            true,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        let err = reg
            .register_hooks(&bus, &TestFactory { counter: &N })
            .unwrap_err();
        match err {
            PluginError::InvalidManifest(msg) => assert!(msg.contains("lifecycle mismatch")),
            other => panic!("expected InvalidManifest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn target_tools_filter_only_fires_for_matching_tool() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "guard",
            vec![HookDeclaration {
                id: "deny".into(),
                lifecycle: HookLifecycle::PreTool,
                priority: 0,
                target_tools: vec!["shell".into()],
                description: String::new(),
            }],
            true,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        reg.register_hooks(&bus, &TestFactory { counter: &N })
            .unwrap();

        // Event targeting `shell` → denied.
        let denied = bus
            .fire_pre(&PreToolEvent::new("shell", serde_json::json!({})))
            .await;
        assert!(denied.outcome.is_deny());

        // Event targeting `echo` → allowed (the wrapper short-circuits).
        let allowed = bus
            .fire_pre(&PreToolEvent::new("echo", serde_json::json!({})))
            .await;
        assert!(allowed.outcome.is_allow());
    }

    #[tokio::test]
    async fn target_tools_filter_applies_to_post_hooks() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "p1",
            vec![HookDeclaration {
                id: "counter".into(),
                lifecycle: HookLifecycle::PostTool,
                priority: 0,
                target_tools: vec!["echo".into()],
                description: String::new(),
            }],
            true,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        N.store(0, Ordering::SeqCst);
        reg.register_hooks(&bus, &TestFactory { counter: &N })
            .unwrap();

        // Target match → counter increments.
        bus.fire_post(&PostToolEvent::new("echo", true, serde_json::json!({})))
            .await;
        // Non-target → counter unchanged.
        bus.fire_post(&PostToolEvent::new("shell", true, serde_json::json!({})))
            .await;
        assert_eq!(N.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn priority_passes_through_to_bus() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "p1",
            vec![
                HookDeclaration {
                    id: "deny".into(),
                    lifecycle: HookLifecycle::PreTool,
                    priority: 0, // fires first
                    target_tools: vec![],
                    description: String::new(),
                },
                HookDeclaration {
                    id: "noop".into(),
                    lifecycle: HookLifecycle::PreTool,
                    priority: 200, // would fire later
                    target_tools: vec![],
                    description: String::new(),
                },
            ],
            true,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        reg.register_hooks(&bus, &TestFactory { counter: &N })
            .unwrap();
        let report = bus
            .fire_pre(&PreToolEvent::new("x", serde_json::json!({})))
            .await;
        assert!(report.outcome.is_deny());
        // The earlier-priority deny short-circuits, so only one hook
        // fired even though two are registered.
        assert_eq!(report.fired, 1);
    }

    #[tokio::test]
    async fn empty_target_tools_means_match_every_tool() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let reg = PluginRegistry::new();
        let p = make_plugin(
            "p1",
            vec![HookDeclaration {
                id: "deny".into(),
                lifecycle: HookLifecycle::PreTool,
                priority: 0,
                target_tools: vec![], // empty → every tool
                description: String::new(),
            }],
            true,
        );
        reg.register(p, CapToken::TOP).unwrap();
        let bus = HookBus::new();
        reg.register_hooks(&bus, &TestFactory { counter: &N })
            .unwrap();
        for tool in ["shell", "echo", "http_get"] {
            let report = bus
                .fire_pre(&PreToolEvent::new(tool, serde_json::json!({})))
                .await;
            assert!(report.outcome.is_deny(), "tool {tool} should be denied");
        }
    }
}
