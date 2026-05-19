//! [`ExecRouter`] — cap-gated dispatch over registered executors.
//!
//! Hosts register one or more [`SessionExecutor`]s under their
//! [`Backend`] tag and call `route.dispatch(grant, backend, req)`.
//! The router re-checks the corresponding `cap:executor:<backend>`
//! cap before invoking the executor — defence in depth above the
//! kernel admit gate.

use std::collections::BTreeMap;
use std::sync::Arc;

use gauss_core::CapToken;
use thiserror::Error;

use crate::types::{Backend, ExecOutput, ExecRequest, Receipt, SessionExecutor};

/// Router-side error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecRouterError {
    /// No executor was registered for the requested backend.
    #[error("no executor registered for backend {0:?}")]
    UnregisteredBackend(Backend),
    /// Caller's grant didn't satisfy the per-backend cap.
    #[error(
        "admit refused: backend {backend:?} requires cap 0x{required:016x}, grant 0x{grant:016x}"
    )]
    AdmitRefused {
        /// Backend the call targeted.
        backend: Backend,
        /// Required cap bits.
        required: u64,
        /// Grant cap bits.
        grant: u64,
    },
    /// Underlying executor failed.
    #[error("executor: {0}")]
    Executor(#[from] crate::types::ExecError),
}

/// Router. Cheap to clone (`Arc`-shared map of executors).
#[derive(Clone, Default)]
pub struct ExecRouter {
    executors: Arc<BTreeMap<Backend, Arc<dyn SessionExecutor>>>,
}

impl ExecRouter {
    /// Build a fresh empty router.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a router from a flat list of `(backend, executor)` pairs.
    #[must_use]
    pub fn from_pairs(pairs: Vec<(Backend, Arc<dyn SessionExecutor>)>) -> Self {
        let mut map = BTreeMap::new();
        for (b, e) in pairs {
            map.insert(b, e);
        }
        Self {
            executors: Arc::new(map),
        }
    }

    /// Register an executor under its declared backend.
    #[must_use]
    pub fn register(mut self, executor: Arc<dyn SessionExecutor>) -> Self {
        let backend = executor.backend();
        let mut map = (*self.executors).clone();
        map.insert(backend, executor);
        self.executors = Arc::new(map);
        self
    }

    /// The set of backends this router knows about.
    #[must_use]
    pub fn backends(&self) -> Vec<Backend> {
        self.executors.keys().copied().collect()
    }

    /// Dispatch a request.
    pub async fn dispatch(
        &self,
        grant: CapToken,
        backend: Backend,
        request: ExecRequest,
    ) -> Result<(ExecOutput, Receipt), ExecRouterError> {
        let required = backend.required_cap();
        if !grant.contains(required) {
            return Err(ExecRouterError::AdmitRefused {
                backend,
                required: required.bits(),
                grant: grant.bits(),
            });
        }
        let executor = self
            .executors
            .get(&backend)
            .ok_or(ExecRouterError::UnregisteredBackend(backend))?;
        let pair = executor.exec(request).await?;
        Ok(pair)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalExecutor;

    #[tokio::test]
    async fn dispatch_routes_to_registered_executor() {
        let router = ExecRouter::new().register(Arc::new(LocalExecutor::new()));
        let req = ExecRequest::new("/bin/sh", vec!["-c".into(), "echo ok".into()]);
        let (out, receipt) = router
            .dispatch(CapToken::EXECUTOR_LOCAL, Backend::Local, req)
            .await
            .unwrap();
        assert_eq!(receipt.backend, Backend::Local);
        assert!(out.stdout.contains("ok"));
    }

    #[tokio::test]
    async fn dispatch_refuses_when_grant_misses_cap() {
        let router = ExecRouter::new().register(Arc::new(LocalExecutor::new()));
        let req = ExecRequest::new("/bin/echo", vec!["x".into()]);
        let err = router
            .dispatch(CapToken::BOTTOM, Backend::Local, req)
            .await
            .unwrap_err();
        assert!(matches!(err, ExecRouterError::AdmitRefused { .. }));
    }

    #[tokio::test]
    async fn dispatch_refuses_unregistered_backend() {
        // Empty router — no executor registered.
        let router = ExecRouter::new();
        let req = ExecRequest::new("/bin/echo", vec!["x".into()]);
        let err = router
            .dispatch(CapToken::TOP, Backend::Docker, req)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ExecRouterError::UnregisteredBackend(Backend::Docker)
        ));
    }

    #[tokio::test]
    async fn router_lists_registered_backends() {
        let router = ExecRouter::new().register(Arc::new(LocalExecutor::new()));
        assert_eq!(router.backends(), vec![Backend::Local]);
    }
}
