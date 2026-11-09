//! In-process event bus.
//!
//! A bounded `tokio::sync::broadcast` channel: publishers never block, and a slow SSE client
//! that falls behind loses old events (it receives a `Lagged` notice and refetches) instead of
//! growing memory without bound.

use kitsunarr_domain::api::events::ServerEvent;
use tokio::sync::broadcast;

pub const DEFAULT_CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<ServerEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(16));
        Self { tx }
    }

    /// Publishes an event; silently dropped when nobody is listening.
    pub fn publish(&self, event: ServerEvent) {
        tracing::trace!(event = event.name(), "publish");
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.tx.subscribe()
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Convenience for toast-style notices.
    pub fn notice(&self, level: &str, message: impl Into<String>) {
        self.publish(ServerEvent::Notice {
            level: level.into(),
            message: message.into(),
        });
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delivers_and_bounds() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        bus.publish(ServerEvent::HealthChanged);
        assert_eq!(rx.recv().await.unwrap(), ServerEvent::HealthChanged);

        for _ in 0..40 {
            bus.publish(ServerEvent::HealthChanged);
        }
        // The receiver lagged: old events were dropped rather than buffered without bound.
        assert!(matches!(
            rx.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
    }
}
