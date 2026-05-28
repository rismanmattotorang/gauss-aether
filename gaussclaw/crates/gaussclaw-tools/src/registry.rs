//! [`ToolRegistry`] — index of available [`gauss_traits::ToolTrait`]
//! implementations.
//!
//! Holds tools by their stable string id (the
//! [`gaussclaw_skill::SkillManifest::name`] field). Lookup is O(log n)
//! over a `BTreeMap`. Cheap to clone via `Arc`.

use std::collections::BTreeMap;
use std::sync::Arc;

use gauss_core::ToolId;
use gauss_traits::ToolTrait;
use thiserror::Error;

/// Registry error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// Lookup by id failed.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
}

/// Convenience result alias.
pub type RegistryResult<T> = Result<T, RegistryError>;

/// A tool registry. Cloning is cheap (internal `BTreeMap` of `Arc`s).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn ToolTrait>>,
}

impl ToolRegistry {
    /// Build an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Replaces any prior tool with the same id.
    pub fn register(&mut self, tool: Arc<dyn ToolTrait>) {
        let id = tool.manifest().id.0.clone();
        self.tools.insert(id, tool);
    }

    /// Look up a tool by name (the manifest's `name` field).
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolTrait>> {
        self.tools.get(name).cloned()
    }

    /// Return all registered tool ids in lexicographic order.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// Iterate `(id, ToolTrait)` pairs in lexicographic order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Arc<dyn ToolTrait>)> {
        self.tools.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Resolve a tool id to its handle.
    ///
    /// # Errors
    /// Returns [`RegistryError::UnknownTool`] when the id is missing.
    pub fn resolve(&self, id: &ToolId) -> RegistryResult<Arc<dyn ToolTrait>> {
        self.get(&id.0)
            .ok_or_else(|| RegistryError::UnknownTool(id.0.clone()))
    }
}

impl core::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("count", &self.tools.len())
            .field("ids", &self.ids())
            .finish()
    }
}
