//! Collections: provider collections (TMDB), anime groups and manual groups.

use kitsunarr_domain::api::library::CollectionItem;
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
