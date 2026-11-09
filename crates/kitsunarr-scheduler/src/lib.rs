//! Background task scheduler.
//!
//! * Task definitions (interval, enabled) live in the `scheduled_tasks` table; each run is
//!   recorded in `task_runs` with status, progress, message and duration.
//! * A task never overlaps itself; manual triggers use the same path as scheduled runs.
//! * Runs interrupted by a crash are marked failed at start-up.
//! * Shutdown cancels the shared token and waits (bounded) for running jobs.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use kitsunarr_db::{Database, tasks};
use kitsunarr_domain::api::events::ServerEvent;
use kitsunarr_events::EventBus;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub type JobResult = Result<Option<String>, String>;
pub type JobFuture = Pin<Box<dyn Future<Output = JobResult> + Send>>;

/// Context handed to a running job.
#[derive(Clone)]
pub struct JobContext {
    pub run_id: i64,
    pub task_name: &'static str,
    pub db: Database,
    pub events: EventBus,
    pub cancel: CancellationToken,
}

impl JobContext {
    /// Records progress (0.0–1.0) and broadcasts it.
    pub async fn progress(&self, fraction: f64, message: &str) {
        let _ = tasks::progress(self.db.pool(), self.run_id, fraction, Some(message)).await;
        self.events.publish(ServerEvent::Task {
            run_id: self.run_id,
            task_name: self.task_name.into(),
            status: "running".into(),
            progress: Some(fraction),
            message: Some(message.into()),
        });
    }
}

pub struct Job {
    pub name: &'static str,
    pub description: &'static str,
    /// Interval used when the task is first registered.
    pub default_interval_minutes: i64,
    /// Upper bound for one run; exceeded runs are cancelled and marked failed.
    pub timeout: Duration,
    pub run: Arc<dyn Fn(JobContext) -> JobFuture + Send + Sync>,
}

struct Inner {
    db: Database,
    events: EventBus,
    jobs: HashMap<&'static str, Job>,
    running: Mutex<HashSet<&'static str>>,
    next_runs: Mutex<HashMap<&'static str, DateTime<Utc>>>,
    cancel: CancellationToken,
    tracker: TaskTracker,
}

#[derive(Clone)]
pub struct Scheduler(Arc<Inner>);

impl Scheduler {
    pub fn new(db: Database, events: EventBus, jobs: Vec<Job>, cancel: CancellationToken) -> Self {
        Self(Arc::new(Inner {
            db,
            events,
            jobs: jobs.into_iter().map(|j| (j.name, j)).collect(),
            running: Mutex::new(HashSet::new()),
            next_runs: Mutex::new(HashMap::new()),
            cancel,
            tracker: TaskTracker::new(),
        }))
    }

    /// Registers tasks, recovers interrupted runs and starts the timer loop.
    pub async fn start(&self, tick: Duration) -> Result<(), kitsunarr_db::DbError> {
        for job in self.0.jobs.values() {
            tasks::ensure(self.0.db.pool(), job.name, job.default_interval_minutes).await?;
        }
        let interrupted = tasks::fail_interrupted(self.0.db.pool()).await?;
        if interrupted > 0 {
            tracing::warn!(
                interrupted,
                "marked task runs interrupted by the previous shutdown as failed"
            );
        }
        let this = self.clone();
        self.0.tracker.spawn(async move {
            let mut interval = tokio::time::interval(tick);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = this.0.cancel.cancelled() => break,
                    _ = interval.tick() => this.tick().await,
                }
            }
        });
        Ok(())
    }

    async fn tick(&self) {
        let defs = match tasks::list(self.0.db.pool()).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(error = %e, "cannot read scheduled tasks");
                return;
            }
        };
        let now = Utc::now();
        for def in defs {
            let Some(job) = self.0.jobs.get(def.name.as_str()) else {
                continue;
            };
            if !def.enabled || def.interval_minutes <= 0 {
                self.0
                    .next_runs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(job.name);
                continue;
            }
            let last = def
                .last_run_at
                .as_deref()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Utc));
            let next = last.map_or(now, |l| l + chrono::Duration::minutes(def.interval_minutes));
            self.0
                .next_runs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(job.name, next);
            if next <= now {
                self.spawn_run(job.name, "scheduled");
            }
        }
    }

    /// Starts a run unless the task is already running. Returns whether it started.
    fn spawn_run(&self, name: &str, trigger: &'static str) -> bool {
        let Some(job) = self.0.jobs.get(name) else {
            return false;
        };
        if self.0.cancel.is_cancelled() {
            return false;
        }
        {
            let mut running = self.0.running.lock().unwrap_or_else(|e| e.into_inner());
            if !running.insert(job.name) {
                return false;
            }
        }
        let this = self.clone();
        let job_name = job.name;
        self.0.tracker.spawn(async move {
            this.execute(job_name, trigger).await;
            this.0
                .running
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(job_name);
        });
        true
    }

    async fn execute(&self, name: &'static str, trigger: &str) {
        let Some(job) = self.0.jobs.get(name) else {
            return;
        };
        let db = &self.0.db;
        let run_id = match tasks::start_run(db.pool(), name, trigger).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(task = name, error = %e, "cannot record task start");
                return;
            }
        };
        let publish = |status: &str, message: Option<String>| {
            self.0.events.publish(ServerEvent::Task {
                run_id,
                task_name: name.into(),
                status: status.into(),
                progress: None,
                message,
            });
        };
        publish("running", None);
        let started = Instant::now();
        let cancel = self.0.cancel.child_token();
        let ctx = JobContext {
            run_id,
            task_name: name,
            db: db.clone(),
            events: self.0.events.clone(),
            cancel: cancel.clone(),
        };
        let fut = (job.run)(ctx);
        let outcome = tokio::select! {
            r = tokio::time::timeout(job.timeout, fut) => match r {
                Ok(r) => r,
                Err(_) => {
                    cancel.cancel();
                    Err(format!("timed out after {} s", job.timeout.as_secs()))
                }
            },
            _ = self.0.cancel.cancelled() => Err("cancelled by shutdown".into()),
        };
        let duration = started.elapsed().as_millis() as i64;
        let (status, message, error) = match &outcome {
            Ok(msg) => ("completed", msg.clone(), None),
            Err(e) if e == "cancelled by shutdown" => ("cancelled", None, Some(e.clone())),
            Err(e) => ("failed", None, Some(e.clone())),
        };
        match &outcome {
            Ok(_) => tracing::info!(
                task = name,
                trigger,
                duration_ms = duration,
                "task completed"
            ),
            Err(e) => {
                tracing::warn!(task = name, trigger, duration_ms = duration, error = %e, "task did not complete")
            }
        }
        if let Err(e) = tasks::finish_run(
            db,
            run_id,
            status,
            message.as_deref(),
            error.as_deref(),
            duration,
        )
        .await
        {
            tracing::error!(task = name, error = %e, "cannot record task result");
        }
        publish(status, message.or(error));
    }

    /// Cancels running jobs and waits up to `grace` for them to finish.
    pub async fn shutdown(&self, grace: Duration) {
        self.0.cancel.cancel();
        self.0.tracker.close();
        if tokio::time::timeout(grace, self.0.tracker.wait())
            .await
            .is_err()
        {
            tracing::warn!("background tasks did not stop within {} s", grace.as_secs());
        }
    }
}

impl kitsunarr_api::TaskControl for Scheduler {
    fn describe(&self, name: &str) -> Option<String> {
        self.0.jobs.get(name).map(|j| j.description.to_string())
    }

    fn is_running(&self, name: &str) -> bool {
        self.0
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(name)
    }

    fn trigger(&self, name: &str) -> bool {
        self.spawn_run(name, "manual")
    }

    fn next_run(&self, name: &str) -> Option<String> {
        self.0
            .next_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    }
}

/// Convenience constructor for jobs written as async closures.
pub fn job<F, Fut>(
    name: &'static str,
    description: &'static str,
    interval_minutes: i64,
    timeout: Duration,
    f: F,
) -> Job
where
    F: Fn(JobContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = JobResult> + Send + 'static,
{
    Job {
        name,
        description,
        default_interval_minutes: interval_minutes,
        timeout,
        run: Arc::new(move |ctx| Box::pin(f(ctx))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kitsunarr_api::TaskControl;
    use std::sync::atomic::{AtomicU32, Ordering};

    async fn db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("s.db")).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn scheduled_and_manual_runs_are_recorded_without_overlap() {
        let (_dir, db) = db().await;
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let slow = job("Slow", "test job", 60, Duration::from_secs(5), move |ctx| {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                ctx.progress(0.5, "halfway").await;
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(Some("done".into()))
            }
        });
        let failing = job("Fail", "fails", 60, Duration::from_secs(5), |_| async {
            Err("boom".to_string())
        });
        let sched = Scheduler::new(
            db.clone(),
            EventBus::default(),
            vec![slow, failing],
            CancellationToken::new(),
        );
        sched.start(Duration::from_millis(50)).await.unwrap();
        // Simulate tasks that have never run, so the next tick schedules both immediately.
        sqlx::query("UPDATE scheduled_tasks SET last_run_at = NULL")
            .execute(db.pool())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(sched.is_running("Slow"));
        assert!(!sched.trigger("Slow"), "no overlap while running");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!sched.is_running("Slow"));
        assert!(sched.next_run("Slow").is_some());
        assert!(sched.trigger("Slow"), "manual trigger after completion");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "interval not yet elapsed, so only the manual run repeated"
        );
        let runs = tasks::recent_runs(db.pool(), 10).await.unwrap();
        assert!(
            runs.iter()
                .any(|r| r.task_name == "Fail" && r.status == "failed")
        );
        assert_eq!(
            runs.iter()
                .filter(|r| r.task_name == "Slow" && r.status == "completed")
                .count(),
            2
        );
        sched.shutdown(Duration::from_secs(2)).await;
        assert!(!sched.trigger("Slow"), "no new runs after shutdown");
    }

    #[tokio::test]
    async fn timeouts_and_shutdown_cancel_jobs() {
        let (_dir, db) = db().await;
        let hang = job(
            "Hang",
            "never finishes",
            60,
            Duration::from_millis(100),
            |ctx| async move {
                ctx.cancel.cancelled().await;
                Err("cancelled".into())
            },
        );
        let sched = Scheduler::new(
            db.clone(),
            EventBus::default(),
            vec![hang],
            CancellationToken::new(),
        );
        sched.start(Duration::from_millis(30)).await.unwrap();
        assert!(sched.trigger("Hang"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let runs = tasks::recent_runs(db.pool(), 10).await.unwrap();
        assert!(runs.iter().any(|r| r.status == "failed"), "{runs:?}");
        sched.shutdown(Duration::from_secs(2)).await;
    }
}
