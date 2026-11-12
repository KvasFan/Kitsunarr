//! Import list membership and sync status (Sonarr's `ImportListItemService` and
//! `ImportListStatusService`).

use chrono::{DateTime, Duration, Utc};
use sqlx::{FromRow, SqlitePool};

use crate::{DbResult, now};

/// The catalogue ids of an item: TVDB, TMDB, IMDb, MyAnimeList, AniList.
type Identity = (
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

/// One show a list returned at its last complete read.
#[derive(Debug, Clone, Default, PartialEq, Eq, FromRow)]
pub struct ListItemRow {
    pub import_list_id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub imdb_id: Option<String>,
    pub mal_id: Option<i64>,
    pub anilist_id: Option<i64>,
}

impl ListItemRow {
    fn identity(&self) -> Identity {
        (
            self.tvdb_id,
            self.tmdb_id,
            self.imdb_id.clone(),
            self.mal_id,
            self.anilist_id,
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, FromRow)]
pub struct ListStatus {
    pub import_list_id: i64,
    pub last_info_sync: Option<String>,
    pub initial_failure: Option<String>,
    pub most_recent_failure: Option<String>,
    pub escalation_level: i64,
    pub disabled_till: Option<String>,
    pub last_error: Option<String>,
    pub has_removed_item_since_last_clean: bool,
}

impl ListStatus {
    pub fn disabled_till(&self) -> Option<DateTime<Utc>> {
        self.disabled_till
            .as_deref()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&Utc))
    }

    pub fn last_info_sync(&self) -> Option<DateTime<Utc>> {
        self.last_info_sync
            .as_deref()
            .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
            .map(|d| d.with_timezone(&Utc))
    }
}

/// Sonarr's `EscalationBackOff.Periods` (seconds), indexed by escalation level.
const BACKOFF: [i64; 12] = [
    0, 60, 300, 600, 900, 1800, 3600, 7200, 14400, 28800, 43200, 86400,
];

pub async fn items(pool: &SqlitePool, list_id: Option<i64>) -> DbResult<Vec<ListItemRow>> {
    Ok(sqlx::query_as(
        "SELECT import_list_id, title, year, tvdb_id, tmdb_id, imdb_id, mal_id, anilist_id \
         FROM import_list_items WHERE ?1 IS NULL OR import_list_id = ?1 ORDER BY id",
    )
    .bind(list_id)
    .fetch_all(pool)
    .await?)
}

/// `SyncSeriesForList`: replaces a list's membership; returns how many items left the list.
pub async fn replace_items(pool: &SqlitePool, list_id: i64, new: &[ListItemRow]) -> DbResult<u64> {
    let mut tx = pool.begin().await?;
    let old: Vec<ListItemRow> = sqlx::query_as(
        "SELECT import_list_id, title, year, tvdb_id, tmdb_id, imdb_id, mal_id, anilist_id \
         FROM import_list_items WHERE import_list_id = ?",
    )
    .bind(list_id)
    .fetch_all(&mut *tx)
    .await?;
    let kept: std::collections::HashSet<_> = new.iter().map(ListItemRow::identity).collect();
    let removed = old.iter().filter(|o| !kept.contains(&o.identity())).count() as u64;
    sqlx::query("DELETE FROM import_list_items WHERE import_list_id = ?")
        .bind(list_id)
        .execute(&mut *tx)
        .await?;
    for i in new {
        sqlx::query(
            "INSERT INTO import_list_items (import_list_id, title, year, tvdb_id, tmdb_id, imdb_id, mal_id, anilist_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(list_id)
        .bind(&i.title)
        .bind(i.year)
        .bind(i.tvdb_id)
        .bind(i.tmdb_id)
        .bind(&i.imdb_id)
        .bind(i.mal_id)
        .bind(i.anilist_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(removed)
}

pub async fn status(pool: &SqlitePool, list_id: i64) -> DbResult<ListStatus> {
    Ok(
        sqlx::query_as("SELECT * FROM import_list_status WHERE import_list_id = ?")
            .bind(list_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(ListStatus {
                import_list_id: list_id,
                ..Default::default()
            }),
    )
}

pub async fn all_status(pool: &SqlitePool) -> DbResult<Vec<ListStatus>> {
    Ok(sqlx::query_as("SELECT * FROM import_list_status")
        .fetch_all(pool)
        .await?)
}

async fn save(pool: &SqlitePool, s: &ListStatus) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO import_list_status (import_list_id, last_info_sync, initial_failure, most_recent_failure, \
         escalation_level, disabled_till, last_error, has_removed_item_since_last_clean) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (import_list_id) DO UPDATE SET last_info_sync = excluded.last_info_sync, \
         initial_failure = excluded.initial_failure, most_recent_failure = excluded.most_recent_failure, \
         escalation_level = excluded.escalation_level, disabled_till = excluded.disabled_till, \
         last_error = excluded.last_error, has_removed_item_since_last_clean = excluded.has_removed_item_since_last_clean",
    )
    .bind(s.import_list_id)
    .bind(&s.last_info_sync)
    .bind(&s.initial_failure)
    .bind(&s.most_recent_failure)
    .bind(s.escalation_level)
    .bind(&s.disabled_till)
    .bind(&s.last_error)
    .bind(s.has_removed_item_since_last_clean)
    .execute(pool)
    .await?;
    Ok(())
}

/// A complete read: clears the failure state and records the sync time (`UpdateListSyncStatus`).
pub async fn record_sync(pool: &SqlitePool, list_id: i64, removed_items: bool) -> DbResult<()> {
    let mut s = status(pool, list_id).await?;
    s.escalation_level = 0;
    s.initial_failure = None;
    s.most_recent_failure = None;
    s.disabled_till = None;
    s.last_error = None;
    s.last_info_sync = Some(now());
    s.has_removed_item_since_last_clean |= removed_items;
    save(pool, &s).await
}

/// A failed read: escalates the back-off (`RecordFailure`); `retry_after` (a rate limit's
/// Retry-After) replaces the period when given.
pub async fn record_failure(
    pool: &SqlitePool,
    list_id: i64,
    error: &str,
    retry_after: Option<Duration>,
) -> DbResult<()> {
    let mut s = status(pool, list_id).await?;
    let t = Utc::now();
    s.escalation_level = (s.escalation_level + 1).min(BACKOFF.len() as i64 - 1);
    if s.initial_failure.is_none() {
        s.initial_failure = Some(now());
    }
    s.most_recent_failure = Some(now());
    let period = retry_after.unwrap_or(Duration::seconds(BACKOFF[s.escalation_level as usize]));
    s.disabled_till = Some((t + period).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    s.last_error = Some(error.to_string());
    save(pool, &s).await
}

pub async fn mark_cleaned(pool: &SqlitePool) -> DbResult<()> {
    sqlx::query("UPDATE import_list_status SET has_removed_item_since_last_clean = 0")
        .execute(pool)
        .await?;
    Ok(())
}

/// A library series with every catalogue identity and tag (for list matching).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeriesIdentity {
    pub id: i64,
    pub title: String,
    pub provider: String,
    pub metadata_id: String,
    pub monitored: bool,
    pub tags: Vec<i64>,
    /// `(kind, id)` pairs from `series_external_ids`.
    pub external: Vec<(String, String)>,
}

pub async fn series_identities(pool: &SqlitePool) -> DbResult<Vec<SeriesIdentity>> {
    let rows: Vec<(i64, String, String, String, bool)> = sqlx::query_as(
        "SELECT id, title, metadata_provider, metadata_id, monitored FROM series ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let ext: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT series_id, kind, external_id FROM series_external_ids")
            .fetch_all(pool)
            .await?;
    let tags: Vec<(i64, i64)> = sqlx::query_as("SELECT series_id, tag_id FROM series_tags")
        .fetch_all(pool)
        .await?;
    let mut out: Vec<SeriesIdentity> = rows
        .into_iter()
        .map(
            |(id, title, provider, metadata_id, monitored)| SeriesIdentity {
                id,
                title,
                provider,
                metadata_id,
                monitored,
                ..Default::default()
            },
        )
        .collect();
    let index: std::collections::HashMap<i64, usize> =
        out.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    for (sid, kind, value) in ext {
        if let Some(&i) = index.get(&sid) {
            out[i].external.push((kind, value));
        }
    }
    for (sid, tag) in tags {
        if let Some(&i) = index.get(&sid) {
            out[i].tags.push(tag);
        }
    }
    Ok(out)
}
