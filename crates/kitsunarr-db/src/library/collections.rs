//! Collections: provider collections (TMDB), anime groups and manual groups.

use kitsunarr_domain::api::library::{
    CollectionDetail, CollectionEdit, CollectionItem, CollectionMember, CollectionMemberEdit,
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

/// Kinds a title belongs to at most one of (Tanukarr: one group per series).
const GROUP_KINDS: &str = "('manual', 'animeGroup')";

pub async fn detail(db: &Database, id: i64) -> DbResult<CollectionDetail> {
    let row: Option<(i64, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, title, kind, overview FROM collections WHERE id = ?")
            .bind(id)
            .fetch_optional(db.pool())
            .await?;
    let Some((id, title, kind, overview)) = row else {
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
    }
    let rows: Vec<M> = sqlx::query_as(
        "SELECT m.id, m.series_id, m.movie_id, m.position, m.role, m.label, \
           COALESCE(s.title, v.title) AS title, COALESCE(s.year, v.year) AS year, COALESCE(s.images, v.images) AS images \
         FROM collection_members m LEFT JOIN series s ON s.id = m.series_id LEFT JOIN movies v ON v.id = m.movie_id \
         WHERE m.collection_id = ? ORDER BY m.position, m.id",
    )
    .bind(id)
    .fetch_all(db.pool())
    .await?;
    Ok(CollectionDetail {
        id,
        title,
        kind,
        overview,
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
    let done = sqlx::query("DELETE FROM collections WHERE id = ?")
        .bind(id)
        .execute(db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
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
    let role = edit.role.as_deref().filter(|r| !r.is_empty());
    check_role(role)?;
    let mut tx = db.pool().begin().await?;
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
    let same_role: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) + 1 FROM collection_members WHERE collection_id = ? AND role IS ?",
    )
    .bind(collection_id)
    .bind(role)
    .fetch_one(&mut *tx)
    .await?;
    let label = edit
        .label
        .clone()
        .filter(|l| !l.trim().is_empty())
        .or_else(|| label_for(role, same_role));
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
    let role = edit.role.as_deref().filter(|r| !r.is_empty());
    check_role(role)?;
    let done = sqlx::query(
        "UPDATE collection_members SET role = COALESCE(?, role), label = COALESCE(?, label), \
         position = COALESCE(?, position) WHERE id = ? AND collection_id = ?",
    )
    .bind(role)
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
    let done = sqlx::query("DELETE FROM collection_members WHERE id = ? AND collection_id = ?")
        .bind(member_id)
        .bind(collection_id)
        .execute(db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
