//! Deterministic in-process "toy" provider.
//!
//! `ToyProvider` returns a fixed script of actions, optionally cycling on each
//! call. It exists so the Differential Turn Engine can be exercised end-to-end
//! without any external dependency on a real LLM API, and so the crash-injection
//! conformance tests have a deterministic policy.

use std::sync::Mutex;

use async_trait::async_trait;
use gauss_core::{Action, GaussError, GaussResult, Observation, TextAction};
use gauss_traits::Provider;

/// Deterministic provider used in tests and for crash-injection harnesses.
#[derive(Debug)]
pub struct ToyProvider {
    script: Vec<Vec<Action>>,
    cursor: Mutex<usize>,
    cycle: bool,
}

impl ToyProvider {
    /// Build a provider that, on each `generate` call, returns the next entry
    /// from `script`. If `cycle` is `true` the cursor wraps; otherwise the
    /// provider returns `GaussError::Internal("script exhausted")` past the
    /// end.
    #[must_use]
    pub const fn new(script: Vec<Vec<Action>>, cycle: bool) -> Self {
        Self {
            script,
            cursor: Mutex::new(0),
            cycle,
        }
    }

    /// Shortcut: a provider that always returns one [`TextAction`] echoing
    /// the supplied string.
    #[must_use]
    pub fn always_text(body: impl Into<String>) -> Self {
        Self::new(vec![vec![Action::Text(TextAction::new(body.into()))]], true)
    }

    /// Return the number of times `generate` has been called so far.
    pub fn call_count(&self) -> usize {
        self.cursor.lock().map(|g| *g).unwrap_or(0)
    }
}

#[async_trait]
impl Provider for ToyProvider {
    async fn generate(&self, _obs: &Observation) -> GaussResult<Vec<Action>> {
        // Resolve the next script entry while holding the lock briefly, then
        // drop it before returning so the Mutex guard does not span an await.
        let next = {
            let mut cursor = self
                .cursor
                .lock()
                .map_err(|e| GaussError::Internal(format!("toy provider mutex poisoned: {e}")))?;
            let index = *cursor;
            if index >= self.script.len() {
                if self.cycle && !self.script.is_empty() {
                    *cursor = 1;
                    drop(cursor);
                    return Ok(self.script[0].clone());
                }
                drop(cursor);
                return Err(GaussError::Internal("script exhausted".into()));
            }
            *cursor = cursor.saturating_add(1);
            index
        };
        Ok(self.script[next].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gauss_core::{ObservationSource, TaintLabel};

    fn obs() -> Observation {
        Observation::new(
            ObservationSource::User {
                channel: "test".into(),
            },
            TaintLabel::User,
            serde_json::Value::Null,
        )
    }

    #[tokio::test]
    async fn always_text_returns_the_same_action() {
        let p = ToyProvider::always_text("hello");
        let out = p.generate(&obs()).await.unwrap();
        match &out[0] {
            Action::Text(t) => assert_eq!(t.body, "hello"),
            _ => panic!("expected text"),
        }
        // cycling — second call returns the same script entry.
        let out2 = p.generate(&obs()).await.unwrap();
        match &out2[0] {
            Action::Text(t) => assert_eq!(t.body, "hello"),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn script_exhaustion_without_cycle_errors() {
        let p = ToyProvider::new(
            vec![vec![Action::Text(TextAction::new("once"))]],
            /* cycle */ false,
        );
        p.generate(&obs()).await.unwrap();
        let err = p
            .generate(&obs())
            .await
            .expect_err("must error after exhaustion");
        match err {
            GaussError::Internal(msg) => assert!(msg.contains("script exhausted")),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
