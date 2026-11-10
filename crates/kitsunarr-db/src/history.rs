//! History (grabs, imports, deletions, renames, failures).

use kitsunarr_domain::api::library::HistoryItem;
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
    pub data: String,
}

/// The grabs of the given download ids (compared case-insensitively), oldest first.
pub async fn grabs_for(db: &Database, download_ids: &[String]) -> DbResult<Vec<GrabRecord>> {
    if download_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = sqlx::QueryBuilder::new(
        "SELECT download_id, date, source_title, series_id, episode_id, quality_id, data FROM history \
         WHERE event_type = 'grabbed' AND download_id IS NOT NULL AND upper(download_id) IN (",
    );
    let mut sep = q.separated(", ");
    for id in download_ids {
        sep.push_bind(id.to_ascii_uppercase());
    }
    q.push(") ORDER BY date, id");
    Ok(q.build_query_as().fetch_all(db.pool()).await?)
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
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM history WHERE (?1 IS NULL OR event_type = ?1)")
            .bind(event_type)
            .fetch_one(db.pool())
            .await?;
    // Newest first unless the caller explicitly asks for ascending order.
    let order = if paging.sort_direction == Some(SortDirection::Asc) {
        "ASC"
    } else {
        "DESC"
    };
    let sql = format!(
        "SELECT h.id, h.event_type, h.date, h.source_title, h.series_id, s.title AS series_title, h.episode_id, h.movie_id, \
         m.title AS movie_title, h.quality_id, h.download_id, h.data FROM history h \
         LEFT JOIN series s ON s.id = h.series_id LEFT JOIN movies m ON m.id = h.movie_id \
         WHERE (?1 IS NULL OR h.event_type = ?1) ORDER BY h.date {order}, h.id {order} LIMIT ?2 OFFSET ?3"
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(event_type)
        .bind(i64::from(paging.page_size()))
        .bind(paging.offset() as i64)
        .fetch_all(db.pool())
        .await?;
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
