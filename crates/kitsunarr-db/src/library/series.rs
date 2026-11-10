//! Series (TV and anime), seasons, episodes and episode files.

use kitsunarr_domain::api::library::{
    BulkEdit, CalendarEntry, CollectionRef, EpisodeFileItem, EpisodeItem, LibraryQuery, SeasonItem,
    SeriesDetail, SeriesListItem, SeriesUpdate,
};
use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MediaKind, MetadataProvider, MonitorNewItems, Page,
    Quality, SeriesStatus, SeriesType, SortDirection,
};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqliteConnection};

use super::{int_list, like_pattern, poster_url};
use crate::{Database, DbError, DbResult, from_json, to_json};

/// Full description of a series used for inserts and full updates.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSeries {
    pub media_kind: MediaKind,
    pub title: String,
    pub year: Option<i32>,
    pub status: SeriesStatus,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub air_time: Option<String>,
    pub runtime: Option<i32>,
    pub certification: Option<String>,
    pub original_language: Option<i32>,
    pub genres: Vec<String>,
    pub images: Vec<MediaImage>,
    pub ratings: Option<serde_json::Value>,
    pub first_aired: Option<String>,
    pub last_aired: Option<String>,
    pub series_type: SeriesType,
    pub metadata_provider: MetadataProvider,
    pub metadata_id: String,
    pub episode_ordering: Option<String>,
    pub monitored: bool,
    pub monitor_new_items: MonitorNewItems,
    pub season_folder: bool,
    pub use_scene_numbering: bool,
    pub quality_profile_id: i64,
    pub root_folder_path: String,
    pub path: String,
    pub added_at: String,
    pub last_refreshed_at: Option<String>,
    pub tags: Vec<i64>,
    pub external_ids: Vec<ExternalId>,
    /// `(title, season_number, source)`.
    pub alternate_titles: Vec<(String, Option<i32>, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSeason {
    pub season_number: i32,
    pub monitored: bool,
    pub title: Option<String>,
    pub images: Vec<MediaImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewEpisodeFile {
    pub series_id: i64,
    pub season_number: i32,
    pub relative_path: String,
    pub size: i64,
    pub added_at: String,
    pub scene_name: Option<String>,
    pub release_group: Option<String>,
    pub quality_id: i32,
    pub revision_version: i32,
    pub revision_real: i32,
    pub is_repack: bool,
    pub languages: Vec<i32>,
    pub media_info: Option<serde_json::Value>,
    pub original_path: Option<String>,
    pub indexer_flags: i64,
    pub release_type: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewEpisode {
    pub series_id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub absolute_episode_number: Option<i32>,
    pub scene_season_number: Option<i32>,
    pub scene_episode_number: Option<i32>,
    pub scene_absolute_episode_number: Option<i32>,
    pub unverified_scene_numbering: bool,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date: Option<String>,
    pub air_date_utc: Option<String>,
    pub runtime: Option<i32>,
    pub monitored: bool,
    pub episode_file_id: Option<i64>,
    pub provider_episode_id: Option<String>,
    pub last_search_at: Option<String>,
}

fn validate(s: &NewSeries) -> DbResult<()> {
    if s.media_kind == MediaKind::Movie {
        return Err(DbError::Invalid(
            "movies are stored in the movie library".into(),
        ));
    }
    if s.title.trim().is_empty() || s.path.trim().is_empty() || s.metadata_id.trim().is_empty() {
        return Err(DbError::Invalid(
            "title, path and metadata id are required".into(),
        ));
    }
    Ok(())
}

pub async fn insert(conn: &mut SqliteConnection, s: &NewSeries) -> DbResult<i64> {
    validate(s)?;
    let title = s.title.trim();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO series (media_kind, title, sort_title, clean_title, title_slug, year, status, overview, network, \
         air_time, runtime, certification, original_language, genres, images, ratings, first_aired, last_aired, series_type, \
         metadata_provider, metadata_id, episode_ordering, monitored, monitor_new_items, season_folder, use_scene_numbering, \
         quality_profile_id, root_folder_path, path, added_at, last_refreshed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(s.media_kind.as_str())
    .bind(title)
    .bind(kitsunarr_domain::text::sort_title(title))
    .bind(kitsunarr_domain::text::clean_title(title))
    .bind(kitsunarr_domain::text::slugify(title))
    .bind(s.year)
    .bind(s.status.as_str())
    .bind(&s.overview)
    .bind(&s.network)
    .bind(&s.air_time)
    .bind(s.runtime)
    .bind(&s.certification)
    .bind(s.original_language)
    .bind(to_json(&s.genres)?)
    .bind(to_json(&s.images)?)
    .bind(s.ratings.as_ref().map(to_json).transpose()?)
    .bind(&s.first_aired)
    .bind(&s.last_aired)
    .bind(s.series_type.as_str())
    .bind(s.metadata_provider.as_str())
    .bind(s.metadata_id.trim())
    .bind(&s.episode_ordering)
    .bind(s.monitored)
    .bind(s.monitor_new_items.as_str())
    .bind(s.season_folder)
    .bind(s.use_scene_numbering)
    .bind(s.quality_profile_id)
    .bind(&s.root_folder_path)
    .bind(&s.path)
    .bind(&s.added_at)
    .bind(&s.last_refreshed_at)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "series"))?;
    replace_links(conn, id, s).await?;
    Ok(id)
}

/// Overwrites every mapped field of an existing series (used by migration "update" policy).
pub async fn update_full(conn: &mut SqliteConnection, id: i64, s: &NewSeries) -> DbResult<()> {
    validate(s)?;
    let title = s.title.trim();
    let r = sqlx::query(
        "UPDATE series SET media_kind = ?, title = ?, sort_title = ?, clean_title = ?, title_slug = ?, year = ?, status = ?, \
         overview = ?, network = ?, air_time = ?, runtime = ?, certification = ?, original_language = ?, genres = ?, images = ?, \
         ratings = ?, first_aired = ?, last_aired = ?, series_type = ?, metadata_provider = ?, metadata_id = ?, episode_ordering = ?, \
         monitored = ?, monitor_new_items = ?, season_folder = ?, use_scene_numbering = ?, quality_profile_id = ?, \
         root_folder_path = ?, path = ? WHERE id = ?",
    )
    .bind(s.media_kind.as_str())
    .bind(title)
    .bind(kitsunarr_domain::text::sort_title(title))
    .bind(kitsunarr_domain::text::clean_title(title))
    .bind(kitsunarr_domain::text::slugify(title))
    .bind(s.year)
    .bind(s.status.as_str())
    .bind(&s.overview)
    .bind(&s.network)
    .bind(&s.air_time)
    .bind(s.runtime)
    .bind(&s.certification)
    .bind(s.original_language)
    .bind(to_json(&s.genres)?)
    .bind(to_json(&s.images)?)
    .bind(s.ratings.as_ref().map(to_json).transpose()?)
    .bind(&s.first_aired)
    .bind(&s.last_aired)
    .bind(s.series_type.as_str())
    .bind(s.metadata_provider.as_str())
    .bind(s.metadata_id.trim())
    .bind(&s.episode_ordering)
    .bind(s.monitored)
    .bind(s.monitor_new_items.as_str())
    .bind(s.season_folder)
    .bind(s.use_scene_numbering)
    .bind(s.quality_profile_id)
    .bind(&s.root_folder_path)
    .bind(&s.path)
    .bind(id)
    .execute(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "series"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    replace_links(conn, id, s).await
}

async fn replace_links(conn: &mut SqliteConnection, id: i64, s: &NewSeries) -> DbResult<()> {
    sqlx::query("DELETE FROM series_tags WHERE series_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for tag in &s.tags {
        sqlx::query("INSERT OR IGNORE INTO series_tags (series_id, tag_id) VALUES (?, ?)")
            .bind(id)
            .bind(tag)
            .execute(&mut *conn)
            .await
            .map_err(|e| DbError::from_write(e, "series tag"))?;
    }
    sqlx::query("DELETE FROM series_external_ids WHERE series_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for ext in &s.external_ids {
        sqlx::query("INSERT OR IGNORE INTO series_external_ids (series_id, kind, external_id) VALUES (?, ?, ?)")
            .bind(id)
            .bind(ext.kind.as_str())
            .bind(&ext.value)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("DELETE FROM series_alternate_titles WHERE series_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for (title, season, source) in &s.alternate_titles {
        if title.trim().is_empty() {
            continue;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO series_alternate_titles (series_id, title, clean_title, season_number, source) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(title.trim())
        .bind(kitsunarr_domain::text::clean_title(title))
        .bind(season)
        .bind(source)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn upsert_season(
    conn: &mut SqliteConnection,
    series_id: i64,
    s: &NewSeason,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO seasons (series_id, season_number, monitored, title, images) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(series_id, season_number) DO UPDATE SET monitored = excluded.monitored, title = excluded.title, images = excluded.images",
    )
    .bind(series_id)
    .bind(s.season_number)
    .bind(s.monitored)
    .bind(&s.title)
    .bind(to_json(&s.images)?)
    .execute(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "season"))?;
    Ok(())
}

pub async fn upsert_episode_file(conn: &mut SqliteConnection, f: &NewEpisodeFile) -> DbResult<i64> {
    sqlx::query_scalar(
        "INSERT INTO episode_files (series_id, season_number, relative_path, size, added_at, scene_name, release_group, quality_id, \
         revision_version, revision_real, is_repack, languages, media_info, original_path, indexer_flags, release_type) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(series_id, relative_path) DO UPDATE SET season_number = excluded.season_number, size = excluded.size, \
         scene_name = excluded.scene_name, release_group = excluded.release_group, quality_id = excluded.quality_id, \
         revision_version = excluded.revision_version, revision_real = excluded.revision_real, is_repack = excluded.is_repack, \
         languages = excluded.languages, media_info = excluded.media_info, original_path = excluded.original_path, \
         indexer_flags = excluded.indexer_flags, release_type = excluded.release_type \
         RETURNING id",
    )
    .bind(f.series_id)
    .bind(f.season_number)
    .bind(&f.relative_path)
    .bind(f.size)
    .bind(&f.added_at)
    .bind(&f.scene_name)
    .bind(&f.release_group)
    .bind(f.quality_id)
    .bind(f.revision_version)
    .bind(f.revision_real)
    .bind(f.is_repack)
    .bind(to_json(&f.languages)?)
    .bind(f.media_info.as_ref().map(to_json).transpose()?)
    .bind(&f.original_path)
    .bind(f.indexer_flags)
    .bind(f.release_type)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "episode file"))
}

/// Inserts or updates an episode keyed by `(series, season, episode)`; returns its id.
pub async fn upsert_episode(conn: &mut SqliteConnection, e: &NewEpisode) -> DbResult<i64> {
    sqlx::query_scalar(
        "INSERT INTO episodes (series_id, season_number, episode_number, absolute_episode_number, scene_season_number, \
         scene_episode_number, scene_absolute_episode_number, unverified_scene_numbering, title, overview, air_date, air_date_utc, \
         runtime, monitored, episode_file_id, provider_episode_id, last_search_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(series_id, season_number, episode_number) DO UPDATE SET \
         absolute_episode_number = excluded.absolute_episode_number, scene_season_number = excluded.scene_season_number, \
         scene_episode_number = excluded.scene_episode_number, scene_absolute_episode_number = excluded.scene_absolute_episode_number, \
         unverified_scene_numbering = excluded.unverified_scene_numbering, title = excluded.title, overview = excluded.overview, \
         air_date = excluded.air_date, air_date_utc = excluded.air_date_utc, runtime = excluded.runtime, monitored = excluded.monitored, \
         episode_file_id = excluded.episode_file_id, provider_episode_id = excluded.provider_episode_id, \
         last_search_at = COALESCE(excluded.last_search_at, episodes.last_search_at) \
         RETURNING id",
    )
    .bind(e.series_id)
    .bind(e.season_number)
    .bind(e.episode_number)
    .bind(e.absolute_episode_number)
    .bind(e.scene_season_number)
    .bind(e.scene_episode_number)
    .bind(e.scene_absolute_episode_number)
    .bind(e.unverified_scene_numbering)
    .bind(&e.title)
    .bind(&e.overview)
    .bind(&e.air_date)
    .bind(&e.air_date_utc)
    .bind(e.runtime)
    .bind(e.monitored)
    .bind(e.episode_file_id)
    .bind(&e.provider_episode_id)
    .bind(&e.last_search_at)
    .fetch_one(&mut *conn)
    .await
    .map_err(|err| DbError::from_write(err, "episode"))
}

pub async fn find_by_metadata(
    conn: &mut SqliteConnection,
    provider: MetadataProvider,
    id: &str,
) -> DbResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM series WHERE metadata_provider = ? AND metadata_id = ?")
            .bind(provider.as_str())
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

pub async fn find_by_path(conn: &mut SqliteConnection, path: &str) -> DbResult<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM series WHERE path = ?")
        .bind(path)
        .fetch_optional(&mut *conn)
        .await?)
}

pub async fn find_by_external_id(
    conn: &mut SqliteConnection,
    kind: ExternalIdKind,
    value: &str,
) -> DbResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT series_id FROM series_external_ids WHERE kind = ? AND external_id = ? LIMIT 1",
    )
    .bind(kind.as_str())
    .bind(value)
    .fetch_optional(&mut *conn)
    .await?)
}

// ---------------------------------------------------------------------------------------------
// Read models
// ---------------------------------------------------------------------------------------------

#[derive(FromRow)]
struct ListRow {
    id: i64,
    media_kind: String,
    title: String,
    sort_title: String,
    year: Option<i64>,
    status: String,
    series_type: String,
    network: Option<String>,
    images: String,
    monitored: bool,
    quality_profile_id: i64,
    quality_profile_name: String,
    metadata_provider: String,
    path: String,
    added_at: String,
    season_count: i64,
    episode_count: i64,
    episode_file_count: i64,
    missing_count: i64,
    size_on_disk: i64,
    next_airing: Option<String>,
    tags: Option<String>,
}

impl ListRow {
    fn into_item(self) -> SeriesListItem {
        SeriesListItem {
            id: self.id,
            kind: self.media_kind.parse().unwrap_or(MediaKind::Series),
            title: self.title,
            sort_title: self.sort_title,
            year: self.year.map(|y| y as i32),
            status: self.status.parse().unwrap_or(SeriesStatus::Unknown),
            series_type: self.series_type.parse().unwrap_or(SeriesType::Standard),
            network: self.network,
            poster_url: poster_url(&self.images),
            monitored: self.monitored,
            quality_profile_id: self.quality_profile_id,
            quality_profile_name: self.quality_profile_name,
            metadata_provider: self
                .metadata_provider
                .parse()
                .unwrap_or(MetadataProvider::Tvdb),
            path: self.path,
            season_count: self.season_count,
            episode_count: self.episode_count,
            episode_file_count: self.episode_file_count,
            monitored_missing_count: self.missing_count,
            size_on_disk: self.size_on_disk,
            next_airing: self.next_airing,
            added_at: self.added_at,
            tags: int_list(self.tags.as_deref()),
        }
    }
}

const LIST_COLUMNS: &str = "s.id, s.media_kind, s.title, s.sort_title, s.year, s.status, s.series_type, s.network, s.images, \
    s.monitored, s.quality_profile_id, qp.name AS quality_profile_name, s.metadata_provider, s.path, s.added_at, \
    (SELECT COUNT(*) FROM seasons x WHERE x.series_id = s.id AND x.season_number > 0) AS season_count, \
    (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id AND e.season_number > 0) AS episode_count, \
    (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id AND e.episode_file_id IS NOT NULL) AS episode_file_count, \
    (SELECT COUNT(*) FROM episodes e WHERE e.series_id = s.id AND e.monitored = 1 AND e.episode_file_id IS NULL \
       AND e.season_number > 0 AND e.air_date_utc IS NOT NULL AND e.air_date_utc <= strftime('%Y-%m-%dT%H:%M:%SZ','now')) AS missing_count, \
    (SELECT COALESCE(SUM(f.size), 0) FROM episode_files f WHERE f.series_id = s.id) AS size_on_disk, \
    (SELECT MIN(e.air_date_utc) FROM episodes e WHERE e.series_id = s.id AND e.air_date_utc > strftime('%Y-%m-%dT%H:%M:%SZ','now')) AS next_airing, \
    (SELECT json_group_array(t.tag_id) FROM series_tags t WHERE t.series_id = s.id) AS tags";

fn sort_expression(key: Option<&str>) -> &'static str {
    match key.unwrap_or("title") {
        "added" | "addedAt" => "s.added_at",
        "year" => "s.year",
        "status" => "s.status",
        "network" => "s.network",
        "size" | "sizeOnDisk" => {
            "(SELECT COALESCE(SUM(f.size), 0) FROM episode_files f WHERE f.series_id = s.id)"
        }
        "nextAiring" => {
            "(SELECT MIN(e.air_date_utc) FROM episodes e WHERE e.series_id = s.id AND e.air_date_utc > strftime('%Y-%m-%dT%H:%M:%SZ','now'))"
        }
        "path" => "s.path",
        _ => "s.sort_title",
    }
}

fn push_filters(qb: &mut QueryBuilder<Sqlite>, q: &LibraryQuery) {
    qb.push(" WHERE 1 = 1");
    if let Some(kind) = q.kind {
        qb.push(" AND s.media_kind = ").push_bind(kind.as_str());
    }
    if let Some(m) = q.monitored {
        qb.push(" AND s.monitored = ").push_bind(m);
    }
    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND s.status = ").push_bind(status.to_string());
    }
    if let Some(tag) = q.tag_id {
        qb.push(
            " AND EXISTS (SELECT 1 FROM series_tags t WHERE t.series_id = s.id AND t.tag_id = ",
        )
        .push_bind(tag)
        .push(")");
    }
    if let Some(p) = q.quality_profile_id {
        qb.push(" AND s.quality_profile_id = ").push_bind(p);
    }
    if let Some(p) = q.provider {
        qb.push(" AND s.metadata_provider = ").push_bind(p.as_str());
    }
    if let Some(term) = q.search.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let like = like_pattern(term);
        let clean = like_pattern(&kitsunarr_domain::text::clean_title(term));
        qb.push(" AND (s.title LIKE ")
            .push_bind(like.clone())
            .push(" ESCAPE '\\'");
        qb.push(" OR s.clean_title LIKE ")
            .push_bind(clean.clone())
            .push(" ESCAPE '\\'");
        qb.push(" OR EXISTS (SELECT 1 FROM series_alternate_titles a WHERE a.series_id = s.id AND a.clean_title LIKE ")
            .push_bind(clean)
            .push(" ESCAPE '\\'))");
    }
}

pub async fn list(db: &Database, q: &LibraryQuery) -> DbResult<Page<SeriesListItem>> {
    let paging = q.page_request();
    let mut count_qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM series s");
    push_filters(&mut count_qb, q);
    let total: i64 = count_qb.build_query_scalar().fetch_one(db.pool()).await?;

    let dir = if paging.direction() == SortDirection::Desc {
        "DESC"
    } else {
        "ASC"
    };
    let sort = sort_expression(paging.sort_key.as_deref());
    // Aggregates are computed only for the rows of the requested page.
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT ");
    qb.push(LIST_COLUMNS);
    qb.push(" FROM series s JOIN quality_profiles qp ON qp.id = s.quality_profile_id");
    push_filters(&mut qb, q);
    qb.push(format_args!(" ORDER BY {sort} {dir}, s.id {dir} LIMIT "));
    qb.push_bind(i64::from(paging.page_size()));
    qb.push(" OFFSET ").push_bind(paging.offset() as i64);
    let rows: Vec<ListRow> = qb.build_query_as().fetch_all(db.pool()).await?;
    Ok(Page {
        page: paging.page(),
        page_size: paging.page_size(),
        total_records: total as u64,
        records: rows.into_iter().map(ListRow::into_item).collect(),
    })
}

#[derive(FromRow)]
struct DetailRow {
    overview: Option<String>,
    runtime: Option<i64>,
    certification: Option<String>,
    genres: String,
    images: String,
    metadata_id: String,
    episode_ordering: Option<String>,
    monitor_new_items: String,
    season_folder: bool,
    use_scene_numbering: bool,
    root_folder_path: String,
    first_aired: Option<String>,
    last_aired: Option<String>,
    last_refreshed_at: Option<String>,
}

pub async fn get_detail(db: &Database, id: i64) -> DbResult<SeriesDetail> {
    let item = get_list_item(db, id).await?;
    let d: DetailRow = sqlx::query_as(
        "SELECT overview, runtime, certification, genres, images, metadata_id, episode_ordering, monitor_new_items, \
         season_folder, use_scene_numbering, root_folder_path, first_aired, last_aired, last_refreshed_at FROM series WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await?
    .ok_or(DbError::NotFound)?;
    let external: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, external_id FROM series_external_ids WHERE series_id = ? ORDER BY kind, external_id")
            .bind(id)
            .fetch_all(db.pool())
            .await?;
    let alternate_titles: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM series_alternate_titles WHERE series_id = ? ORDER BY title",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    let seasons: Vec<(i64, bool, Option<String>, i64, i64, i64)> = sqlx::query_as(
        "SELECT x.season_number, x.monitored, x.title, \
           (SELECT COUNT(*) FROM episodes e WHERE e.series_id = x.series_id AND e.season_number = x.season_number), \
           (SELECT COUNT(*) FROM episodes e WHERE e.series_id = x.series_id AND e.season_number = x.season_number AND e.episode_file_id IS NOT NULL), \
           (SELECT COALESCE(SUM(f.size), 0) FROM episode_files f WHERE f.series_id = x.series_id AND f.season_number = x.season_number) \
         FROM seasons x WHERE x.series_id = ? ORDER BY x.season_number",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    let collections: Vec<(i64, String)> = sqlx::query_as(
        "SELECT c.id, c.title FROM collections c JOIN collection_members m ON m.collection_id = c.id WHERE m.series_id = ? ORDER BY c.title",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    Ok(SeriesDetail {
        item,
        overview: d.overview,
        runtime: d.runtime.map(|r| r as i32),
        certification: d.certification,
        genres: from_json(&d.genres)?,
        images: from_json(&d.images)?,
        metadata_id: d.metadata_id,
        external_ids: external
            .into_iter()
            .filter_map(|(k, v)| k.parse().ok().map(|kind| ExternalId { kind, value: v }))
            .collect(),
        alternate_titles,
        episode_ordering: d.episode_ordering,
        monitor_new_items: d.monitor_new_items.parse().unwrap_or(MonitorNewItems::All),
        season_folder: d.season_folder,
        use_scene_numbering: d.use_scene_numbering,
        root_folder_path: d.root_folder_path,
        first_aired: d.first_aired,
        last_aired: d.last_aired,
        last_refreshed_at: d.last_refreshed_at,
        seasons: seasons
            .into_iter()
            .map(|(n, monitored, title, episodes, files, size)| SeasonItem {
                season_number: n as i32,
                monitored,
                title,
                episode_count: episodes,
                episode_file_count: files,
                size_on_disk: size,
            })
            .collect(),
        collections: collections
            .into_iter()
            .map(|(id, title)| CollectionRef { id, title })
            .collect(),
    })
}

pub async fn get_list_item(db: &Database, id: i64) -> DbResult<SeriesListItem> {
    let sql = format!(
        "SELECT {LIST_COLUMNS} FROM series s JOIN quality_profiles qp ON qp.id = s.quality_profile_id WHERE s.id = ?"
    );
    let row: Option<ListRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.ok_or(DbError::NotFound)?.into_item())
}

#[derive(FromRow)]
struct EpisodeRow {
    id: i64,
    series_id: i64,
    season_number: i64,
    episode_number: i64,
    absolute_episode_number: Option<i64>,
    scene_season_number: Option<i64>,
    scene_episode_number: Option<i64>,
    title: Option<String>,
    air_date: Option<String>,
    air_date_utc: Option<String>,
    runtime: Option<i64>,
    monitored: bool,
    file_id: Option<i64>,
    file_relative_path: Option<String>,
    file_size: Option<i64>,
    file_quality_id: Option<i64>,
    file_languages: Option<String>,
    file_release_group: Option<String>,
    file_added_at: Option<String>,
}

pub async fn list_episodes(
    db: &Database,
    series_id: i64,
    season: Option<i32>,
) -> DbResult<Vec<EpisodeItem>> {
    let rows: Vec<EpisodeRow> = sqlx::query_as(
        "SELECT e.id, e.series_id, e.season_number, e.episode_number, e.absolute_episode_number, e.scene_season_number, \
         e.scene_episode_number, e.title, e.air_date, e.air_date_utc, e.runtime, e.monitored, \
         f.id AS file_id, f.relative_path AS file_relative_path, f.size AS file_size, f.quality_id AS file_quality_id, \
         f.languages AS file_languages, f.release_group AS file_release_group, f.added_at AS file_added_at \
         FROM episodes e LEFT JOIN episode_files f ON f.id = e.episode_file_id \
         WHERE e.series_id = ? AND (? IS NULL OR e.season_number = ?) \
         ORDER BY e.season_number, e.episode_number",
    )
    .bind(series_id)
    .bind(season)
    .bind(season)
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let file = r.file_id.map(|fid| {
                let qid = r.file_quality_id.unwrap_or(0) as i32;
                EpisodeFileItem {
                    id: fid,
                    relative_path: r.file_relative_path.clone().unwrap_or_default(),
                    size: r.file_size.unwrap_or(0),
                    quality_id: qid,
                    quality_name: Quality::by_id(qid)
                        .unwrap_or_else(Quality::unknown)
                        .name
                        .to_string(),
                    languages: r
                        .file_languages
                        .as_deref()
                        .and_then(|l| serde_json::from_str(l).ok())
                        .unwrap_or_default(),
                    release_group: r.file_release_group.clone(),
                    added_at: r.file_added_at.clone().unwrap_or_default(),
                }
            });
            EpisodeItem {
                id: r.id,
                series_id: r.series_id,
                season_number: r.season_number as i32,
                episode_number: r.episode_number as i32,
                absolute_episode_number: r.absolute_episode_number.map(|v| v as i32),
                scene_season_number: r.scene_season_number.map(|v| v as i32),
                scene_episode_number: r.scene_episode_number.map(|v| v as i32),
                title: r.title,
                air_date: r.air_date,
                air_date_utc: r.air_date_utc,
                runtime: r.runtime.map(|v| v as i32),
                monitored: r.monitored,
                file,
            }
        })
        .collect())
}

/// Applies a partial update. Returns `NotFound` for unknown ids.
pub async fn apply_update(db: &Database, id: i64, u: &SeriesUpdate) -> DbResult<()> {
    let mut tx = db.pool().begin().await?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM series WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(DbError::NotFound);
    }
    if let Some(m) = u.monitored {
        sqlx::query("UPDATE series SET monitored = ? WHERE id = ?")
            .bind(m)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(p) = u.quality_profile_id {
        sqlx::query("UPDATE series SET quality_profile_id = ? WHERE id = ?")
            .bind(p)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DbError::from_write(e, "quality profile"))?;
    }
    if let Some(t) = u.series_type {
        sqlx::query("UPDATE series SET series_type = ? WHERE id = ?")
            .bind(t.as_str())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(f) = u.season_folder {
        sqlx::query("UPDATE series SET season_folder = ? WHERE id = ?")
            .bind(f)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(m) = u.monitor_new_items {
        sqlx::query("UPDATE series SET monitor_new_items = ? WHERE id = ?")
            .bind(m.as_str())
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(tags) = &u.tags {
        sqlx::query("DELETE FROM series_tags WHERE series_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for tag in tags {
            sqlx::query("INSERT OR IGNORE INTO series_tags (series_id, tag_id) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(&mut *tx)
                .await
                .map_err(|e| DbError::from_write(e, "tag"))?;
        }
    }
    if let Some(seasons) = &u.seasons {
        for s in seasons {
            sqlx::query(
                "UPDATE seasons SET monitored = ? WHERE series_id = ? AND season_number = ?",
            )
            .bind(s.monitored)
            .bind(id)
            .bind(s.season_number)
            .execute(&mut *tx)
            .await?;
            // Season monitoring cascades to its episodes, as in Sonarr.
            sqlx::query(
                "UPDATE episodes SET monitored = ? WHERE series_id = ? AND season_number = ?",
            )
            .bind(s.monitored)
            .bind(id)
            .bind(s.season_number)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub async fn bulk_edit(db: &Database, edit: &BulkEdit) -> DbResult<u64> {
    let mut tx = db.pool().begin().await?;
    let mut updated = 0u64;
    for id in &edit.ids {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM series WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            continue;
        }
        if let Some(m) = edit.monitored {
            sqlx::query("UPDATE series SET monitored = ? WHERE id = ?")
                .bind(m)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(p) = edit.quality_profile_id {
            sqlx::query("UPDATE series SET quality_profile_id = ? WHERE id = ?")
                .bind(p)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|e| DbError::from_write(e, "quality profile"))?;
        }
        for tag in &edit.add_tags {
            sqlx::query("INSERT OR IGNORE INTO series_tags (series_id, tag_id) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(&mut *tx)
                .await
                .map_err(|e| DbError::from_write(e, "tag"))?;
        }
        for tag in &edit.remove_tags {
            sqlx::query("DELETE FROM series_tags WHERE series_id = ? AND tag_id = ?")
                .bind(id)
                .bind(tag)
                .execute(&mut *tx)
                .await?;
        }
        updated += 1;
    }
    tx.commit().await?;
    Ok(updated)
}

pub async fn set_episodes_monitored(db: &Database, ids: &[i64], monitored: bool) -> DbResult<u64> {
    let mut tx = db.pool().begin().await?;
    let mut n = 0;
    for chunk in ids.chunks(500) {
        let mut qb = QueryBuilder::<Sqlite>::new("UPDATE episodes SET monitored = ");
        qb.push_bind(monitored).push(" WHERE id IN (");
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        n += qb.build().execute(&mut *tx).await?.rows_affected();
    }
    tx.commit().await?;
    Ok(n)
}

/// Removes the series and its records. Files on disk are never touched here.
pub async fn delete(db: &Database, id: i64) -> DbResult<()> {
    let r = sqlx::query("DELETE FROM series WHERE id = ?")
        .bind(id)
        .execute(db.pool())
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

#[derive(FromRow)]
struct CalendarRow {
    episode_id: i64,
    series_id: i64,
    series_title: String,
    media_kind: String,
    season_number: i64,
    episode_number: i64,
    absolute_episode_number: Option<i64>,
    title: Option<String>,
    air_date_utc: String,
    monitored: bool,
    has_file: bool,
}

pub async fn calendar(
    db: &Database,
    start: &str,
    end: &str,
    include_unmonitored: bool,
) -> DbResult<Vec<CalendarEntry>> {
    let rows: Vec<CalendarRow> = sqlx::query_as(
        "SELECT e.id AS episode_id, e.series_id, s.title AS series_title, s.media_kind, e.season_number, e.episode_number, \
         e.absolute_episode_number, e.title, e.air_date_utc, (e.monitored AND s.monitored) AS monitored, \
         e.episode_file_id IS NOT NULL AS has_file \
         FROM episodes e JOIN series s ON s.id = e.series_id \
         WHERE e.air_date_utc >= ? AND e.air_date_utc < ? AND (? OR (e.monitored = 1 AND s.monitored = 1)) \
         ORDER BY e.air_date_utc, s.sort_title, e.season_number, e.episode_number LIMIT 5000",
    )
    .bind(start)
    .bind(end)
    .bind(include_unmonitored)
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CalendarEntry {
            episode_id: r.episode_id,
            series_id: r.series_id,
            series_title: r.series_title,
            kind: r.media_kind.parse().unwrap_or(MediaKind::Series),
            season_number: r.season_number as i32,
            episode_number: r.episode_number as i32,
            absolute_episode_number: r.absolute_episode_number.map(|v| v as i32),
            title: r.title,
            air_date_utc: r.air_date_utc,
            monitored: r.monitored,
            has_file: r.has_file,
        })
        .collect())
}

/// An episode file with every column release decisions read (custom formats, upgrades).
#[derive(Debug, Clone, FromRow)]
pub struct FileRecord {
    pub id: i64,
    pub relative_path: String,
    pub size: i64,
    pub scene_name: Option<String>,
    pub original_path: Option<String>,
    pub release_group: Option<String>,
    pub quality_id: i64,
    pub revision_version: i64,
    pub revision_real: i64,
    pub is_repack: bool,
    /// Language ids.
    pub languages: String,
    pub indexer_flags: i64,
    pub release_type: i64,
}

impl FileRecord {
    pub fn language_ids(&self) -> Vec<i32> {
        serde_json::from_str(&self.languages).unwrap_or_default()
    }
}

/// The files of a series.
pub async fn file_records(db: &Database, series_id: i64) -> DbResult<Vec<FileRecord>> {
    Ok(sqlx::query_as(
        "SELECT id, relative_path, size, scene_name, original_path, release_group, quality_id, \
           revision_version, revision_real, is_repack, languages, indexer_flags, release_type \
         FROM episode_files WHERE series_id = ? ORDER BY id",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await?)
}

/// The series' original language id, when known.
pub async fn original_language(db: &Database, series_id: i64) -> DbResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT original_language FROM series WHERE id = ?")
            .bind(series_id)
            .fetch_optional(db.pool())
            .await?
            .flatten(),
    )
}

/// How a library series can be recognised in a release name.
#[derive(Debug, Clone)]
pub struct TitleRecord {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub alternate_titles: Vec<String>,
    /// `(kind, value)` pairs, e.g. `("tvdb", "81189")`.
    pub external_ids: Vec<(String, String)>,
}

/// Titles, years, alternate titles and identifiers of every series.
pub async fn title_records(db: &Database) -> DbResult<Vec<TitleRecord>> {
    let rows: Vec<(i64, String, Option<i64>)> =
        sqlx::query_as("SELECT id, title, year FROM series ORDER BY id")
            .fetch_all(db.pool())
            .await?;
    let alternates: Vec<(i64, String)> = sqlx::query_as(
        "SELECT series_id, title FROM series_alternate_titles ORDER BY series_id, title",
    )
    .fetch_all(db.pool())
    .await?;
    let ids: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT series_id, kind, external_id FROM series_external_ids ORDER BY series_id, kind",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, title, year)| TitleRecord {
            id,
            title,
            year,
            alternate_titles: alternates
                .iter()
                .filter(|(s, _)| *s == id)
                .map(|(_, t)| t.clone())
                .collect(),
            external_ids: ids
                .iter()
                .filter(|(s, _, _)| *s == id)
                .map(|(_, k, v)| (k.clone(), v.clone()))
                .collect(),
        })
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use kitsunarr_domain::api::library::SeasonMonitoring;

    pub fn sample(title: &str, metadata_id: &str) -> NewSeries {
        NewSeries {
            media_kind: MediaKind::Series,
            title: title.into(),
            year: Some(2008),
            status: SeriesStatus::Ended,
            overview: None,
            network: Some("AMC".into()),
            air_time: None,
            runtime: Some(47),
            certification: None,
            original_language: Some(1),
            genres: vec!["Drama".into()],
            images: vec![MediaImage {
                cover_type: "poster".into(),
                remote_url: "https://img/p.jpg".into(),
            }],
            ratings: None,
            first_aired: Some("2008-01-20".into()),
            last_aired: None,
            series_type: SeriesType::Standard,
            metadata_provider: MetadataProvider::Tvdb,
            metadata_id: metadata_id.into(),
            episode_ordering: None,
            monitored: true,
            monitor_new_items: MonitorNewItems::All,
            season_folder: true,
            use_scene_numbering: false,
            quality_profile_id: 1,
            root_folder_path: "/media/tv".into(),
            path: format!("/media/tv/{title}"),
            added_at: "2026-01-01T00:00:00Z".into(),
            last_refreshed_at: None,
            tags: vec![],
            external_ids: vec![ExternalId {
                kind: ExternalIdKind::Tvdb,
                value: metadata_id.into(),
            }],
            alternate_titles: vec![("Breaking Bad (US)".into(), None, Some("test".into()))],
        }
    }

    pub async fn insert_sample(
        db: &Database,
        title: &str,
        metadata_id: &str,
        episodes: i32,
    ) -> i64 {
        let mut conn = db.pool().acquire().await.unwrap();
        let id = insert(&mut conn, &sample(title, metadata_id))
            .await
            .unwrap();
        upsert_season(
            &mut conn,
            id,
            &NewSeason {
                season_number: 1,
                monitored: true,
                title: None,
                images: vec![],
            },
        )
        .await
        .unwrap();
        for n in 1..=episodes {
            let file_id = if n == 1 {
                Some(
                    upsert_episode_file(
                        &mut conn,
                        &NewEpisodeFile {
                            series_id: id,
                            season_number: 1,
                            relative_path: format!("Season 01/S01E{n:02}.mkv"),
                            size: 1_000,
                            added_at: "2026-01-02T00:00:00Z".into(),
                            scene_name: None,
                            release_group: Some("GRP".into()),
                            quality_id: 7,
                            revision_version: 1,
                            revision_real: 0,
                            is_repack: false,
                            languages: vec![1],
                            media_info: None,
                            original_path: None,
                            indexer_flags: 0,
                            release_type: 0,
                        },
                    )
                    .await
                    .unwrap(),
                )
            } else {
                None
            };
            upsert_episode(
                &mut conn,
                &NewEpisode {
                    series_id: id,
                    season_number: 1,
                    episode_number: n,
                    absolute_episode_number: Some(n),
                    scene_season_number: None,
                    scene_episode_number: None,
                    scene_absolute_episode_number: None,
                    unverified_scene_numbering: false,
                    title: Some(format!("Episode {n}")),
                    overview: None,
                    air_date: Some("2008-01-20".into()),
                    air_date_utc: Some(format!("2008-01-{:02}T02:00:00Z", 20 + n)),
                    runtime: Some(47),
                    monitored: true,
                    episode_file_id: file_id,
                    provider_episode_id: None,
                    last_search_at: None,
                },
            )
            .await
            .unwrap();
        }
        id
    }

    #[tokio::test]
    async fn insert_list_detail_update_delete() {
        let db = crate::test_support::db().await;
        let a = insert_sample(&db, "Breaking Bad", "81189", 3).await;
        let _b = insert_sample(&db, "The Office", "73244", 0).await;

        let page = list(&db, &LibraryQuery::default()).await.unwrap();
        assert_eq!(page.total_records, 2);
        assert_eq!(
            page.records[0].title, "Breaking Bad",
            "sorted by sort title (articles ignored)"
        );
        let bb = &page.records[0];
        assert_eq!(
            (
                bb.episode_count,
                bb.episode_file_count,
                bb.monitored_missing_count
            ),
            (3, 1, 2)
        );
        assert_eq!(bb.size_on_disk, 1_000);
        assert_eq!(bb.poster_url.as_deref(), Some("https://img/p.jpg"));

        let search = list(
            &db,
            &LibraryQuery {
                search: Some("office".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(search.total_records, 1);
        let alt = list(
            &db,
            &LibraryQuery {
                search: Some("breaking bad us".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            alt.total_records, 2,
            "alternate titles are searchable (both samples share the alias)"
        );
        let desc = list(
            &db,
            &LibraryQuery {
                sort_key: Some("title".into()),
                sort_direction: Some(SortDirection::Desc),
                page_size: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(desc.records.len(), 1);
        assert_eq!(desc.records[0].title, "The Office");

        let detail = get_detail(&db, a).await.unwrap();
        assert_eq!(detail.seasons.len(), 1);
        assert_eq!(detail.seasons[0].episode_file_count, 1);
        assert_eq!(detail.external_ids[0].value, "81189");

        apply_update(
            &db,
            a,
            &SeriesUpdate {
                monitored: Some(false),
                seasons: Some(vec![SeasonMonitoring {
                    season_number: 1,
                    monitored: false,
                }]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let eps = list_episodes(&db, a, Some(1)).await.unwrap();
        assert_eq!(eps.len(), 3);
        assert!(eps.iter().all(|e| !e.monitored));
        assert_eq!(eps[0].file.as_ref().unwrap().quality_name, "Bluray-1080p");

        let ids: Vec<i64> = eps.iter().map(|e| e.id).collect();
        assert_eq!(set_episodes_monitored(&db, &ids, true).await.unwrap(), 3);

        let cal = calendar(&db, "2008-01-01T00:00:00Z", "2008-02-01T00:00:00Z", true)
            .await
            .unwrap();
        assert_eq!(cal.len(), 3);

        delete(&db, a).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(left, 0, "episodes cascade with their series");
    }

    #[tokio::test]
    async fn provider_identity_is_unique() {
        let db = crate::test_support::db().await;
        let mut conn = db.pool().acquire().await.unwrap();
        insert(&mut conn, &sample("A", "1")).await.unwrap();
        let mut dup = sample("B", "1");
        dup.path = "/media/tv/B".into();
        assert!(matches!(
            insert(&mut conn, &dup).await,
            Err(DbError::Conflict(_))
        ));
        let mut other_provider = dup.clone();
        other_provider.metadata_provider = MetadataProvider::AniList;
        insert(&mut conn, &other_provider).await.unwrap();
        assert!(
            find_by_metadata(&mut conn, MetadataProvider::AniList, "1")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_by_external_id(&mut conn, ExternalIdKind::Tvdb, "1")
                .await
                .unwrap()
                .is_some()
        );
    }
}

/// Provider-owned fields refreshed from a metadata catalogue. User choices (monitoring,
/// profile, path, tags, series type) are never touched by a refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesMetadata {
    pub title: String,
    pub year: Option<i32>,
    pub status: SeriesStatus,
    pub overview: Option<String>,
    pub network: Option<String>,
    pub air_time: Option<String>,
    pub runtime: Option<i32>,
    pub original_language: Option<i32>,
    pub genres: Vec<String>,
    pub images: Vec<MediaImage>,
    pub ratings: Option<serde_json::Value>,
    pub first_aired: Option<String>,
    pub last_aired: Option<String>,
    pub external_ids: Vec<ExternalId>,
    pub alternate_titles: Vec<String>,
}

pub async fn update_metadata(
    conn: &mut SqliteConnection,
    id: i64,
    m: &SeriesMetadata,
) -> DbResult<()> {
    let title = m.title.trim();
    let r = sqlx::query(
        "UPDATE series SET title = ?, sort_title = ?, clean_title = ?, title_slug = ?, year = ?, status = ?, overview = ?, \
         network = ?, air_time = ?, runtime = ?, original_language = COALESCE(?, original_language), genres = ?, images = ?, \
         ratings = ?, first_aired = ?, last_aired = ?, last_refreshed_at = ? WHERE id = ?",
    )
    .bind(title)
    .bind(kitsunarr_domain::text::sort_title(title))
    .bind(kitsunarr_domain::text::clean_title(title))
    .bind(kitsunarr_domain::text::slugify(title))
    .bind(m.year)
    .bind(m.status.as_str())
    .bind(&m.overview)
    .bind(&m.network)
    .bind(&m.air_time)
    .bind(m.runtime)
    .bind(m.original_language)
    .bind(to_json(&m.genres)?)
    .bind(to_json(&m.images)?)
    .bind(m.ratings.as_ref().map(to_json).transpose()?)
    .bind(&m.first_aired)
    .bind(&m.last_aired)
    .bind(crate::now())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    for ext in &m.external_ids {
        sqlx::query("INSERT OR IGNORE INTO series_external_ids (series_id, kind, external_id) VALUES (?, ?, ?)")
            .bind(id)
            .bind(ext.kind.as_str())
            .bind(&ext.value)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("DELETE FROM series_alternate_titles WHERE series_id = ? AND source = 'provider'")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for alt in &m.alternate_titles {
        sqlx::query(
            "INSERT OR IGNORE INTO series_alternate_titles (series_id, title, clean_title, season_number, source) VALUES (?, ?, ?, NULL, 'provider')",
        )
        .bind(id)
        .bind(alt.trim())
        .bind(kitsunarr_domain::text::clean_title(alt))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Inserts a new episode, or refreshes the provider fields of an existing one while keeping
/// its monitoring flag and file link. Returns `true` when a row was inserted.
pub async fn merge_episode(conn: &mut SqliteConnection, e: &NewEpisode) -> DbResult<bool> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM episodes WHERE series_id = ? AND season_number = ? AND episode_number = ?",
    )
    .bind(e.series_id)
    .bind(e.season_number)
    .bind(e.episode_number)
    .fetch_optional(&mut *conn)
    .await?;
    match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE episodes SET absolute_episode_number = COALESCE(?, absolute_episode_number), title = COALESCE(?, title), \
                 overview = COALESCE(?, overview), air_date = COALESCE(?, air_date), air_date_utc = COALESCE(?, air_date_utc), \
                 runtime = COALESCE(?, runtime), provider_episode_id = COALESCE(?, provider_episode_id) WHERE id = ?",
            )
            .bind(e.absolute_episode_number)
            .bind(&e.title)
            .bind(&e.overview)
            .bind(&e.air_date)
            .bind(&e.air_date_utc)
            .bind(e.runtime)
            .bind(&e.provider_episode_id)
            .bind(id)
            .execute(&mut *conn)
            .await?;
            Ok(false)
        }
        None => {
            upsert_episode(conn, e).await?;
            Ok(true)
        }
    }
}

/// Inserts a season if it does not exist yet; existing seasons keep their monitoring flag.
pub async fn ensure_season(
    conn: &mut SqliteConnection,
    series_id: i64,
    season_number: i32,
    monitored: bool,
    title: Option<&str>,
) -> DbResult<bool> {
    let r = sqlx::query("INSERT OR IGNORE INTO seasons (series_id, season_number, monitored, title) VALUES (?, ?, ?, ?)")
        .bind(series_id)
        .bind(season_number)
        .bind(monitored)
        .bind(title)
        .execute(&mut *conn)
        .await?;
    Ok(r.rows_affected() == 1)
}
