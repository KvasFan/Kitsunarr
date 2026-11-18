//! History (grabs, imports, deletions, renames, failures).

use kitsunarr_domain::api::library::{HistoryFilter, HistoryItem};
use kitsunarr_domain::{Page, PageRequest, Quality, SortDirection};
use sqlx::{FromRow, SqliteConnection};

use crate::{Database, DbResult, to_json};

#[derive(Debug, Clone, PartialEq)]
pub struct NewHistory {
    pub event_type: String,
    pub date: String,
    pub source_title: String,
    pub series_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub quality_id: Option<i32>,
    pub revision: Option<serde_json::Value>,
    pub languages: Vec<i32>,
    pub custom_format_score: Option<i32>,
    pub download_id: Option<String>,
    pub data: serde_json::Value,
}

pub async fn insert(conn: &mut SqliteConnection, h: &NewHistory) -> DbResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO history (event_type, date, source_title, series_id, episode_id, movie_id, quality_id, revision, languages, \
         custom_format_score, download_id, data) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&h.event_type)
    .bind(&h.date)
    .bind(&h.source_title)
    .bind(h.series_id)
    .bind(h.episode_id)
    .bind(h.movie_id)
    .bind(h.quality_id)
    .bind(h.revision.as_ref().map(to_json).transpose()?)
    .bind(to_json(&h.languages)?)
    .bind(h.custom_format_score)
    .bind(&h.download_id)
    .bind(to_json(&h.data)?)
    .fetch_one(&mut *conn)
    .await?)
}

/// A "grabbed" history entry, for tracking downloads.
#[derive(Debug, Clone, FromRow)]
pub struct GrabRecord {
    pub download_id: String,
    pub date: String,
    pub source_title: String,
    pub series_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub quality_id: Option<i64>,
    /// JSON `Revision`.
    pub revision: Option<String>,
    /// JSON list of language ids.
    pub languages: Option<String>,
    pub data: String,
    /// A film's grab (Tanukarr's movies).
    #[sqlx(default)]
    pub movie_id: Option<i64>,
}

/// The grabs of the given download ids (compared case-insensitively), oldest first.
pub async fn grabs_for(db: &Database, download_ids: &[String]) -> DbResult<Vec<GrabRecord>> {
    if download_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = sqlx::QueryBuilder::new(
        "SELECT download_id, date, source_title, series_id, episode_id, quality_id, revision, languages, data, movie_id \
         FROM history WHERE event_type = 'grabbed' AND download_id IS NOT NULL AND upper(download_id) IN (",
    );
    let mut sep = q.separated(", ");
    for id in download_ids {
        sep.push_bind(id.to_ascii_uppercase());
    }
    q.push(") ORDER BY date, id");
    Ok(q.build_query_as().fetch_all(db.pool()).await?)
}

/// An episode's most recent history event, when it is a grab.
#[derive(Debug, Clone, FromRow)]
pub struct LastGrab {
    pub episode_id: i64,
    pub date: String,
    pub source_title: String,
    pub quality_id: Option<i64>,
    /// JSON `Revision`.
    pub revision: Option<String>,
    /// JSON list of language ids.
    pub languages: Option<String>,
    pub data: String,
}

/// The series' episodes whose most recent history event (`MostRecentForEpisode`: latest date,
/// then latest entry) is a grab.
pub async fn last_grabs_for_series(db: &Database, series_id: i64) -> DbResult<Vec<LastGrab>> {
    Ok(sqlx::query_as(
        "SELECT h.episode_id, h.date, h.source_title, h.quality_id, h.revision, h.languages, h.data \
         FROM history h WHERE h.series_id = ? AND h.episode_id IS NOT NULL AND h.event_type = 'grabbed' \
         AND NOT EXISTS (SELECT 1 FROM history n WHERE n.series_id = h.series_id \
           AND n.episode_id = h.episode_id AND (n.date > h.date OR (n.date = h.date AND n.id > h.id)))",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await?)
}

/// A grab or download-folder import of one of a series' episodes.
#[derive(Debug, Clone, FromRow)]
pub struct GrabOrImport {
    pub episode_id: i64,
    pub event_type: String,
    pub source_title: String,
    pub quality_id: Option<i64>,
    /// JSON `Revision`.
    pub revision: Option<String>,
    pub download_id: Option<String>,
}

/// The series' grabs and download-folder imports, newest first (`FindByEpisodeId` orders by
/// date, descending), for `AlreadyImportedSpecification`.
pub async fn grabs_and_imports(db: &Database, series_id: i64) -> DbResult<Vec<GrabOrImport>> {
    Ok(sqlx::query_as(
        "SELECT episode_id, event_type, source_title, quality_id, revision, download_id FROM history \
         WHERE series_id = ? AND episode_id IS NOT NULL \
           AND event_type IN ('grabbed', 'downloadFolderImported') \
         ORDER BY date DESC, id DESC",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await?)
}

/// Download ids (upper-case) among `download_ids` that were imported: a
/// `downloadFolderImported` entry newer than their last grab.
/// A series' grabs and failed downloads, newest first: (event type, date, release guid,
/// download id). Sonarr v5 reports them on interactive search results (`ReleaseHistoryResource`).
pub async fn grab_and_failure_events(
    db: &Database,
    series_id: i64,
) -> DbResult<Vec<(String, String, Option<String>, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT event_type, date, json_extract(data, '$.guid'), download_id FROM history \
         WHERE series_id = ? AND event_type IN ('grabbed', 'downloadFailed') ORDER BY date DESC",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await?)
}

pub async fn imported_downloads(
    db: &Database,
    download_ids: &[String],
) -> DbResult<std::collections::HashSet<String>> {
    if download_ids.is_empty() {
        return Ok(Default::default());
    }
    let mut q = sqlx::QueryBuilder::new(
        "SELECT upper(download_id) FROM history h WHERE event_type = 'downloadFolderImported' \
         AND download_id IS NOT NULL AND upper(download_id) IN (",
    );
    let mut sep = q.separated(", ");
    for id in download_ids {
        sep.push_bind(id.to_ascii_uppercase());
    }
    q.push(
        ") AND NOT EXISTS (SELECT 1 FROM history g WHERE g.event_type = 'grabbed' \
         AND upper(g.download_id) = upper(h.download_id) AND g.id > h.id)",
    );
    let rows: Vec<(String,)> = q.build_query_as().fetch_all(db.pool()).await?;
    Ok(rows.into_iter().map(|(d,)| d).collect())
}

/// Download ids (upper-case) among `download_ids` that failed: a `downloadFailed` entry newer
/// than their last grab.
pub async fn failed_downloads(
    db: &Database,
    download_ids: &[String],
) -> DbResult<std::collections::HashSet<String>> {
    if download_ids.is_empty() {
        return Ok(Default::default());
    }
    let mut q = sqlx::QueryBuilder::new(
        "SELECT upper(download_id) FROM history h WHERE event_type = 'downloadFailed' \
         AND download_id IS NOT NULL AND upper(download_id) IN (",
    );
    let mut sep = q.separated(", ");
    for id in download_ids {
        sep.push_bind(id.to_ascii_uppercase());
    }
    q.push(
        ") AND NOT EXISTS (SELECT 1 FROM history g WHERE g.event_type = 'grabbed' \
         AND upper(g.download_id) = upper(h.download_id) AND g.id > h.id)",
    );
    let rows: Vec<(String,)> = q.build_query_as().fetch_all(db.pool()).await?;
    Ok(rows.into_iter().map(|(d,)| d).collect())
}

/// A history entry by id, for "mark as failed".
pub async fn get(db: &Database, id: i64) -> DbResult<(String, Option<String>)> {
    sqlx::query_as("SELECT event_type, download_id FROM history WHERE id = ?")
        .bind(id)
        .fetch_optional(db.pool())
        .await?
        .ok_or(crate::DbError::NotFound)
}

#[derive(FromRow)]
struct Row {
    id: i64,
    event_type: String,
    date: String,
    source_title: String,
    series_id: Option<i64>,
    series_title: Option<String>,
    episode_id: Option<i64>,
    movie_id: Option<i64>,
    movie_title: Option<String>,
    quality_id: Option<i64>,
    download_id: Option<String>,
    data: String,
}

pub async fn list(
    db: &Database,
    paging: &PageRequest,
    event_type: Option<&str>,
) -> DbResult<Page<HistoryItem>> {
    list_filtered(
        db,
        paging,
        &HistoryFilter {
            event_type: event_type.map(str::to_string),
            ..Default::default()
        },
    )
    .await
}

fn conditions(query: &mut sqlx::QueryBuilder<sqlx::Sqlite>, filter: &HistoryFilter) {
    query.push(" WHERE 1 = 1");
    if let Some(event) = &filter.event_type {
        query.push(" AND h.event_type = ").push_bind(event.clone());
    }
    if let Some(id) = filter.series_id {
        query.push(" AND h.series_id = ").push_bind(id);
    }
    if let Some(id) = filter.episode_id {
        query.push(" AND h.episode_id = ").push_bind(id);
    }
    if let Some(id) = filter.movie_id {
        query.push(" AND h.movie_id = ").push_bind(id);
    }
    if let Some(id) = &filter.download_id {
        query
            .push(" AND h.download_id = ")
            .push_bind(id.clone())
            .push(" COLLATE NOCASE");
    }
}

pub async fn list_filtered(
    db: &Database,
    paging: &PageRequest,
    filter: &HistoryFilter,
) -> DbResult<Page<HistoryItem>> {
    // Count and rows belong to one SQLite snapshot, even while new history is being written.
    let mut tx = db.pool().begin().await?;
    let mut count = sqlx::QueryBuilder::new("SELECT COUNT(*) FROM history h");
    conditions(&mut count, filter);
    let total: i64 = count.build_query_scalar().fetch_one(&mut *tx).await?;
    let order = if paging.sort_direction == Some(SortDirection::Asc) {
        "ASC"
    } else {
        "DESC"
    };
    let mut query = sqlx::QueryBuilder::new(
        "SELECT h.id, h.event_type, h.date, h.source_title, h.series_id, s.title AS series_title, h.episode_id, h.movie_id, m.title AS movie_title, h.quality_id, h.download_id, h.data FROM history h LEFT JOIN series s ON s.id = h.series_id LEFT JOIN movies m ON m.id = h.movie_id",
    );
    conditions(&mut query, filter);
    query
        .push(format!(" ORDER BY h.date {order}, h.id {order} LIMIT "))
        .push_bind(i64::from(paging.page_size()))
        .push(" OFFSET ")
        .push_bind(paging.offset() as i64);
    let rows: Vec<Row> = query.build_query_as().fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(Page {
        page: paging.page(),
        page_size: paging.page_size(),
        total_records: total as u64,
        records: rows
            .into_iter()
            .map(|r| HistoryItem {
                id: r.id,
                event_type: r.event_type,
                date: r.date,
                source_title: r.source_title,
                series_id: r.series_id,
                series_title: r.series_title,
                episode_id: r.episode_id,
                movie_id: r.movie_id,
                movie_title: r.movie_title,
                quality_name: r
                    .quality_id
                    .and_then(|q| Quality::by_id(q as i32))
                    .map(|q| q.name.to_string()),
                download_id: r.download_id,
                data: serde_json::from_str(&r.data).unwrap_or(serde_json::Value::Null),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_and_page() {
        let db = crate::test_support::db().await;
        let mut conn = db.pool().acquire().await.unwrap();
        for i in 0..5 {
            insert(
                &mut conn,
                &NewHistory {
                    event_type: if i % 2 == 0 {
                        "grabbed".into()
                    } else {
                        "downloadFolderImported".into()
                    },
                    date: format!("2026-01-0{}T00:00:00Z", i + 1),
                    source_title: format!("Release {i}"),
                    series_id: None,
                    episode_id: None,
                    movie_id: None,
                    quality_id: Some(7),
                    revision: None,
                    languages: vec![1],
                    custom_format_score: None,
                    download_id: Some(format!("hash{i}")),
                    data: serde_json::json!({"indexer": "Nyaa"}),
                },
            )
            .await
            .unwrap();
        }
        drop(conn);
        let page = list(
            &db,
            &PageRequest {
                page_size: Some(2),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(page.total_records, 5);
        assert_eq!(
            page.records[0].source_title, "Release 4",
            "newest first by default"
        );
        let grabs = list(&db, &PageRequest::default(), Some("grabbed"))
            .await
            .unwrap();
        assert_eq!(grabs.total_records, 3);
        assert_eq!(
            grabs.records[0].quality_name.as_deref(),
            Some("Bluray-1080p")
        );
    }
}
