//! Bookkeeping for Sonarr/Tanukarr migrations: runs, source→target id map, unmapped values.

use sqlx::{FromRow, SqliteConnection, SqliteExecutor};

use crate::{DbResult, now, to_json};

#[derive(Debug, Clone, FromRow)]
pub struct MigrationRun {
    pub id: i64,
    pub source_app: String,
    pub source_method: String,
    pub source_version: String,
    pub source_instance: Option<String>,
    pub source_fingerprint: String,
    pub status: String,
    pub options: String,
    pub summary: Option<String>,
    pub report: Option<String>,
    pub backup_path: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

pub struct NewRun<'a> {
    pub source_app: &'a str,
    pub source_method: &'a str,
    pub source_version: &'a str,
    pub source_instance: Option<&'a str>,
    pub source_fingerprint: &'a str,
    pub options: &'a serde_json::Value,
    pub backup_path: Option<&'a str>,
}

pub async fn start(ex: impl SqliteExecutor<'_>, r: NewRun<'_>) -> DbResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO migration_runs (source_app, source_method, source_version, source_instance, source_fingerprint, status, \
         options, backup_path, started_at) VALUES (?, ?, ?, ?, ?, 'running', ?, ?, ?) RETURNING id",
    )
    .bind(r.source_app)
    .bind(r.source_method)
    .bind(r.source_version)
    .bind(r.source_instance)
    .bind(r.source_fingerprint)
    .bind(to_json(r.options)?)
    .bind(r.backup_path)
    .bind(now())
    .fetch_one(ex)
    .await?)
}

pub async fn finish(
    ex: impl SqliteExecutor<'_>,
    id: i64,
    status: &str,
    summary: &serde_json::Value,
    report: &serde_json::Value,
    error: Option<&str>,
) -> DbResult<()> {
    sqlx::query("UPDATE migration_runs SET status = ?, summary = ?, report = ?, error = ?, finished_at = ? WHERE id = ?")
        .bind(status)
        .bind(to_json(summary)?)
        .bind(to_json(report)?)
        .bind(error)
        .bind(now())
        .bind(id)
        .execute(ex)
        .await?;
    Ok(())
}

pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<MigrationRun>> {
    Ok(
        sqlx::query_as("SELECT * FROM migration_runs ORDER BY id DESC LIMIT 100")
            .fetch_all(ex)
            .await?,
    )
}

pub async fn get(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<Option<MigrationRun>> {
    Ok(sqlx::query_as("SELECT * FROM migration_runs WHERE id = ?")
        .bind(id)
        .fetch_optional(ex)
        .await?)
}

/// Marks runs left `running` by a crash as failed.
pub async fn fail_interrupted(ex: impl SqliteExecutor<'_>) -> DbResult<u64> {
    let r = sqlx::query(
        "UPDATE migration_runs SET status = 'failed', error = 'interrupted', finished_at = ? WHERE status = 'running'",
    )
    .bind(now())
    .execute(ex)
    .await?;
    Ok(r.rows_affected())
}

pub async fn lookup(
    conn: &mut SqliteConnection,
    fingerprint: &str,
    entity: &str,
    source_id: &str,
) -> DbResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT target_id FROM migration_entity_map WHERE source_fingerprint = ? AND entity = ? AND source_id = ?",
    )
    .bind(fingerprint)
    .bind(entity)
    .bind(source_id)
    .fetch_optional(&mut *conn)
    .await?)
}

pub async fn record(
    conn: &mut SqliteConnection,
    fingerprint: &str,
    entity: &str,
    source_id: &str,
    target_id: i64,
    run_id: i64,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO migration_entity_map (source_fingerprint, entity, source_id, target_id, run_id) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(source_fingerprint, entity, source_id) DO UPDATE SET target_id = excluded.target_id, run_id = excluded.run_id",
    )
    .bind(fingerprint)
    .bind(entity)
    .bind(source_id)
    .bind(target_id)
    .bind(run_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn record_unmapped(
    conn: &mut SqliteConnection,
    run_id: i64,
    entity: &str,
    source_id: Option<&str>,
    field: &str,
    value: &serde_json::Value,
) -> DbResult<()> {
    sqlx::query("INSERT INTO migration_unmapped (run_id, entity, source_id, field, value) VALUES (?, ?, ?, ?, ?)")
        .bind(run_id)
        .bind(entity)
        .bind(source_id)
        .bind(field)
        .bind(to_json(value)?)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

pub async fn unmapped_count(ex: impl SqliteExecutor<'_>, run_id: i64) -> DbResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM migration_unmapped WHERE run_id = ?")
            .bind(run_id)
            .fetch_one(ex)
            .await?,
    )
}
