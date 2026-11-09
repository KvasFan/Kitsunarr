//! Sliding-window rate limiter (async, shared through clones).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Clone)]
pub struct RateLimiter {
    max: usize,
    window: Duration,
    sent: Arc<Mutex<VecDeque<Instant>>>,
}

impl RateLimiter {
    /// At most `max` requests per `window`.
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max: max.max(1),
            window,
            sent: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Waits until a request may be sent, then records it.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut sent = self.sent.lock().await;
                let now = Instant::now();
                while sent
                    .front()
                    .is_some_and(|t| now.duration_since(*t) >= self.window)
                {
                    sent.pop_front();
                }
                if sent.len() < self.max {
                    sent.push_back(now);
                    return;
                }
                self.window - now.duration_since(*sent.front().expect("non-empty"))
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn enforces_the_window() {
        let limiter = RateLimiter::new(2, Duration::from_secs(10));
        let start = Instant::now();
        limiter.acquire().await;
        limiter.acquire().await;
        assert!(start.elapsed() < Duration::from_millis(1));
        limiter.acquire().await; // third request waits for the window to slide
        assert!(start.elapsed() >= Duration::from_secs(10));
    }
}
