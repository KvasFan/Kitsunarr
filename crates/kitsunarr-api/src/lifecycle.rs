//! Stopping and restarting the server from the API (Sonarr's `system/shutdown` and
//! `system/restart`): the server binary hands its shutdown token to the state; cancelling it
//! starts the graceful shutdown, and a restart request makes the binary start itself again
//! once everything has stopped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct Lifecycle {
    token: CancellationToken,
    restart: Arc<AtomicBool>,
}

impl Lifecycle {
    /// A lifecycle driving the server's shutdown token.
    pub fn new(token: CancellationToken) -> Self {
        Self {
            token,
            restart: Arc::default(),
        }
    }

    /// Starts the graceful shutdown shortly (after the current response has been sent).
    pub fn shutdown(&self) {
        tracing::info!("shutdown requested through the API");
        self.stop_soon();
    }

    /// Starts the graceful shutdown; the server starts again afterwards.
    pub fn restart(&self) {
        tracing::info!("restart requested through the API");
        self.restart.store(true, Ordering::SeqCst);
        self.stop_soon();
    }

    /// Whether the server should start again after stopping.
    pub fn restart_requested(&self) -> bool {
        self.restart.load(Ordering::SeqCst)
    }

    /// Whether a shutdown or restart has been requested.
    pub fn stopping(&self) -> bool {
        self.token.is_cancelled()
    }

    fn stop_soon(&self) {
        let token = self.token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            token.cancel();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restart_cancels_the_token_and_is_remembered() {
        let token = CancellationToken::new();
        let l = Lifecycle::new(token.clone());
        assert!(!l.restart_requested() && !l.stopping());
        l.restart();
        tokio::time::timeout(Duration::from_secs(2), token.cancelled())
            .await
            .unwrap();
        assert!(l.restart_requested() && l.stopping());
        let s = Lifecycle::new(CancellationToken::new());
        s.shutdown();
        assert!(!s.restart_requested());
    }
}
