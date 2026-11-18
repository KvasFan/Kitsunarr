//! Bounded interactive searches and cancellation of a client's superseded request.
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::error::{ApiError, ApiResult};

pub(crate) const SOURCE_BUDGET: Duration = Duration::from_secs(90);
const INTERACTIVE_BUDGET: Duration = Duration::from_secs(100);

#[derive(Default)]
struct Active {
    generation: u64,
    entries: HashMap<String, (u64, CancellationToken)>,
}

#[derive(Clone, Default)]
pub struct SearchRegistry(Arc<Mutex<Active>>);

struct Lease {
    owner: SearchRegistry,
    key: Option<String>,
    generation: u64,
    token: CancellationToken,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.token.cancel();
        if let Some(key) = &self.key {
            let mut active = self.owner.0.lock().unwrap_or_else(|e| e.into_inner());
            if active
                .entries
                .get(key)
                .is_some_and(|(id, _)| *id == self.generation)
            {
                active.entries.remove(key);
            }
        }
    }
}

impl SearchRegistry {
    fn begin(&self, key: Option<&str>) -> ApiResult<Lease> {
        let token = CancellationToken::new();
        let mut generation = 0;
        if let Some(key) = key {
            if key.is_empty()
                || key.len() > 64
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            {
                return Err(ApiError::field(
                    "searchSession",
                    "Invalid search session identifier.",
                ));
            }
            let mut active = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if active.entries.len() >= 256 && !active.entries.contains_key(key) {
                return Err(ApiError::TooManyRequests(
                    "Too many active searches; retry shortly.".into(),
                ));
            }
            active.generation = active.generation.wrapping_add(1);
            generation = active.generation;
            if let Some((_, old)) = active
                .entries
                .insert(key.into(), (generation, token.clone()))
            {
                old.cancel();
            }
        }
        Ok(Lease {
            owner: self.clone(),
            key: key.map(str::to_string),
            generation,
            token,
        })
    }

    pub async fn run<T>(
        &self,
        key: Option<&str>,
        work: impl Future<Output = ApiResult<T>>,
    ) -> ApiResult<T> {
        self.run_with_budget(key, work, INTERACTIVE_BUDGET).await
    }

    async fn run_with_budget<T>(
        &self,
        key: Option<&str>,
        work: impl Future<Output = ApiResult<T>>,
        budget: Duration,
    ) -> ApiResult<T> {
        let lease = self.begin(key)?;
        tokio::select! {
            biased;
            _ = lease.token.cancelled() => Err(ApiError::Conflict("This search was superseded by a newer request.".into())),
            result = tokio::time::timeout(budget,work) => result.unwrap_or_else(|_| Err(ApiError::GatewayTimeout("The search exceeded its total time budget. Choose fewer sources and try again.".into()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn superseded_request_cancels_without_removing_its_replacement() {
        let registry = SearchRegistry::default();
        let old = registry.begin(Some("client")).unwrap();
        let current = registry.begin(Some("client")).unwrap();
        assert!(old.token.is_cancelled());
        drop(old);
        assert!(!current.token.is_cancelled());
        assert_eq!(registry.0.lock().unwrap().entries.len(), 1);
        drop(current);
        assert!(registry.0.lock().unwrap().entries.is_empty());
    }
    #[tokio::test]
    async fn total_budget_and_superseding_bound_work_and_cleanup() {
        let registry = SearchRegistry::default();
        let result = registry
            .run_with_budget(
                Some("budget"),
                std::future::pending::<ApiResult<()>>(),
                Duration::from_millis(10),
            )
            .await;
        assert!(matches!(result, Err(ApiError::GatewayTimeout(_))));
        assert!(registry.0.lock().unwrap().entries.is_empty());
        let copy = registry.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let old = tokio::spawn(async move {
            copy.run(Some("shared"), async {
                let _ = started.send(());
                std::future::pending::<ApiResult<()>>().await
            })
            .await
        });
        ready.await.unwrap();
        assert_eq!(
            registry
                .run(Some("shared"), async { Ok(42) })
                .await
                .unwrap(),
            42
        );
        assert!(matches!(old.await.unwrap(), Err(ApiError::Conflict(_))));
        assert!(registry.0.lock().unwrap().entries.is_empty());
    }
}
