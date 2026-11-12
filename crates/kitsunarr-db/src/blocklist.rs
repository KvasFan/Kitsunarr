//! The blocklist (Sonarr's `Blocklist`, `BlocklistRepository`): releases that failed or were
//! rejected by hand, which decisions reject from then on.

use kitsunarr_domain::api::queue::BlocklistItem;
use kitsunarr_domain::{DownloadProtocol, Page, PageRequest, Quality};
use sqlx::{FromRow, SqliteExecutor};

use crate::{Database, DbResult, from_json, to_json};

#[derive(Debug, Clone, PartialEq)]
pub struct NewBlocklist {
    /// The series, or `None` for a film's entry.
    pub series_id: Option<i64>,
    /// The film, for a film's entry.
    pub movie_id: Option<i64>,
    pub episode_ids: Vec<i64>,
    pub source_title: String,
    pub quality_id: Option<i32>,
    pub revision: Option<serde_json::Value>,
    pub languages: Vec<i32>,
    pub date: String,
    pub published_date: Option<String>,
    pub size: Option<i64>,
    pub protocol: DownloadProtocol,
    pub indexer: Option<String>,
    pub indexer_flags: i64,
    pub release_type: Option<String>,
    pub message: Option<String>,
    pub source: Option<String>,
    pub torrent_info_hash: Option<String>,
}

pub async fn insert(ex: impl SqliteExecutor<'_>, b: &NewBlocklist) -> DbResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO blocklist (series_id, movie_id, episode_ids, source_title, quality_id, revision, languages, date, \
         published_date, size, protocol, indexer, indexer_flags, release_type, message, source, torrent_info_hash) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(b.series_id)
    .bind(b.movie_id)
    .bind(to_json(&b.episode_ids)?)
    .bind(&b.source_title)
    .bind(b.quality_id)
    .bind(b.revision.as_ref().map(to_json).transpose()?)
    .bind(to_json(&b.languages)?)
    .bind(&b.date)
    .bind(&b.published_date)
    .bind(b.size)
    .bind(b.protocol.as_str())
    .bind(&b.indexer)
    .bind(b.indexer_flags)
    .bind(&b.release_type)
    .bind(&b.message)
    .bind(&b.source)
    .bind(&b.torrent_info_hash)
    .fetch_one(ex)
    .await?)
}

/// What release matching needs (`ReleaseComparerModel`).
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct BlocklistMatch {
    pub series_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub source_title: String,
    pub protocol: String,
    pub indexer: Option<String>,
    pub published_date: Option<String>,
    pub torrent_info_hash: Option<String>,
}

/// The entries of one series.
pub async fn for_series(db: &Database, series_id: i64) -> DbResult<Vec<BlocklistMatch>> {
    Ok(sqlx::query_as(
        "SELECT series_id, movie_id, source_title, protocol, indexer, published_date, torrent_info_hash \
         FROM blocklist WHERE series_id = ?",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await?)
}

/// The entries of one film.
pub async fn for_movie(db: &Database, movie_id: i64) -> DbResult<Vec<BlocklistMatch>> {
    Ok(sqlx::query_as(
        "SELECT series_id, movie_id, source_title, protocol, indexer, published_date, torrent_info_hash \
         FROM blocklist WHERE movie_id = ?",
    )
    .bind(movie_id)
    .fetch_all(db.pool())
    .await?)
}

/// Every entry (RSS sync looks across the library).
pub async fn all(db: &Database) -> DbResult<Vec<BlocklistMatch>> {
    Ok(sqlx::query_as(
        "SELECT series_id, movie_id, source_title, protocol, indexer, published_date, torrent_info_hash FROM blocklist",
    )
    .fetch_all(db.pool())
    .await?)
}

#[derive(FromRow)]
struct Row {
    id: i64,
    series_id: Option<i64>,
    movie_id: Option<i64>,
    series_title: Option<String>,
    episode_ids: String,
    source_title: String,
    quality_id: Option<i64>,
    date: String,
    protocol: String,
    indexer: Option<String>,
    message: Option<String>,
    source: Option<String>,
}

/// Newest first, unless ascending order is asked for.
pub async fn list(db: &Database, paging: &PageRequest) -> DbResult<Page<BlocklistItem>> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM blocklist")
        .fetch_one(db.pool())
        .await?;
    let order = if paging.sort_direction == Some(kitsunarr_domain::SortDirection::Asc) {
        "ASC"
    } else {
        "DESC"
    };
    let sql = format!(
        "SELECT b.id, b.series_id, b.movie_id, COALESCE(s.title, m.title) AS series_title, b.episode_ids, b.source_title, \
         b.quality_id, b.date, b.protocol, b.indexer, b.message, b.source FROM blocklist b \
         LEFT JOIN series s ON s.id = b.series_id LEFT JOIN movies m ON m.id = b.movie_id \
         ORDER BY b.date {order}, b.id {order} LIMIT ?1 OFFSET ?2"
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(i64::from(paging.page_size()))
        .bind(paging.offset() as i64)
        .fetch_all(db.pool())
        .await?;
    let mut records = Vec::with_capacity(rows.len());
    for r in rows {
        records.push(BlocklistItem {
            id: r.id,
            series_id: r.series_id.unwrap_or(0),
            movie_id: r.movie_id,
            series_title: r.series_title,
            episode_ids: from_json(&r.episode_ids)?,
            source_title: r.source_title,
            quality: r
                .quality_id
                .and_then(|q| Quality::by_id(q as i32))
                .map(|q| q.name.to_string()),
            date: r.date,
            protocol: r.protocol.parse().unwrap_or(DownloadProtocol::Unknown),
            indexer: r.indexer,
            message: r.message,
            source: r.source,
        });
    }
    Ok(Page {
        page: paging.page(),
        page_size: paging.page_size(),
        total_records: total as u64,
        records,
    })
}

/// Deletes the given entries; returns how many were removed.
pub async fn delete(db: &Database, ids: &[i64]) -> DbResult<u64> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut q = sqlx::QueryBuilder::new("DELETE FROM blocklist WHERE id IN (");
    let mut sep = q.separated(", ");
    for id in ids {
        sep.push_bind(*id);
    }
    q.push(")");
    Ok(q.build().execute(db.pool()).await?.rows_affected())
}

/// `ClearBlocklistCommand`.
pub async fn clear(db: &Database) -> DbResult<u64> {
    Ok(sqlx::query("DELETE FROM blocklist")
        .execute(db.pool())
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_list_match_and_delete() {
        let db = crate::test_support::db().await;
        let series_id = crate::library::series::tests::insert_sample(&db, "Show", "1", 2).await;
        let entry = |title: &str, hash: Option<&str>| NewBlocklist {
            movie_id: None,
            series_id: Some(series_id),
            episode_ids: vec![],
            source_title: title.into(),
            quality_id: Some(4),
            revision: None,
            languages: vec![1],
            date: format!("2026-11-12T10:00:0{}Z", title.len() % 10),
            published_date: Some("2026-11-01T00:00:00Z".into()),
            size: Some(100),
            protocol: if hash.is_some() {
                DownloadProtocol::Torrent
            } else {
                DownloadProtocol::Usenet
            },
            indexer: Some("Indexer".into()),
            indexer_flags: 0,
            release_type: Some("singleEpisode".into()),
            message: Some("Failed download detected".into()),
            source: None,
            torrent_info_hash: hash.map(str::to_string),
        };
        let a = insert(db.pool(), &entry("Show.S01E01.720p", Some("abcd")))
            .await
            .unwrap();
        insert(db.pool(), &entry("Show.S01E02.720p.HDTV", None))
            .await
            .unwrap();
        let matches = for_series(&db, series_id).await.unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(all(&db).await.unwrap().len(), 2);
        let page = list(&db, &PageRequest::default()).await.unwrap();
        assert_eq!(page.total_records, 2);
        assert_eq!(page.records[0].series_title.as_deref(), Some("Show"));
        assert_eq!(page.records[0].quality.as_deref(), Some("HDTV-720p"));
        assert_eq!(delete(&db, &[a]).await.unwrap(), 1);
        assert_eq!(clear(&db).await.unwrap(), 1);
        assert!(all(&db).await.unwrap().is_empty());
    }
}
