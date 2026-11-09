//! Scheduled task definitions and execution history.

use kitsunarr_domain::api::system::TaskRunItem;
use sqlx::{FromRow, SqliteExecutor};

use crate::{DbResult, now};

#[derive(Debug, Clone, FromRow)]
pub struct ScheduledTask {
    pub name: String,
    pub interval_minutes: i64,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_duration_ms: Option<i64>,
    pub last_status: Option<String>,
}

/// Registers a task. A newly registered task counts as having just run, so its first
/// scheduled execution happens one interval after installation (manual runs are always possible).
pub async fn ensure(
    ex: impl SqliteExecutor<'_>,
    name: &str,
    interval_minutes: i64,
) -> DbResult<()> {
    sqlx::query("INSERT OR IGNORE INTO scheduled_tasks (name, interval_minutes, last_run_at) VALUES (?, ?, ?)")
        .bind(name)
        .bind(interval_minutes)
        .bind(now())
        .execute(ex)
        .await?;
    Ok(())
}

pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<ScheduledTask>> {
    Ok(
        sqlx::query_as("SELECT * FROM scheduled_tasks ORDER BY name")
            .fetch_all(ex)
            .await?,
    )
}

pub async fn set_interval(
    ex: impl SqliteExecutor<'_>,
    name: &str,
    interval_minutes: i64,
    enabled: bool,
) -> DbResult<bool> {
    let r =
        sqlx::query("UPDATE scheduled_tasks SET interval_minutes = ?, enabled = ? WHERE name = ?")
            .bind(interval_minutes)
            .bind(enabled)
            .bind(name)
            .execute(ex)
            .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn start_run(ex: impl SqliteExecutor<'_>, name: &str, trigger: &str) -> DbResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO task_runs (task_name, trigger, status, started_at) VALUES (?, ?, 'running', ?) RETURNING id",
    )
    .bind(name)
    .bind(trigger)
    .bind(now())
    .fetch_one(ex)
    .await?)
}

pub async fn progress(
    ex: impl SqliteExecutor<'_>,
    run_id: i64,
    progress: f64,
    message: Option<&str>,
) -> DbResult<()> {
    sqlx::query("UPDATE task_runs SET progress = ?, message = COALESCE(?, message) WHERE id = ?")
        .bind(progress.clamp(0.0, 1.0))
        .bind(message)
        .bind(run_id)
        .execute(ex)
        .await?;
    Ok(())
}

/// Marks a run finished and records the outcome on the task definition.
pub async fn finish_run(
    db: &crate::Database,
    run_id: i64,
    status: &str,
    message: Option<&str>,
    error: Option<&str>,
    duration_ms: i64,
) -> DbResult<()> {
    let ts = now();
    let mut tx = db.pool().begin().await?;
    let name: String = sqlx::query_scalar(
        "UPDATE task_runs SET status = ?, finished_at = ?, message = COALESCE(?, message), error = ?, \
         progress = CASE WHEN ? = 'completed' THEN 1.0 ELSE progress END WHERE id = ? RETURNING task_name",
    )
    .bind(status)
    .bind(&ts)
    .bind(message)
    .bind(error)
    .bind(status)
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE scheduled_tasks SET last_run_at = ?, last_duration_ms = ?, last_status = ?, \
         last_success_at = CASE WHEN ? = 'completed' THEN ? ELSE last_success_at END WHERE name = ?",
    )
    .bind(&ts)
    .bind(duration_ms)
    .bind(status)
    .bind(status)
    .bind(&ts)
    .bind(&name)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Runs left `running` by a crash or kill are marked failed at start-up.
pub async fn fail_interrupted(ex: impl SqliteExecutor<'_>) -> DbResult<u64> {
    let r = sqlx::query(
        "UPDATE task_runs SET status = 'failed', finished_at = ?, error = 'interrupted by shutdown' WHERE status IN ('queued', 'running')",
    )
    .bind(now())
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

#[derive(FromRow)]
struct RunRow {
    id: i64,
    task_name: String,
    trigger: String,
    status: String,
    started_at: String,
    finished_at: Option<String>,
    message: Option<String>,
    progress: Option<f64>,
}

pub async fn recent_runs(ex: impl SqliteExecutor<'_>, limit: i64) -> DbResult<Vec<TaskRunItem>> {
    let rows: Vec<RunRow> = sqlx::query_as(
        "SELECT id, task_name, trigger, status, started_at, finished_at, message, progress FROM task_runs ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TaskRunItem {
            id: r.id,
            task_name: r.task_name,
            trigger: r.trigger,
            status: r.status,
            started_at: r.started_at,
            finished_at: r.finished_at,
            message: r.message,
            progress: r.progress,
        })
        .collect())
}

/// Keeps the most recent `keep` runs.
pub async fn prune_runs(ex: impl SqliteExecutor<'_>, keep: i64) -> DbResult<u64> {
    let r = sqlx::query(
        "DELETE FROM task_runs WHERE id NOT IN (SELECT id FROM task_runs ORDER BY id DESC LIMIT ?)",
    )
    .bind(keep)
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_lifecycle() {
        let db = crate::test_support::db().await;
        ensure(db.pool(), "Test", 5).await.unwrap();
        let run = start_run(db.pool(), "Test", "manual").await.unwrap();
        progress(db.pool(), run, 0.5, Some("half")).await.unwrap();
        finish_run(&db, run, "completed", None, None, 12)
            .await
            .unwrap();
        let t = list(db.pool())
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.name == "Test")
            .unwrap();
        assert_eq!(t.last_status.as_deref(), Some("completed"));
        assert!(t.last_success_at.is_some());
        let runs = recent_runs(db.pool(), 10).await.unwrap();
        assert_eq!(runs[0].progress, Some(1.0));
        assert_eq!(runs[0].message.as_deref(), Some("half"));
        start_run(db.pool(), "Test", "scheduled").await.unwrap();
        assert_eq!(fail_interrupted(db.pool()).await.unwrap(), 1);
        assert_eq!(prune_runs(db.pool(), 1).await.unwrap(), 1);
    }
}
