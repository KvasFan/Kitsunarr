//! Tanukarr's `AniListRequestService`: one request pacer shared by every AniList request the
//! server makes (metadata, searches, franchise relations, import list reads, list tests, custom
//! list lookups and progress updates), since they all consume the same server allowance.
//!
//! Requests are sent one at a time, at least 2.1 seconds apart (AniList allows 90 per minute;
//! it degrades to 30), spaced further when `X-RateLimit-Limit` advertises a lower allowance or
//! `X-RateLimit-Remaining` shows the window's budget running out. An HTTP 429 sets a cooldown
//! from `Retry-After` or `X-RateLimit-Reset` (one minute otherwise) and the request is tried up
//! to three times within two minutes; then [`PacerError::RateLimited`] reports how long to
//! wait. Optional requests (presentation lookups) get one attempt within five seconds.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use tokio::time::Instant;

/// `RequestInterval`.
pub const REQUEST_INTERVAL: Duration = Duration::from_millis(2100);
const MAX_ATTEMPTS: u32 = 3;
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);
const MAX_REQUEST_WAIT: Duration = Duration::from_secs(120);
const OPTIONAL_REQUEST_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum PacerError {
    /// AniList's allowance is used up; retry after the given time.
    #[error("AniList request limit reached. Retry later.")]
    RateLimited { retry_after: Duration },
    #[error(transparent)]
    Request(#[from] reqwest::Error),
}

#[derive(Debug)]
struct State {
    not_before: Option<Instant>,
    last_request_at: Option<Instant>,
    advertised_interval: Duration,
    budget_reset_at: Option<Instant>,
    last_remaining: i64,
    budget_interval: Duration,
}

/// The shared pacer; clones share the same state.
#[derive(Debug, Clone)]
pub struct AniListPacer {
    interval: Duration,
    state: Arc<Mutex<State>>,
}

impl Default for AniListPacer {
    fn default() -> Self {
        Self::new(REQUEST_INTERVAL)
    }
}

fn header<T: std::str::FromStr>(resp: &reqwest::Response, name: &str) -> Option<T> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
}

/// A Unix timestamp (seconds) as an instant; `None` when invalid.
fn instant_at_unix(seconds: i64, now: Instant) -> Option<Instant> {
    let at = UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(seconds).ok()?))?;
    Some(match at.duration_since(SystemTime::now()) {
        Ok(ahead) => now + ahead,
        Err(behind) => now.checked_sub(behind.duration()).unwrap_or(now),
    })
}

fn later(a: Option<Instant>, b: Instant) -> Option<Instant> {
    Some(a.map_or(b, |a| a.max(b)))
}

impl State {
    fn new(interval: Duration) -> Self {
        Self {
            not_before: None,
            last_request_at: None,
            advertised_interval: interval,
            budget_reset_at: None,
            last_remaining: 0,
            budget_interval: interval,
        }
    }

    fn remaining_cooldown(&self, now: Instant) -> Duration {
        self.not_before
            .map_or(Duration::ZERO, |n| n.saturating_duration_since(now))
    }

    fn rate_limited(&self, now: Instant) -> PacerError {
        let secs = self.remaining_cooldown(now).as_secs_f64().ceil().max(1.0);
        PacerError::RateLimited {
            retry_after: Duration::from_secs(secs as u64),
        }
    }

    fn effective_interval(&mut self, interval: Duration, now: Instant) -> Duration {
        if self.budget_reset_at.is_none_or(|r| r <= now) {
            self.budget_interval = interval;
        }
        self.advertised_interval.max(self.budget_interval)
    }

    /// `UpdateAdvertisedBudget`.
    fn update_budget(&mut self, interval: Duration, resp: &reqwest::Response) {
        let now = Instant::now();
        if let Some(limit) = header::<i64>(resp, "X-RateLimit-Limit").filter(|l| *l > 0) {
            // A lower allowance needs wider spacing; never faster than the default.
            self.advertised_interval = Duration::from_secs_f64(61.0 / limit as f64).max(interval);
        }
        if let Some(remaining) = header::<i64>(resp, "X-RateLimit-Remaining").filter(|r| *r >= 0) {
            let reset = header::<i64>(resp, "X-RateLimit-Reset")
                .and_then(|s| instant_at_unix(s, now))
                .filter(|r| *r > now);
            if let Some(reset) = reset {
                self.budget_reset_at = Some(reset);
            } else if self.budget_reset_at.is_none_or(|r| r <= now)
                || remaining > self.last_remaining
            {
                // Successful responses often omit the reset: track the observed window.
                self.budget_reset_at = Some(now + DEFAULT_COOLDOWN);
            }
            self.last_remaining = remaining;
            self.budget_interval = match (remaining, self.budget_reset_at) {
                (r, Some(reset)) if r > 0 => reset.saturating_duration_since(now) / r as u32,
                _ => interval,
            };
        }
        let effective = self.effective_interval(interval, now);
        if effective > interval {
            self.not_before = later(self.not_before, now + effective);
        }
    }

    /// `ExtendCooldown`.
    fn extend_cooldown(&mut self, resp: &reqwest::Response) {
        let now = Instant::now();
        let mut not_before = now;
        if let Some(retry) = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
        {
            if let Ok(seconds) = retry.trim().parse::<i64>() {
                if seconds > 0 {
                    not_before = now + Duration::from_secs(seconds as u64);
                }
            } else if let Ok(date) = chrono::DateTime::parse_from_rfc2822(retry.trim())
                && let Some(at) = instant_at_unix(date.timestamp(), now)
            {
                not_before = at;
            }
        }
        if let Some(reset) =
            header::<i64>(resp, "X-RateLimit-Reset").and_then(|s| instant_at_unix(s, now))
            && reset > not_before
        {
            not_before = reset;
        }
        if not_before <= now {
            not_before = now + DEFAULT_COOLDOWN;
        }
        self.not_before = later(self.not_before, not_before);
    }
}

/// The process's pacers, one per AniList server (scheme, host and port): metadata, import
/// lists, list tests, custom-list lookups and progress updates sending to the same server
/// share its allowance, as Tanukarr's process-wide request state does.
fn registry() -> &'static std::sync::Mutex<std::collections::HashMap<String, AniListPacer>> {
    static PACERS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, AniListPacer>>,
    > = std::sync::OnceLock::new();
    PACERS.get_or_init(Default::default)
}

fn server_key(base: &str) -> String {
    match reqwest::Url::parse(base.trim()) {
        Ok(u) => format!(
            "{}://{}:{}",
            u.scheme(),
            u.host_str().unwrap_or_default().to_ascii_lowercase(),
            u.port_or_known_default().unwrap_or_default()
        ),
        Err(_) => base.trim().to_ascii_lowercase(),
    }
}

impl AniListPacer {
    /// A pacer spacing requests at least `interval` apart (2.1 seconds for AniList's servers;
    /// tests against a local mock use a shorter one). Not shared: see [`Self::shared`].
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            state: Arc::new(Mutex::new(State::new(interval))),
        }
    }

    /// The pacer every request to the AniList server at `base` shares (created with the
    /// default spacing on first use).
    pub fn shared(base: &str) -> Self {
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(server_key(base))
            .or_default()
            .clone()
    }

    /// Like [`Self::shared`], with this spacing: a pacer already registered for the server
    /// with another spacing is replaced (its cooldown is kept only when the spacing matches).
    pub fn shared_with_interval(base: &str, interval: Duration) -> Self {
        let mut pacers = registry().lock().unwrap_or_else(|e| e.into_inner());
        let pacer = pacers
            .entry(server_key(base))
            .or_insert_with(|| Self::new(interval));
        if pacer.interval != interval {
            *pacer = Self::new(interval);
        }
        pacer.clone()
    }

    /// Whether two handles share the same state.
    pub fn same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    /// Sends a request built by `build` (called once per attempt). Returns any response that
    /// is not HTTP 429.
    pub async fn send(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, PacerError> {
        let state = self.state.lock().await;
        self.execute(state, build, false).await
    }

    /// `ExecuteOptional`: one attempt within five seconds, given up when another request
    /// holds the pacer for more than two seconds.
    pub async fn send_optional(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, PacerError> {
        let state = match tokio::time::timeout(Duration::from_secs(2), self.state.lock()).await {
            Ok(state) => state,
            Err(_) => {
                return Err(PacerError::RateLimited {
                    retry_after: Duration::from_secs(1),
                });
            }
        };
        self.execute(state, build, true).await
    }

    async fn execute(
        &self,
        mut state: tokio::sync::MutexGuard<'_, State>,
        build: impl Fn() -> reqwest::RequestBuilder,
        optional: bool,
    ) -> Result<reqwest::Response, PacerError> {
        let interval = self.interval;
        let deadline = Instant::now()
            + if optional {
                OPTIONAL_REQUEST_WAIT
            } else {
                MAX_REQUEST_WAIT
            };
        let attempts = if optional { 1 } else { MAX_ATTEMPTS };
        for attempt in 1..=attempts {
            // `WaitForCooldown` and the per-request spacing.
            let next = state.last_request_at.map(|l| l + interval);
            if let Some(next) = next.filter(|n| *n > deadline) {
                state.not_before = later(state.not_before, next);
                return Err(state.rate_limited(Instant::now()));
            }
            if state.remaining_cooldown(Instant::now()) > Duration::ZERO {
                if state.not_before.is_some_and(|n| n > deadline) {
                    return Err(state.rate_limited(Instant::now()));
                }
                tokio::time::sleep_until(state.not_before.expect("cooldown")).await;
            }
            if let Some(next) = next {
                tokio::time::sleep_until(next).await;
            }
            let now = Instant::now();
            state.last_request_at = Some(now);
            if now > deadline {
                return Err(state.rate_limited(now));
            }
            // A failed request can still consume the allowance: reserve it before sending.
            let effective = state.effective_interval(interval, now);
            if effective > interval {
                state.not_before = later(state.not_before, now + effective);
            }
            if state.budget_reset_at.is_some_and(|r| r > now) && state.last_remaining > 0 {
                state.last_remaining -= 1;
                if state.last_remaining == 0 {
                    let reset = state.budget_reset_at.expect("checked");
                    if state.not_before.is_none_or(|n| reset > n) {
                        state.not_before = Some(reset);
                    }
                }
            }
            let resp = build().send().await?;
            state.update_budget(interval, &resp);
            if resp.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
                if header::<i64>(&resp, "X-RateLimit-Remaining") == Some(0) {
                    state.extend_cooldown(&resp);
                }
                return Ok(resp);
            }
            state.extend_cooldown(&resp);
            let now = Instant::now();
            if attempt == attempts || state.not_before.is_some_and(|n| n > deadline) {
                if !optional {
                    tracing::warn!(
                        retry_after = state.remaining_cooldown(now).as_secs(),
                        "AniList request limit reached"
                    );
                }
                return Err(state.rate_limited(now));
            }
            tracing::debug!(
                wait = state.remaining_cooldown(now).as_secs(),
                retry = attempt + 1,
                attempts,
                "AniList request limit reached; waiting before retrying"
            );
        }
        Err(state.rate_limited(Instant::now()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, StatusCode};
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn serve(router: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}/")
    }

    /// Answers 429 (`Retry-After: 1`) to the first `failures` requests, then 200.
    async fn flaky(
        failures: usize,
    ) -> (
        String,
        Arc<AtomicUsize>,
        Arc<std::sync::Mutex<Vec<Instant>>>,
    ) {
        let hits = Arc::new(AtomicUsize::new(0));
        let times = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (h, t) = (hits.clone(), times.clone());
        let router = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let (h, t) = (h.clone(), t.clone());
                async move {
                    t.lock().unwrap().push(Instant::now());
                    let mut headers = HeaderMap::new();
                    if h.fetch_add(1, Ordering::SeqCst) < failures {
                        headers.insert("Retry-After", "1".parse().unwrap());
                        headers.insert("X-RateLimit-Remaining", "0".parse().unwrap());
                        (StatusCode::TOO_MANY_REQUESTS, headers, "{}")
                    } else {
                        headers.insert("X-RateLimit-Limit", "90".parse().unwrap());
                        headers.insert("X-RateLimit-Remaining", "80".parse().unwrap());
                        (StatusCode::OK, headers, "{\"data\":{}}")
                    }
                }
            }),
        );
        (serve(router).await, hits, times)
    }

    #[tokio::test]
    async fn requests_are_spaced_by_the_interval_across_clones() {
        let (url, hits, times) = flaky(0).await;
        let pacer = AniListPacer::new(Duration::from_millis(300));
        let other = pacer.clone();
        let http = reqwest::Client::new();
        let (a, b) = tokio::join!(
            pacer.send(|| http.post(&url)),
            other.send(|| http.post(&url))
        );
        assert_eq!(a.unwrap().status(), 200);
        assert_eq!(b.unwrap().status(), 200);
        pacer.send(|| http.post(&url)).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        let t = times.lock().unwrap().clone();
        for pair in t.windows(2) {
            assert!(
                pair[1] - pair[0] >= Duration::from_millis(290),
                "{:?}",
                pair[1] - pair[0]
            );
        }
    }

    #[tokio::test]
    async fn a_429_waits_for_retry_after_and_retries() {
        let (url, hits, times) = flaky(2).await;
        let pacer = AniListPacer::new(Duration::from_millis(10));
        let http = reqwest::Client::new();
        let resp = pacer.send(|| http.post(&url)).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        let t = times.lock().unwrap().clone();
        // `Retry-After: 1` between each attempt.
        assert!(t[1] - t[0] >= Duration::from_millis(900));
        assert!(t[2] - t[1] >= Duration::from_millis(900));
    }

    #[tokio::test]
    async fn three_429s_report_the_cooldown_and_the_next_request_waits() {
        let (url, hits, _) = flaky(usize::MAX).await;
        let pacer = AniListPacer::new(Duration::from_millis(10));
        let http = reqwest::Client::new();
        match pacer.send(|| http.post(&url)).await {
            Err(PacerError::RateLimited { retry_after }) => {
                assert!(retry_after >= Duration::from_secs(1));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        // An optional request does not wait out a cooldown beyond its deadline, but this
        // one is short enough to wait for.
        let started = Instant::now();
        let _ = pacer.send_optional(|| http.post(&url)).await;
        assert!(started.elapsed() >= Duration::from_millis(500));
        assert_eq!(hits.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn a_low_advertised_limit_widens_the_spacing() {
        let times = Arc::new(std::sync::Mutex::new(Vec::new()));
        let t = times.clone();
        let router = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let t = t.clone();
                async move {
                    t.lock().unwrap().push(Instant::now());
                    let mut headers = HeaderMap::new();
                    // 61 s / 61 requests: one second apart.
                    headers.insert("X-RateLimit-Limit", "61".parse().unwrap());
                    (StatusCode::OK, headers, "{}")
                }
            }),
        );
        let url = serve(router).await;
        let pacer = AniListPacer::new(Duration::from_millis(10));
        let http = reqwest::Client::new();
        pacer.send(|| http.post(&url)).await.unwrap();
        pacer.send(|| http.post(&url)).await.unwrap();
        let t = times.lock().unwrap().clone();
        assert!(
            t[1] - t[0] >= Duration::from_millis(950),
            "{:?}",
            t[1] - t[0]
        );
    }

    #[test]
    fn one_pacer_per_server() {
        let a = AniListPacer::shared("https://graphql.example.test/");
        let b = AniListPacer::shared("HTTPS://GRAPHQL.example.test:443/graphql");
        assert!(a.same_as(&b));
        assert!(!a.same_as(&AniListPacer::shared("https://other.example.test")));
        assert_eq!(a.interval, REQUEST_INTERVAL);
        let fast = AniListPacer::shared_with_interval("http://127.0.0.1:1/x", Duration::ZERO);
        assert!(fast.same_as(&AniListPacer::shared("http://127.0.0.1:1/")));
        assert_eq!(fast.interval, Duration::ZERO);
    }

    #[tokio::test]
    async fn a_cooldown_past_the_deadline_fails_at_once() {
        let mut headers = HeaderMap::new();
        headers.insert("Retry-After", "600".parse().unwrap());
        let router = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let headers = headers.clone();
                async move { (StatusCode::TOO_MANY_REQUESTS, headers, "{}") }
            }),
        );
        let url = serve(router).await;
        let pacer = AniListPacer::new(Duration::from_millis(10));
        let http = reqwest::Client::new();
        let started = Instant::now();
        match pacer.send(|| http.post(&url)).await {
            Err(PacerError::RateLimited { retry_after }) => {
                assert!(retry_after > Duration::from_secs(500));
            }
            other => panic!("{other:?}"),
        }
        // Ten minutes exceed the two-minute deadline: no waiting, no second attempt.
        assert!(started.elapsed() < Duration::from_secs(5));
        match pacer.send(|| http.post(&url)).await {
            Err(PacerError::RateLimited { .. }) => {}
            other => panic!("{other:?}"),
        }
    }
}
