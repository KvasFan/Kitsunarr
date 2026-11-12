//! Movies and movie files.

use kitsunarr_domain::api::library::{
    BulkEdit, CollectionRef, LibraryQuery, MovieDetail, MovieFileItem, MovieListItem,
};
use kitsunarr_domain::{
    ExternalId, ExternalIdKind, MediaImage, MetadataProvider, MinimumAvailability, MovieStatus,
    Page, Quality, SortDirection,
};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqliteConnection};

use super::{int_list, like_pattern, poster_url};
use crate::{Database, DbError, DbResult, from_json, to_json};

#[derive(Debug, Clone, PartialEq)]
pub struct NewMovie {
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub status: MovieStatus,
    pub overview: Option<String>,
    pub studio: Option<String>,
    pub runtime: Option<i32>,
    pub certification: Option<String>,
    pub original_language: Option<i32>,
    pub genres: Vec<String>,
    pub images: Vec<MediaImage>,
    pub ratings: Option<serde_json::Value>,
    pub in_cinemas: Option<String>,
    pub digital_release: Option<String>,
    pub physical_release: Option<String>,
    pub metadata_provider: MetadataProvider,
    pub metadata_id: String,
    pub monitored: bool,
    pub minimum_availability: MinimumAvailability,
    pub quality_profile_id: i64,
    pub root_folder_path: String,
    pub path: String,
    pub added_at: String,
    pub last_refreshed_at: Option<String>,
    pub tags: Vec<i64>,
    pub external_ids: Vec<ExternalId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMovieFile {
    pub movie_id: i64,
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
    pub edition: Option<String>,
}

fn validate(m: &NewMovie) -> DbResult<()> {
    if m.title.trim().is_empty() || m.path.trim().is_empty() || m.metadata_id.trim().is_empty() {
        return Err(DbError::Invalid(
            "title, path and metadata id are required".into(),
        ));
    }
    Ok(())
}

pub async fn insert(conn: &mut SqliteConnection, m: &NewMovie) -> DbResult<i64> {
    validate(m)?;
    let title = m.title.trim();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO movies (title, sort_title, clean_title, title_slug, original_title, year, status, overview, studio, runtime, \
         certification, original_language, genres, images, ratings, in_cinemas, digital_release, physical_release, \
         metadata_provider, metadata_id, monitored, minimum_availability, quality_profile_id, root_folder_path, path, added_at, \
         last_refreshed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(title)
    .bind(kitsunarr_domain::text::sort_title(title))
    .bind(kitsunarr_domain::text::clean_title(title))
    .bind(kitsunarr_domain::text::slugify(title))
    .bind(&m.original_title)
    .bind(m.year)
    .bind(m.status.as_str())
    .bind(&m.overview)
    .bind(&m.studio)
    .bind(m.runtime)
    .bind(&m.certification)
    .bind(m.original_language)
    .bind(to_json(&m.genres)?)
    .bind(to_json(&m.images)?)
    .bind(m.ratings.as_ref().map(to_json).transpose()?)
    .bind(&m.in_cinemas)
    .bind(&m.digital_release)
    .bind(&m.physical_release)
    .bind(m.metadata_provider.as_str())
    .bind(m.metadata_id.trim())
    .bind(m.monitored)
    .bind(m.minimum_availability.as_str())
    .bind(m.quality_profile_id)
    .bind(&m.root_folder_path)
    .bind(&m.path)
    .bind(&m.added_at)
    .bind(&m.last_refreshed_at)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "movie"))?;
    replace_links(conn, id, m).await?;
    Ok(id)
}

pub async fn update_full(conn: &mut SqliteConnection, id: i64, m: &NewMovie) -> DbResult<()> {
    validate(m)?;
    let title = m.title.trim();
    let r = sqlx::query(
        "UPDATE movies SET title = ?, sort_title = ?, clean_title = ?, title_slug = ?, original_title = ?, year = ?, status = ?, \
         overview = ?, studio = ?, runtime = ?, certification = ?, original_language = ?, genres = ?, images = ?, ratings = ?, \
         in_cinemas = ?, digital_release = ?, physical_release = ?, metadata_provider = ?, metadata_id = ?, monitored = ?, \
         minimum_availability = ?, quality_profile_id = ?, root_folder_path = ?, path = ? WHERE id = ?",
    )
    .bind(title)
    .bind(kitsunarr_domain::text::sort_title(title))
    .bind(kitsunarr_domain::text::clean_title(title))
    .bind(kitsunarr_domain::text::slugify(title))
    .bind(&m.original_title)
    .bind(m.year)
    .bind(m.status.as_str())
    .bind(&m.overview)
    .bind(&m.studio)
    .bind(m.runtime)
    .bind(&m.certification)
    .bind(m.original_language)
    .bind(to_json(&m.genres)?)
    .bind(to_json(&m.images)?)
    .bind(m.ratings.as_ref().map(to_json).transpose()?)
    .bind(&m.in_cinemas)
    .bind(&m.digital_release)
    .bind(&m.physical_release)
    .bind(m.metadata_provider.as_str())
    .bind(m.metadata_id.trim())
    .bind(m.monitored)
    .bind(m.minimum_availability.as_str())
    .bind(m.quality_profile_id)
    .bind(&m.root_folder_path)
    .bind(&m.path)
    .bind(id)
    .execute(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "movie"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    replace_links(conn, id, m).await
}

async fn replace_links(conn: &mut SqliteConnection, id: i64, m: &NewMovie) -> DbResult<()> {
    sqlx::query("DELETE FROM movie_tags WHERE movie_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for tag in &m.tags {
        sqlx::query("INSERT OR IGNORE INTO movie_tags (movie_id, tag_id) VALUES (?, ?)")
            .bind(id)
            .bind(tag)
            .execute(&mut *conn)
            .await
            .map_err(|e| DbError::from_write(e, "movie tag"))?;
    }
    sqlx::query("DELETE FROM movie_external_ids WHERE movie_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for ext in &m.external_ids {
        sqlx::query("INSERT OR IGNORE INTO movie_external_ids (movie_id, kind, external_id) VALUES (?, ?, ?)")
            .bind(id)
            .bind(ext.kind.as_str())
            .bind(&ext.value)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Inserts or replaces the single file record of a movie.
pub async fn upsert_file(conn: &mut SqliteConnection, f: &NewMovieFile) -> DbResult<i64> {
    sqlx::query_scalar(
        "INSERT INTO movie_files (movie_id, relative_path, size, added_at, scene_name, release_group, quality_id, revision_version, \
         revision_real, is_repack, languages, media_info, original_path, edition) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(movie_id) DO UPDATE SET relative_path = excluded.relative_path, size = excluded.size, \
         scene_name = excluded.scene_name, release_group = excluded.release_group, quality_id = excluded.quality_id, \
         revision_version = excluded.revision_version, revision_real = excluded.revision_real, is_repack = excluded.is_repack, \
         languages = excluded.languages, media_info = excluded.media_info, original_path = excluded.original_path, \
         edition = excluded.edition RETURNING id",
    )
    .bind(f.movie_id)
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
    .bind(&f.edition)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "movie file"))
}

pub async fn find_by_metadata(
    conn: &mut SqliteConnection,
    provider: MetadataProvider,
    id: &str,
) -> DbResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM movies WHERE metadata_provider = ? AND metadata_id = ?")
            .bind(provider.as_str())
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

pub async fn find_by_path(conn: &mut SqliteConnection, path: &str) -> DbResult<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM movies WHERE path = ?")
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
        "SELECT movie_id FROM movie_external_ids WHERE kind = ? AND external_id = ? LIMIT 1",
    )
    .bind(kind.as_str())
    .bind(value)
    .fetch_optional(&mut *conn)
    .await?)
}

#[derive(FromRow)]
struct ListRow {
    id: i64,
    title: String,
    sort_title: String,
    year: Option<i64>,
    status: String,
    studio: Option<String>,
    images: String,
    monitored: bool,
    minimum_availability: String,
    quality_profile_id: i64,
    quality_profile_name: String,
    metadata_provider: String,
    path: String,
    has_file: bool,
    size_on_disk: i64,
    added_at: String,
    tags: Option<String>,
}

impl ListRow {
    fn into_item(self) -> MovieListItem {
        MovieListItem {
            id: self.id,
            title: self.title,
            sort_title: self.sort_title,
            year: self.year.map(|y| y as i32),
            status: self.status.parse().unwrap_or(MovieStatus::Unknown),
            studio: self.studio,
            poster_url: poster_url(&self.images),
            monitored: self.monitored,
            minimum_availability: self
                .minimum_availability
                .parse()
                .unwrap_or(MinimumAvailability::Released),
            quality_profile_id: self.quality_profile_id,
            quality_profile_name: self.quality_profile_name,
            metadata_provider: self
                .metadata_provider
                .parse()
                .unwrap_or(MetadataProvider::TmdbMovie),
            path: self.path,
            has_file: self.has_file,
            size_on_disk: self.size_on_disk,
            added_at: self.added_at,
            tags: int_list(self.tags.as_deref()),
        }
    }
}

const LIST_COLUMNS: &str = "m.id, m.title, m.sort_title, m.year, m.status, m.studio, m.images, m.monitored, m.minimum_availability, \
    m.quality_profile_id, qp.name AS quality_profile_name, m.metadata_provider, m.path, m.added_at, \
    EXISTS (SELECT 1 FROM movie_files f WHERE f.movie_id = m.id) AS has_file, \
    (SELECT COALESCE(SUM(f.size), 0) FROM movie_files f WHERE f.movie_id = m.id) AS size_on_disk, \
    (SELECT json_group_array(t.tag_id) FROM movie_tags t WHERE t.movie_id = m.id) AS tags";

fn sort_expression(key: Option<&str>) -> &'static str {
    match key.unwrap_or("title") {
        "added" | "addedAt" => "m.added_at",
        "year" => "m.year",
        "status" => "m.status",
        "size" | "sizeOnDisk" => {
            "(SELECT COALESCE(SUM(f.size), 0) FROM movie_files f WHERE f.movie_id = m.id)"
        }
        "path" => "m.path",
        _ => "m.sort_title",
    }
}

fn push_filters(qb: &mut QueryBuilder<Sqlite>, q: &LibraryQuery) {
    qb.push(" WHERE 1 = 1");
    if let Some(m) = q.monitored {
        qb.push(" AND m.monitored = ").push_bind(m);
    }
    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND m.status = ").push_bind(status.to_string());
    }
    if let Some(tag) = q.tag_id {
        qb.push(" AND EXISTS (SELECT 1 FROM movie_tags t WHERE t.movie_id = m.id AND t.tag_id = ")
            .push_bind(tag)
            .push(")");
    }
    if let Some(p) = q.quality_profile_id {
        qb.push(" AND m.quality_profile_id = ").push_bind(p);
    }
    if let Some(p) = q.provider {
        qb.push(" AND m.metadata_provider = ").push_bind(p.as_str());
    }
    if let Some(term) = q.search.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        qb.push(" AND (m.title LIKE ")
            .push_bind(like_pattern(term))
            .push(" ESCAPE '\\'");
        qb.push(" OR m.clean_title LIKE ")
            .push_bind(like_pattern(&kitsunarr_domain::text::clean_title(term)))
            .push(" ESCAPE '\\'");
        qb.push(" OR m.original_title LIKE ")
            .push_bind(like_pattern(term))
            .push(" ESCAPE '\\')");
    }
}

pub async fn list(db: &Database, q: &LibraryQuery) -> DbResult<Page<MovieListItem>> {
    let paging = q.page_request();
    let mut count_qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM movies m");
    push_filters(&mut count_qb, q);
    let total: i64 = count_qb.build_query_scalar().fetch_one(db.pool()).await?;
    let dir = if paging.direction() == SortDirection::Desc {
        "DESC"
    } else {
        "ASC"
    };
    let mut qb = QueryBuilder::<Sqlite>::new("SELECT ");
    qb.push(LIST_COLUMNS)
        .push(" FROM movies m JOIN quality_profiles qp ON qp.id = m.quality_profile_id");
    push_filters(&mut qb, q);
    qb.push(format_args!(
        " ORDER BY {} {dir}, m.id {dir} LIMIT ",
        sort_expression(paging.sort_key.as_deref())
    ));
    qb.push_bind(i64::from(paging.page_size()))
        .push(" OFFSET ")
        .push_bind(paging.offset() as i64);
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
    root_folder_path: String,
    in_cinemas: Option<String>,
    digital_release: Option<String>,
    physical_release: Option<String>,
    last_refreshed_at: Option<String>,
}

#[derive(FromRow)]
struct FileRow {
    id: i64,
    relative_path: String,
    size: i64,
    quality_id: i64,
    languages: String,
    release_group: Option<String>,
    added_at: String,
}

pub async fn get_detail(db: &Database, id: i64) -> DbResult<MovieDetail> {
    let sql = format!(
        "SELECT {LIST_COLUMNS} FROM movies m JOIN quality_profiles qp ON qp.id = m.quality_profile_id WHERE m.id = ?"
    );
    let item: ListRow = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(db.pool())
        .await?
        .ok_or(DbError::NotFound)?;
    let d: DetailRow = sqlx::query_as(
        "SELECT overview, runtime, certification, genres, images, metadata_id, root_folder_path, in_cinemas, digital_release, \
         physical_release, last_refreshed_at FROM movies WHERE id = ?",
    )
    .bind(id)
    .fetch_one(db.pool())
    .await?;
    let file: Option<FileRow> = sqlx::query_as(
        "SELECT id, relative_path, size, quality_id, languages, release_group, added_at FROM movie_files WHERE movie_id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    let external: Vec<(String, String)> = sqlx::query_as(
        "SELECT kind, external_id FROM movie_external_ids WHERE movie_id = ? ORDER BY kind",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    let collections: Vec<(i64, String)> = sqlx::query_as(
        "SELECT c.id, c.title FROM collections c JOIN collection_members m ON m.collection_id = c.id WHERE m.movie_id = ? ORDER BY c.title",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    Ok(MovieDetail {
        item: item.into_item(),
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
        root_folder_path: d.root_folder_path,
        in_cinemas: d.in_cinemas,
        digital_release: d.digital_release,
        physical_release: d.physical_release,
        last_refreshed_at: d.last_refreshed_at,
        file: file.map(|f| {
            let qid = f.quality_id as i32;
            MovieFileItem {
                id: f.id,
                relative_path: f.relative_path,
                size: f.size,
                quality_id: qid,
                quality_name: Quality::by_id(qid)
                    .unwrap_or_else(Quality::unknown)
                    .name
                    .to_string(),
                languages: serde_json::from_str(&f.languages).unwrap_or_default(),
                release_group: f.release_group,
                added_at: f.added_at,
            }
        }),
        collections: collections
            .into_iter()
            .map(|(id, title)| CollectionRef { id, title })
            .collect(),
    })
}

pub async fn set_monitored(db: &Database, id: i64, monitored: bool) -> DbResult<()> {
    let r = sqlx::query("UPDATE movies SET monitored = ? WHERE id = ?")
        .bind(monitored)
        .bind(id)
        .execute(db.pool())
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn bulk_edit(db: &Database, edit: &BulkEdit) -> DbResult<u64> {
    let mut tx = db.pool().begin().await?;
    let mut updated = 0;
    for id in &edit.ids {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM movies WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            continue;
        }
        if let Some(m) = edit.monitored {
            sqlx::query("UPDATE movies SET monitored = ? WHERE id = ?")
                .bind(m)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(p) = edit.quality_profile_id {
            sqlx::query("UPDATE movies SET quality_profile_id = ? WHERE id = ?")
                .bind(p)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|e| DbError::from_write(e, "quality profile"))?;
        }
        for tag in &edit.add_tags {
            sqlx::query("INSERT OR IGNORE INTO movie_tags (movie_id, tag_id) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(&mut *tx)
                .await
                .map_err(|e| DbError::from_write(e, "tag"))?;
        }
        for tag in &edit.remove_tags {
            sqlx::query("DELETE FROM movie_tags WHERE movie_id = ? AND tag_id = ?")
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

/// Removes the movie record (and its file record). Files on disk are never touched here.
pub async fn delete(db: &Database, id: i64) -> DbResult<()> {
    let r = sqlx::query("DELETE FROM movies WHERE id = ?")
        .bind(id)
        .execute(db.pool())
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Refreshes provider-owned movie fields; user settings stay untouched.
#[allow(clippy::too_many_arguments)]
pub async fn update_metadata(
    conn: &mut SqliteConnection,
    id: i64,
    title: &str,
    original_title: Option<&str>,
    year: Option<i32>,
    status: MovieStatus,
    overview: Option<&str>,
    studio: Option<&str>,
    runtime: Option<i32>,
    genres: &[String],
    images: &[MediaImage],
    in_cinemas: Option<&str>,
    external_ids: &[ExternalId],
) -> DbResult<()> {
    let title = title.trim();
    let r = sqlx::query(
        "UPDATE movies SET title = ?, sort_title = ?, clean_title = ?, title_slug = ?, original_title = ?, year = ?, status = ?, \
         overview = ?, studio = ?, runtime = ?, genres = ?, images = ?, in_cinemas = ?, last_refreshed_at = ? WHERE id = ?",
    )
    .bind(title)
    .bind(kitsunarr_domain::text::sort_title(title))
    .bind(kitsunarr_domain::text::clean_title(title))
    .bind(kitsunarr_domain::text::slugify(title))
    .bind(original_title)
    .bind(year)
    .bind(status.as_str())
    .bind(overview)
    .bind(studio)
    .bind(runtime)
    .bind(to_json(genres)?)
    .bind(to_json(images)?)
    .bind(in_cinemas)
    .bind(crate::now())
    .bind(id)
    .execute(&mut *conn)
    .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    for ext in external_ids {
        sqlx::query("INSERT OR IGNORE INTO movie_external_ids (movie_id, kind, external_id) VALUES (?, ?, ?)")
            .bind(id)
            .bind(ext.kind.as_str())
            .bind(&ext.value)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// A monitored film as RSS releases are matched with it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MovieMatch {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i64>,
    pub imdb_id: Option<String>,
    pub metadata_provider: String,
}

/// Monitored films, for RSS sync (Tanukarr's movie RSS mapping).
pub async fn monitored_for_matching(db: &Database) -> DbResult<Vec<MovieMatch>> {
    Ok(sqlx::query_as(
        "SELECT m.id, m.title, m.original_title, m.year, \
         (SELECT external_id FROM movie_external_ids x WHERE x.movie_id = m.id AND x.kind = 'imdb' LIMIT 1) AS imdb_id, \
         m.metadata_provider FROM movies m WHERE m.monitored = 1",
    )
    .fetch_all(db.pool())
    .await?)
}

/// When a film was last grabbed, if its most recent history event is that grab (the history
/// rule of RSS sync: a film being downloaded is not grabbed again).
pub async fn pending_grab_date(db: &Database, id: i64) -> DbResult<Option<String>> {
    let last: Option<(String, String)> = sqlx::query_as(
        "SELECT event_type, date FROM history WHERE movie_id = ? ORDER BY date DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    Ok(last.filter(|(e, _)| e == "grabbed").map(|(_, d)| d))
}

/// A film's original title (searches and release matching).
pub async fn original_title(db: &Database, id: i64) -> DbResult<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, Option<String>>("SELECT original_title FROM movies WHERE id = ?")
            .bind(id)
            .fetch_optional(db.pool())
            .await?
            .flatten(),
    )
}

/// Records when a film was last searched for.
pub async fn set_last_search(db: &Database, id: i64, at: &str) -> DbResult<()> {
    sqlx::query("UPDATE movies SET last_search_at = ? WHERE id = ?")
        .bind(at)
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample(title: &str, id: &str) -> NewMovie {
        NewMovie {
            title: title.into(),
            original_title: Some("カウボーイビバップ 天国の扉".into()),
            year: Some(2001),
            status: MovieStatus::Released,
            overview: None,
            studio: Some("Sunrise".into()),
            runtime: Some(115),
            certification: None,
            original_language: Some(8),
            genres: vec![],
            images: vec![],
            ratings: None,
            in_cinemas: None,
            digital_release: None,
            physical_release: None,
            metadata_provider: MetadataProvider::AniList,
            metadata_id: id.into(),
            monitored: true,
            minimum_availability: MinimumAvailability::Released,
            quality_profile_id: 1,
            root_folder_path: "/media/movies".into(),
            path: format!("/media/movies/{title}"),
            added_at: "2026-01-01T00:00:00Z".into(),
            last_refreshed_at: None,
            tags: vec![],
            external_ids: vec![ExternalId {
                kind: ExternalIdKind::AniList,
                value: id.into(),
            }],
        }
    }

    #[tokio::test]
    async fn movie_lifecycle() {
        let db = crate::test_support::db().await;
        let mut conn = db.pool().acquire().await.unwrap();
        let id = insert(&mut conn, &sample("Cowboy Bebop: The Movie", "5"))
            .await
            .unwrap();
        upsert_file(
            &mut conn,
            &NewMovieFile {
                movie_id: id,
                relative_path: "Cowboy.Bebop.The.Movie.2001.1080p.BluRay.x264-AMIABLE.mkv".into(),
                size: 4_000,
                added_at: "2026-01-02T00:00:00Z".into(),
                scene_name: None,
                release_group: Some("AMIABLE".into()),
                quality_id: 7,
                revision_version: 1,
                revision_real: 0,
                is_repack: false,
                languages: vec![8],
                media_info: None,
                original_path: None,
                edition: None,
            },
        )
        .await
        .unwrap();
        drop(conn);
        let page = list(
            &db,
            &LibraryQuery {
                search: Some("天国".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.total_records, 1, "original titles are searchable");
        assert!(page.records[0].has_file);
        let detail = get_detail(&db, id).await.unwrap();
        assert_eq!(detail.file.unwrap().quality_name, "Bluray-1080p");
        set_monitored(&db, id, false).await.unwrap();
        delete(&db, id).await.unwrap();
        let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM movie_files")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(files, 0);
    }
}
