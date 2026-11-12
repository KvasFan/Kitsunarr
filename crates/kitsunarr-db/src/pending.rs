//! Pending releases (Sonarr's `PendingReleaseService`): releases held back by a delay profile.

use sqlx::{FromRow, SqlitePool};

use crate::{DbResult, now};

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct PendingRow {
    pub id: i64,
    pub series_id: i64,
    pub indexer_id: i64,
    pub guid: String,
    pub title: String,
    /// The release as JSON (the API layer's shape).
    pub release: String,
    /// JSON list of episode ids.
    pub episode_ids: String,
    pub added_at: String,
}

pub async fn list(pool: &SqlitePool) -> DbResult<Vec<PendingRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM pending_releases ORDER BY added_at, id")
            .fetch_all(pool)
            .await?,
    )
}

/// Adds or refreshes a pending release (keeping when it was first held).
pub async fn upsert(
    pool: &SqlitePool,
    series_id: i64,
    indexer_id: i64,
    guid: &str,
    title: &str,
    release: &str,
    episode_ids: &[i64],
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO pending_releases (series_id, indexer_id, guid, title, release, episode_ids, added_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (indexer_id, guid) DO UPDATE SET release = excluded.release, \
         episode_ids = excluded.episode_ids, series_id = excluded.series_id, title = excluded.title",
    )
    .bind(series_id)
    .bind(indexer_id)
    .bind(guid)
    .bind(title)
    .bind(release)
    .bind(serde_json::to_string(episode_ids).unwrap_or_else(|_| "[]".into()))
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, indexer_id: i64, guid: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM pending_releases WHERE indexer_id = ? AND guid = ?")
        .bind(indexer_id)
        .bind(guid)
        .execute(pool)
        .await?;
    Ok(())
}

/// Removes pending releases for any of these episodes (a release was grabbed for them).
pub async fn remove_for_episodes(pool: &SqlitePool, episode_ids: &[i64]) -> DbResult<u64> {
    if episode_ids.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query(
        "DELETE FROM pending_releases WHERE EXISTS (SELECT 1 FROM json_each(pending_releases.episode_ids) p \
         WHERE p.value IN (SELECT value FROM json_each(?)))",
    )
    .bind(serde_json::to_string(episode_ids).unwrap_or_else(|_| "[]".into()))
    .execute(pool)
    .await?
    .rows_affected())
}
