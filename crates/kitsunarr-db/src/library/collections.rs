//! Collections: provider collections (TMDB), anime groups and manual groups.

use kitsunarr_domain::api::library::{
    CollectionDetail, CollectionEdit, CollectionEntry, CollectionItem, CollectionMember,
    CollectionMemberEdit,
};
use kitsunarr_domain::{MediaImage, MetadataProvider};
use sqlx::{FromRow, SqliteConnection};

use super::poster_url;
use crate::{Database, DbError, DbResult, now, to_json};

#[derive(Debug, Clone, PartialEq)]
pub struct NewCollection {
    pub title: String,
    /// `manual`, `provider` or `animeGroup`.
    pub kind: String,
    pub metadata_provider: Option<MetadataProvider>,
    pub metadata_id: Option<String>,
    pub overview: Option<String>,
    pub images: Vec<MediaImage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Member {
    Series(i64),
    Movie(i64),
}

/// Automatic grouping never overrides a stored membership or an explicit removal.
pub async fn automatic_grouping_blocked(
    conn: &mut SqliteConnection,
    member: Member,
) -> DbResult<bool> {
    let (series, movie) = match member {
        Member::Series(id) => (Some(id), None),
        Member::Movie(id) => (None, Some(id)),
    };
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM collection_members WHERE series_id IS ?1 AND movie_id IS ?2) OR EXISTS(SELECT 1 FROM collection_grouping_exclusions WHERE series_id IS ?1 AND movie_id IS ?2)")
        .bind(series).bind(movie).fetch_one(conn).await?)
}

fn member_ids(member: Member) -> (Option<i64>, Option<i64>) {
    match member {
        Member::Series(id) => (Some(id), None),
        Member::Movie(id) => (None, Some(id)),
    }
}

/// Whether the title is kept out of automatic grouping (Tanukarr's `AnimeGroupingMode.Disabled`,
/// or an explicit removal from a group).
pub async fn automatic_grouping_disabled(
    conn: &mut SqliteConnection,
    member: Member,
) -> DbResult<bool> {
    let (series, movie) = member_ids(member);
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM collection_grouping_exclusions WHERE series_id IS ?1 AND movie_id IS ?2)",
    )
    .bind(series)
    .bind(movie)
    .fetch_one(conn)
    .await?)
}

/// Sets Tanukarr's grouping mode of a title: disabled keeps it out of automatic grouping and,
/// as `Series.ApplyChanges` clears the group, removes it from its anime groups and provider
/// collections (manual collections are kept); automatic lets the next refresh group it again.
/// Returns whether anything changed.
pub async fn set_automatic_grouping(
    conn: &mut SqliteConnection,
    member: Member,
    enabled: bool,
) -> DbResult<bool> {
    let (series, movie) = member_ids(member);
    if enabled {
        let done = sqlx::query(
            "DELETE FROM collection_grouping_exclusions WHERE series_id IS ?1 AND movie_id IS ?2",
        )
        .bind(series)
        .bind(movie)
        .execute(&mut *conn)
        .await?;
        return Ok(done.rows_affected() > 0);
    }
    let added = sqlx::query(
        "INSERT OR IGNORE INTO collection_grouping_exclusions (series_id, movie_id) VALUES (?, ?)",
    )
    .bind(series)
    .bind(movie)
    .execute(&mut *conn)
    .await?;
    let removed = sqlx::query(
        "DELETE FROM collection_members WHERE series_id IS ?1 AND movie_id IS ?2 \
         AND collection_id IN (SELECT id FROM collections WHERE kind IN ('animeGroup', 'provider'))",
    )
    .bind(series)
    .bind(movie)
    .execute(&mut *conn)
    .await?;
    Ok(added.rows_affected() + removed.rows_affected() > 0)
}

/// [`set_automatic_grouping`] in its own transaction.
pub async fn set_title_grouping(db: &Database, member: Member, enabled: bool) -> DbResult<bool> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let changed = set_automatic_grouping(&mut tx, member, enabled).await?;
    tx.commit().await?;
    Ok(changed)
}

pub async fn insert(conn: &mut SqliteConnection, c: &NewCollection) -> DbResult<i64> {
    if c.title.trim().is_empty() {
        return Err(DbError::Invalid("collection title is required".into()));
    }
    sqlx::query_scalar(
        "INSERT INTO collections (title, sort_title, kind, metadata_provider, metadata_id, overview, images, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(c.title.trim())
    .bind(kitsunarr_domain::text::sort_title(&c.title))
    .bind(&c.kind)
    .bind(c.metadata_provider.map(|p| p.as_str()))
    .bind(&c.metadata_id)
    .bind(&c.overview)
    .bind(to_json(&c.images)?)
    .bind(now())
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "collection"))
}

/// Finds a collection by exact title and kind (used to merge Tanukarr groups by title).
pub async fn find_by_title(
    conn: &mut SqliteConnection,
    title: &str,
    kind: &str,
) -> DbResult<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM collections WHERE title = ? AND kind = ? LIMIT 1")
            .bind(title.trim())
            .bind(kind)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

pub async fn add_member(
    conn: &mut SqliteConnection,
    collection_id: i64,
    member: Member,
    position: i32,
    role: Option<&str>,
    label: Option<&str>,
) -> DbResult<()> {
    let (series_id, movie_id) = match member {
        Member::Series(id) => (Some(id), None),
        Member::Movie(id) => (None, Some(id)),
    };
    sqlx::query(
        "INSERT OR IGNORE INTO collection_members (collection_id, position, series_id, movie_id, role, label) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(collection_id)
    .bind(position)
    .bind(series_id)
    .bind(movie_id)
    .bind(role)
    .bind(label)
    .execute(&mut *conn)
    .await
    .map_err(|e| DbError::from_write(e, "collection member"))?;
    Ok(())
}

#[derive(FromRow)]
struct Row {
    id: i64,
    title: String,
    kind: String,
    images: String,
    member_count: i64,
    series_count: i64,
    movie_count: i64,
    first_series_images: Option<String>,
    first_movie_images: Option<String>,
}

pub async fn list(db: &Database) -> DbResult<Vec<CollectionItem>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT c.id, c.title, c.kind, c.images, \
           (SELECT COUNT(*) FROM collection_members m WHERE m.collection_id = c.id) AS member_count, \
           (SELECT COUNT(*) FROM collection_members m WHERE m.collection_id = c.id AND m.series_id IS NOT NULL) AS series_count, \
           (SELECT COUNT(*) FROM collection_members m WHERE m.collection_id = c.id AND m.movie_id IS NOT NULL) AS movie_count, \
           (SELECT s.images FROM collection_members m JOIN series s ON s.id = m.series_id WHERE m.collection_id = c.id ORDER BY m.position LIMIT 1) AS first_series_images, \
           (SELECT v.images FROM collection_members m JOIN movies v ON v.id = m.movie_id WHERE m.collection_id = c.id ORDER BY m.position LIMIT 1) AS first_movie_images \
         FROM collections c ORDER BY c.sort_title",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            // A collection without its own artwork borrows its first member's poster.
            let poster = poster_url(&r.images)
                .or_else(|| r.first_series_images.as_deref().and_then(poster_url))
                .or_else(|| r.first_movie_images.as_deref().and_then(poster_url));
            CollectionItem {
                id: r.id,
                title: r.title,
                kind: r.kind,
                poster_url: poster,
                member_count: r.member_count,
                series_count: r.series_count,
                movie_count: r.movie_count,
            }
        })
        .collect())
}

/// (group title, role, ordinal).
pub type GroupPlace = (String, Option<String>, Option<i32>);

/// A series' franchise or manual group, for naming: (group title, role, ordinal). The ordinal
/// is the label's number ("Season 2", "Film 03"), else the next slot after preceding members and their native seasons.
pub async fn group_of_series(db: &Database, series_id: i64) -> DbResult<Option<GroupPlace>> {
    type Row = (i64, i64, String, Option<String>);
    let row: Option<Row> = sqlx::query_as(
        "SELECT m.id, c.id, c.title, m.role FROM collection_members m \
         JOIN collections c ON c.id = m.collection_id \
         WHERE m.series_id = ? AND c.kind IN ('animeGroup', 'manual') \
         ORDER BY CASE c.kind WHEN 'manual' THEN 0 ELSE 1 END, c.id LIMIT 1",
    )
    .bind(series_id)
    .fetch_optional(db.pool())
    .await?;
    let Some((member_id, collection_id, title, role)) = row else {
        return Ok(None);
    };
    let mut conn = db.pool().acquire().await?;
    let ordinal = member_slots(&mut conn, collection_id)
        .await?
        .into_iter()
        .find(|m| m.id == member_id)
        .map(|m| m.start);
    Ok(Some((title, role, ordinal)))
}

/// Numbering is shared by cards, naming tokens and manual slot allocation.
struct MemberSlots {
    id: i64,
    role: String,
    start: i32,
    span: i32,
}

fn effective_role(movie_id: Option<i64>, role: Option<&str>) -> &str {
    if movie_id.is_some() {
        "movie"
    } else {
        role.unwrap_or("season")
    }
}

fn numbered_slot(
    next: &mut std::collections::HashMap<String, i32>,
    role: &str,
    label: Option<&str>,
    span: i32,
) -> i32 {
    let cursor = next.entry(role.into()).or_insert(1);
    let start = member_number(label).unwrap_or(*cursor);
    *cursor = (*cursor).max(start.saturating_add(span));
    start
}

async fn member_slots(
    conn: &mut SqliteConnection,
    collection_id: i64,
) -> DbResult<Vec<MemberSlots>> {
    type Row = (i64, Option<i64>, Option<String>, Option<String>, i32);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT m.id, m.movie_id, m.role, m.label, MAX(1,(SELECT COUNT(*) FROM seasons s WHERE s.series_id = m.series_id AND s.season_number > 0)) \
         FROM collection_members m WHERE m.collection_id = ? ORDER BY m.position, m.id"
    ).bind(collection_id).fetch_all(conn).await?;
    let mut next = std::collections::HashMap::new();
    Ok(rows
        .into_iter()
        .map(|(id, movie, role, label, span)| {
            let role = effective_role(movie, role.as_deref()).to_string();
            let start = numbered_slot(&mut next, &role, label.as_deref(), span);
            MemberSlots {
                id,
                role,
                start,
                span,
            }
        })
        .collect())
}

/// Kinds a title belongs to at most one of (Tanukarr: one group per series).
const GROUP_KINDS: &str = "('manual', 'animeGroup')";

pub async fn detail(db: &Database, id: i64) -> DbResult<CollectionDetail> {
    let row: Option<(i64, String, String, Option<String>, String)> =
        sqlx::query_as("SELECT id, title, kind, overview, images FROM collections WHERE id = ?")
            .bind(id)
            .fetch_optional(db.pool())
            .await?;
    let Some((id, title, kind, overview, images)) = row else {
        return Err(DbError::NotFound);
    };
    #[derive(FromRow)]
    struct M {
        id: i64,
        series_id: Option<i64>,
        movie_id: Option<i64>,
        position: i64,
        role: Option<String>,
        label: Option<String>,
        title: Option<String>,
        year: Option<i64>,
        images: Option<String>,
        overview: Option<String>,
        monitored: bool,
        movie_has_file: bool,
    }
    let rows: Vec<M> = sqlx::query_as(
        "SELECT m.id, m.series_id, m.movie_id, m.position, m.role, m.label, \
           COALESCE(s.title, v.title) AS title, COALESCE(s.year, v.year) AS year, COALESCE(s.images, v.images) AS images, \
           COALESCE(s.overview, v.overview) AS overview, COALESCE(s.monitored, v.monitored, 0) AS monitored, \
           EXISTS(SELECT 1 FROM movie_files f WHERE f.movie_id = m.movie_id) AS movie_has_file \
         FROM collection_members m LEFT JOIN series s ON s.id = m.series_id LEFT JOIN movies v ON v.id = m.movie_id \
         WHERE m.collection_id = ? ORDER BY m.position, m.id",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    #[derive(FromRow)]
    struct Season {
        series_id: i64,
        season_number: i32,
        monitored: bool,
        title: Option<String>,
        images: String,
        episode_count: i64,
        episode_file_count: i64,
    }
    let seasons: Vec<Season> = sqlx::query_as(
        "SELECT se.series_id, se.season_number, se.monitored, se.title, se.images, \
         (SELECT COUNT(*) FROM episodes e WHERE e.series_id = se.series_id AND e.season_number = se.season_number) AS episode_count, \
         (SELECT COUNT(*) FROM episodes e WHERE e.series_id = se.series_id AND e.season_number = se.season_number AND e.episode_file_id IS NOT NULL) AS episode_file_count \
         FROM seasons se JOIN collection_members m ON m.series_id = se.series_id WHERE m.collection_id = ? ORDER BY se.series_id, se.season_number"
    ).bind(id).fetch_all(db.pool()).await?;
    let mut entries = Vec::new();
    let mut next = std::collections::HashMap::new();
    for m in &rows {
        let role = effective_role(m.movie_id, m.role.as_deref());
        let native: Vec<_> = seasons
            .iter()
            .filter(|s| {
                Some(s.series_id) == m.series_id && (s.season_number > 0 || s.episode_count > 0)
            })
            .collect();
        let regular = native.iter().filter(|s| s.season_number > 0).count().max(1) as i32;
        let base = numbered_slot(&mut next, role, m.label.as_deref(), regular);
        let section = match role {
            "movie" => "movies",
            "special" => "specials",
            _ => "seasons",
        };
        let poster = m.images.as_deref().and_then(poster_url);
        if m.movie_id.is_some() || native.is_empty() {
            entries.push(CollectionEntry {
                member_id: m.id,
                series_id: m.series_id,
                movie_id: m.movie_id,
                season_number: None,
                display_number: base,
                section: section.into(),
                title: m.title.clone().unwrap_or_default(),
                series_title: m.title.clone().unwrap_or_default(),
                year: m.year.map(|y| y as i32),
                overview: m.overview.clone(),
                poster_url: poster,
                monitored: m.monitored,
                episode_count: i64::from(m.movie_id.is_some()),
                episode_file_count: i64::from(m.movie_has_file),
            });
        } else {
            let mut offset = 0;
            for se in native {
                let section = if se.season_number == 0 {
                    "specials"
                } else {
                    section
                };
                let number = if se.season_number == 0 {
                    0
                } else {
                    let n = base + offset;
                    offset += 1;
                    n
                };
                entries.push(CollectionEntry {
                    member_id: m.id,
                    series_id: m.series_id,
                    movie_id: None,
                    season_number: Some(se.season_number),
                    display_number: number,
                    section: section.into(),
                    title: se
                        .title
                        .clone()
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or_else(|| m.title.clone().unwrap_or_default()),
                    series_title: m.title.clone().unwrap_or_default(),
                    year: m.year.map(|y| y as i32),
                    overview: m.overview.clone(),
                    poster_url: poster_url(&se.images).or_else(|| poster.clone()),
                    monitored: se.monitored,
                    episode_count: se.episode_count,
                    episode_file_count: se.episode_file_count,
                });
            }
        }
    }
    entries.sort_by_key(|e| {
        (
            match e.section.as_str() {
                "seasons" => 0,
                "movies" => 1,
                _ => 2,
            },
            e.display_number,
            e.member_id,
            e.season_number,
        )
    });
    Ok(CollectionDetail {
        id,
        title,
        kind,
        overview: overview.filter(|o| !o.trim().is_empty()).or_else(|| {
            rows.iter()
                .find_map(|m| m.overview.clone().filter(|o| !o.trim().is_empty()))
        }),
        poster_url: poster_url(&images).or_else(|| {
            rows.iter()
                .find_map(|m| m.images.as_deref().and_then(poster_url))
        }),
        entries,
        members: rows
            .into_iter()
            .map(|m| CollectionMember {
                id: m.id,
                series_id: m.series_id,
                movie_id: m.movie_id,
                title: m.title.unwrap_or_default(),
                year: m.year.map(|y| y as i32),
                poster_url: m.images.as_deref().and_then(poster_url),
                position: m.position as i32,
                role: m.role,
                label: m.label,
            })
            .collect(),
    })
}

/// Creates a manual collection.
pub async fn create_manual(db: &Database, edit: &CollectionEdit) -> DbResult<i64> {
    let mut conn = db.pool().acquire().await?;
    insert(
        &mut conn,
        &NewCollection {
            title: edit.title.clone(),
            kind: "manual".into(),
            metadata_provider: None,
            metadata_id: None,
            overview: edit.overview.clone().filter(|o| !o.trim().is_empty()),
            images: Vec::new(),
        },
    )
    .await
}

pub async fn rename(db: &Database, id: i64, edit: &CollectionEdit) -> DbResult<()> {
    if edit.title.trim().is_empty() {
        return Err(DbError::Invalid("collection title is required".into()));
    }
    let done = sqlx::query(
        "UPDATE collections SET title = ?, sort_title = ?, overview = COALESCE(?, overview) WHERE id = ?",
    )
    .bind(edit.title.trim())
    .bind(kitsunarr_domain::text::sort_title(&edit.title))
    .bind(edit.overview.as_deref().map(str::trim))
    .bind(id)
    .execute(db.pool())
    .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Deletes a collection; its members' series and movies are untouched.
pub async fn delete(db: &Database, id: i64) -> DbResult<()> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("INSERT OR IGNORE INTO collection_grouping_exclusions (series_id,movie_id) SELECT series_id,movie_id FROM collection_members WHERE collection_id = ?")
        .bind(id).execute(&mut *tx).await?;
    let done = sqlx::query("DELETE FROM collections WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    tx.commit().await?;
    Ok(())
}

/// Only generated numbering labels carry a slot. Digits in an arbitrary arc title or year
/// must never change collection numbering or file naming.
pub fn member_number(label: Option<&str>) -> Option<i32> {
    let label = label?.trim();
    let (prefix, number) = label.rsplit_once(' ')?;
    if !["season", "movie", "film", "special"]
        .iter()
        .any(|p| prefix.eq_ignore_ascii_case(p))
    {
        return None;
    }
    number.parse::<i32>().ok().filter(|n| (1..=999).contains(n))
}

fn label_for(role: Option<&str>, n: i64) -> Option<String> {
    match role {
        Some("season") => Some(format!("Season {n}")),
        Some("movie") => Some(format!("Movie {n}")),
        Some("special") => Some(format!("Special {n}")),
        _ => None,
    }
}

fn check_role(role: Option<&str>) -> DbResult<()> {
    match role {
        None | Some("season" | "movie" | "special") => Ok(()),
        Some(other) => Err(DbError::Invalid(format!(
            "role must be season, movie or special, not {other}"
        ))),
    }
}

/// Adds a series or movie (Tanukarr's manual grouping). A title leaves any other franchise or
/// manual group when it joins one.
pub async fn add_member_manually(
    db: &Database,
    collection_id: i64,
    edit: &CollectionMemberEdit,
) -> DbResult<i64> {
    let member = match (edit.series_id, edit.movie_id) {
        (Some(s), None) => Member::Series(s),
        (None, Some(m)) => Member::Movie(m),
        _ => {
            return Err(DbError::Invalid("give either a series or a movie".into()));
        }
    };
    if edit.position.is_some_and(|n| n < 1) {
        return Err(DbError::Invalid("position must be positive".into()));
    }
    let role = edit.role.as_deref().filter(|r| !r.is_empty());
    check_role(role)?;
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let kind: Option<String> = sqlx::query_scalar("SELECT kind FROM collections WHERE id = ?")
        .bind(collection_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(kind) = kind else {
        return Err(DbError::NotFound);
    };
    let (series_id, movie_id) = match member {
        Member::Series(id) => (Some(id), None),
        Member::Movie(id) => (None, Some(id)),
    };
    let exists: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM series WHERE id = ?1) + (SELECT COUNT(*) FROM movies WHERE id = ?2)",
    )
    .bind(series_id)
    .bind(movie_id)
    .fetch_one(&mut *tx)
    .await?;
    if exists == 0 {
        return Err(DbError::NotFound);
    }
    let already: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM collection_members WHERE collection_id = ? AND (series_id IS ? AND movie_id IS ?)",
    )
    .bind(collection_id)
    .bind(series_id)
    .bind(movie_id)
    .fetch_one(&mut *tx)
    .await?;
    if already > 0 {
        return Err(DbError::Conflict(
            "the title is already in this collection".into(),
        ));
    }
    if kind == "manual" || kind == "animeGroup" {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM collection_members WHERE (series_id IS ?1 AND movie_id IS ?2) AND collection_id IN \
             (SELECT id FROM collections WHERE kind IN {GROUP_KINDS})"
        )))
        .bind(series_id)
        .bind(movie_id)
        .execute(&mut *tx)
        .await?;
    }
    let next: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM collection_members WHERE collection_id = ?",
    )
    .bind(collection_id)
    .fetch_one(&mut *tx)
    .await?;
    let effective = effective_role(movie_id, role);
    let occupied = member_slots(&mut tx, collection_id).await?;
    let needed: i64 = if series_id.is_some() {
        sqlx::query_scalar(
            "SELECT MAX(1,COUNT(*)) FROM seasons WHERE series_id = ? AND season_number > 0",
        )
        .bind(series_id)
        .fetch_one(&mut *tx)
        .await?
    } else {
        1
    };
    let mut used = std::collections::HashSet::new();
    for m in occupied.into_iter().filter(|m| m.role == effective) {
        used.extend(m.start..m.start.saturating_add(m.span));
    }
    let number = (1..=999)
        .find(|n| {
            *n + needed as i32 - 1 <= 999
                && (*n..*n + needed as i32).all(|slot| !used.contains(&slot))
        })
        .ok_or_else(|| DbError::Invalid("no free collection numbers remain".into()))?;
    let label = edit
        .label
        .clone()
        .filter(|l| !l.trim().is_empty())
        .or_else(|| label_for(Some(effective), i64::from(number)));
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO collection_members (collection_id, position, series_id, movie_id, role, label) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(collection_id)
    .bind(edit.position.map_or(next, i64::from))
    .bind(series_id)
    .bind(movie_id)
    .bind(role)
    .bind(label.as_deref().map(str::trim))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| DbError::from_write(e, "collection member"))?;
    tx.commit().await?;
    Ok(id)
}

/// Changes a member's role, label or position.
pub async fn update_member(
    db: &Database,
    collection_id: i64,
    member_id: i64,
    edit: &CollectionMemberEdit,
) -> DbResult<()> {
    if edit.position.is_some_and(|n| n < 1) {
        return Err(DbError::Invalid("position must be positive".into()));
    }
    let role = edit.role.as_deref().filter(|r| !r.is_empty());
    check_role(role)?;
    let done = sqlx::query(
        "UPDATE collection_members SET role = CASE WHEN ? THEN ? ELSE role END, label = CASE WHEN ? THEN ? ELSE label END, \
         position = COALESCE(?, position) WHERE id = ? AND collection_id = ?",
    )
    .bind(edit.role.is_some())
    .bind(role)
    .bind(edit.label.is_some())
    .bind(
        edit.label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty()),
    )
    .bind(edit.position)
    .bind(member_id)
    .bind(collection_id)
    .execute(db.pool())
    .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn remove_member(db: &Database, collection_id: i64, member_id: i64) -> DbResult<()> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("INSERT OR IGNORE INTO collection_grouping_exclusions (series_id,movie_id) SELECT series_id,movie_id FROM collection_members WHERE id = ? AND collection_id = ?")
        .bind(member_id).bind(collection_id).execute(&mut *tx).await?;
    let done = sqlx::query("DELETE FROM collection_members WHERE id = ? AND collection_id = ?")
        .bind(member_id)
        .bind(collection_id)
        .execute(&mut *tx)
        .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn manual(db: &Database, title: &str) -> i64 {
        create_manual(
            db,
            &CollectionEdit {
                title: title.into(),
                overview: None,
            },
        )
        .await
        .unwrap()
    }

    #[test]
    fn only_numbering_labels_define_slots() {
        for (label, expected) in [
            ("Season 2", Some(2)),
            ("film 03", Some(3)),
            (" Movie 9 ", Some(9)),
            ("Special 1", Some(1)),
            ("Arc 2024", None),
            ("Season 0", None),
            ("Season 1000", None),
            ("Part 2", None),
        ] {
            assert_eq!(member_number(Some(label)), expected, "{label}");
        }
        assert_eq!(member_number(None), None);
    }

    #[tokio::test]
    async fn native_seasons_keep_their_identity_and_borrow_presentation() {
        let db = crate::test_support::db().await;
        let s = crate::library::series::tests::insert_sample(&db, "Saga", "1", 2).await;
        let c = manual(&db, "Collection").await;
        sqlx::query("UPDATE series SET overview = 'A shared story' WHERE id = ?")
            .bind(s)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(r#"INSERT INTO seasons (series_id, season_number, monitored, title, images) VALUES (?, 3, 0, 'Third arc', '[{"coverType":"poster","remoteUrl":"https://img/season.jpg"}]'), (?, 0, 1, NULL, '[]')"#)
            .bind(s).bind(s).execute(db.pool()).await.unwrap();
        // Only nonempty native specials are included.
        let edit = CollectionMemberEdit {
            series_id: Some(s),
            role: Some("season".into()),
            label: Some("Season 5".into()),
            ..Default::default()
        };
        add_member_manually(&db, c, &edit).await.unwrap();
        let d = detail(&db, c).await.unwrap();
        assert!(d.overview.is_some());
        assert_eq!(d.poster_url.as_deref(), Some("https://img/p.jpg"));
        assert_eq!(d.entries.len(), 2);
        let first = &d.entries[0];
        assert_eq!(
            (
                first.display_number,
                first.season_number,
                first.episode_count,
                first.episode_file_count
            ),
            (5, Some(1), 2, 1)
        );
        let third = &d.entries[1];
        assert_eq!(
            (third.display_number, third.season_number, third.monitored),
            (6, Some(3), false)
        );
        assert_eq!(third.title, "Third arc");
        assert_eq!(third.poster_url.as_deref(), Some("https://img/season.jpg"));
        sqlx::query("INSERT INTO episodes (series_id,season_number,episode_number) VALUES (?,0,1)")
            .bind(s)
            .execute(db.pool())
            .await
            .unwrap();
        let d = detail(&db, c).await.unwrap();
        assert_eq!(d.entries.len(), 3);
        assert_eq!(
            (d.entries[2].section.as_str(), d.entries[2].season_number),
            ("specials", Some(0))
        );
        assert_eq!(group_of_series(&db, s).await.unwrap().unwrap().2, Some(5));
    }

    #[tokio::test]
    async fn numbering_reserves_native_spans_and_reuses_holes() {
        let db = crate::test_support::db().await;
        let c = manual(&db, "Collection").await;
        let a = crate::library::series::tests::insert_sample(&db, "A", "1", 0).await;
        let b = crate::library::series::tests::insert_sample(&db, "B", "2", 0).await;
        let d = crate::library::series::tests::insert_sample(&db, "D", "3", 0).await;
        sqlx::query("INSERT INTO seasons (series_id,season_number) VALUES (?,2)")
            .bind(a)
            .execute(db.pool())
            .await
            .unwrap();
        let edit = |id| CollectionMemberEdit {
            series_id: Some(id),
            role: Some("season".into()),
            ..Default::default()
        };
        let first = add_member_manually(&db, c, &edit(a)).await.unwrap();
        add_member_manually(&db, c, &edit(b)).await.unwrap();
        assert_eq!(
            detail(&db, c).await.unwrap().members[1].label.as_deref(),
            Some("Season 3")
        );
        remove_member(&db, c, first).await.unwrap();
        add_member_manually(&db, c, &edit(d)).await.unwrap();
        let result = detail(&db, c).await.unwrap();
        assert_eq!(result.members[1].label.as_deref(), Some("Season 1"));
        assert_eq!(
            result
                .entries
                .iter()
                .map(|e| e.display_number)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        // Unlabelled migrated members and naming tokens follow the same spans.
        update_member(
            &db,
            c,
            result.members[0].id,
            &CollectionMemberEdit {
                role: Some("".into()),
                label: Some("Arc 2024".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        update_member(
            &db,
            c,
            result.members[1].id,
            &CollectionMemberEdit {
                role: Some("".into()),
                label: Some("".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(group_of_series(&db, d).await.unwrap().unwrap().2, Some(2));
        assert_eq!(detail(&db, c).await.unwrap().entries[1].display_number, 2);
    }

    #[tokio::test]
    async fn manual_removals_and_deletions_block_automatic_grouping() {
        let db = crate::test_support::db().await;
        let s = crate::library::series::tests::insert_sample(&db, "Saga", "1", 0).await;
        let c = manual(&db, "Collection").await;
        let mut conn = db.pool().acquire().await.unwrap();
        let m = crate::library::movies::insert(
            &mut conn,
            &crate::library::movies::tests::sample("Film", "2"),
        )
        .await
        .unwrap();
        assert!(
            !automatic_grouping_blocked(&mut conn, Member::Series(s))
                .await
                .unwrap()
        );
        drop(conn);
        let sid = add_member_manually(
            &db,
            c,
            &CollectionMemberEdit {
                series_id: Some(s),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        add_member_manually(
            &db,
            c,
            &CollectionMemberEdit {
                movie_id: Some(m),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        remove_member(&db, c, sid).await.unwrap();
        delete(&db, c).await.unwrap();
        let mut conn = db.pool().acquire().await.unwrap();
        assert!(
            automatic_grouping_blocked(&mut conn, Member::Series(s))
                .await
                .unwrap()
        );
        assert!(
            automatic_grouping_blocked(&mut conn, Member::Movie(m))
                .await
                .unwrap()
        );
        drop(conn);
        let other = manual(&db, "Elsewhere").await;
        add_member_manually(
            &db,
            other,
            &CollectionMemberEdit {
                series_id: Some(s),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            detail(&db, other).await.unwrap().members.len(),
            1,
            "manual reattachment is allowed"
        );
        assert!(matches!(
            remove_member(&db, other, i64::MAX).await,
            Err(DbError::NotFound)
        ));
        // Foreign keys clean exclusions when their title is removed.
        sqlx::query("DELETE FROM movies WHERE id = ?")
            .bind(m)
            .execute(db.pool())
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM collection_grouping_exclusions WHERE movie_id = ?",
        )
        .bind(m)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn mixed_collection() {
        let db = crate::test_support::db().await;
        let s = crate::library::series::tests::insert_sample(&db, "Cowboy Bebop", "76885", 1).await;
        let mut conn = db.pool().acquire().await.unwrap();
        let m = crate::library::movies::insert(
            &mut conn,
            &crate::library::movies::tests::sample("Cowboy Bebop: The Movie", "5"),
        )
        .await
        .unwrap();
        let c = insert(
            &mut conn,
            &NewCollection {
                title: "Cowboy Bebop".into(),
                kind: "animeGroup".into(),
                metadata_provider: None,
                metadata_id: None,
                overview: None,
                images: vec![],
            },
        )
        .await
        .unwrap();
        add_member(&mut conn, c, Member::Series(s), 1, Some("season"), None)
            .await
            .unwrap();
        add_member(
            &mut conn,
            c,
            Member::Movie(m),
            2,
            Some("movie"),
            Some("Film 01"),
        )
        .await
        .unwrap();
        add_member(&mut conn, c, Member::Movie(m), 3, None, None)
            .await
            .unwrap(); // ignored duplicate
        assert_eq!(
            find_by_title(&mut conn, "Cowboy Bebop", "animeGroup")
                .await
                .unwrap(),
            Some(c)
        );
        drop(conn);
        let all = list(&db).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].series_count, all[0].movie_count), (1, 1));
        assert_eq!(
            all[0].poster_url.as_deref(),
            Some("https://img/p.jpg"),
            "borrowed from the first member"
        );
    }
}
