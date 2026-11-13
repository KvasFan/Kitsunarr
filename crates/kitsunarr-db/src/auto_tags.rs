//! Auto tagging rules (Sonarr's `AutoTagging` table) and their application to titles.
//!
//! As in Sonarr (`SeriesService.UpdateAutoTaggingTags`), rules are applied when a title is
//! added, edited (alone or in bulk) and refreshed; saving a rule does not retag the library.

use kitsunarr_domain::api::settings::{AutoTag, CustomFormatSpecification};
use kitsunarr_domain::auto_tagging::{
    self, AutoTagSubject, movie_status_value, series_status_value, series_type_value,
};
use kitsunarr_domain::{MetadataProvider, MovieStatus, SeriesStatus, SeriesType};
use sqlx::{FromRow, SqliteConnection, SqliteExecutor};

use crate::{DbError, DbResult, from_json, to_json};

#[derive(FromRow)]
struct Row {
    id: i64,
    name: String,
    remove_tags_automatically: bool,
    specifications: String,
    tags: String,
}

pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<AutoTag>> {
    let rows: Vec<Row> = sqlx::query_as("SELECT * FROM auto_tags ORDER BY name COLLATE NOCASE")
        .fetch_all(ex)
        .await?;
    rows.into_iter()
        .map(|r| {
            Ok(AutoTag {
                id: r.id,
                name: r.name,
                remove_tags_automatically: r.remove_tags_automatically,
                specifications: from_json::<Vec<CustomFormatSpecification>>(&r.specifications)?,
                tags: from_json(&r.tags)?,
            })
        })
        .collect()
}

fn checked(rule: &AutoTag) -> DbResult<()> {
    auto_tagging::validate(rule).map_err(DbError::Invalid)
}

pub async fn create(ex: impl SqliteExecutor<'_>, rule: &AutoTag) -> DbResult<i64> {
    checked(rule)?;
    sqlx::query_scalar(
        "INSERT INTO auto_tags (name, remove_tags_automatically, specifications, tags) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(rule.name.trim())
    .bind(rule.remove_tags_automatically)
    .bind(to_json(&rule.specifications)?)
    .bind(to_json(&rule.tags)?)
    .fetch_one(ex)
    .await
    .map_err(|e| DbError::from_write(e, "auto tag"))
}

pub async fn update(ex: impl SqliteExecutor<'_>, rule: &AutoTag) -> DbResult<()> {
    checked(rule)?;
    let r = sqlx::query(
        "UPDATE auto_tags SET name = ?, remove_tags_automatically = ?, specifications = ?, tags = ? WHERE id = ?",
    )
    .bind(rule.name.trim())
    .bind(rule.remove_tags_automatically)
    .bind(to_json(&rule.specifications)?)
    .bind(to_json(&rule.tags)?)
    .bind(rule.id)
    .execute(ex)
    .await
    .map_err(|e| DbError::from_write(e, "auto tag"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn delete(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<()> {
    let r = sqlx::query("DELETE FROM auto_tags WHERE id = ?")
        .bind(id)
        .execute(ex)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

#[derive(FromRow)]
struct TitleRow {
    genres: String,
    network: Option<String>,
    original_country: Option<String>,
    original_language: Option<i64>,
    quality_profile_id: i64,
    root_folder_path: String,
    kind: String,
    status: String,
    year: Option<i32>,
    monitored: bool,
    tags: String,
}

fn subject(r: TitleRow, kind: impl Fn(&str) -> i64, status: i64) -> DbResult<AutoTagSubject> {
    Ok(AutoTagSubject {
        genres: from_json(&r.genres)?,
        network: r.network,
        original_country: r.original_country,
        original_language: r.original_language,
        quality_profile_id: r.quality_profile_id,
        root_folder_path: r.root_folder_path,
        series_type: kind(&r.kind),
        status,
        tags: from_json::<Vec<Option<i64>>>(&r.tags)?
            .into_iter()
            .flatten()
            .collect(),
        year: r.year,
        monitored: r.monitored,
    })
}

/// Applies the rules to a series; true when its tags changed.
pub async fn apply_series(conn: &mut SqliteConnection, id: i64) -> DbResult<bool> {
    let rules = list(&mut *conn).await?;
    if rules.is_empty() {
        return Ok(false);
    }
    let row: Option<TitleRow> = sqlx::query_as(
        "SELECT genres, network, original_country, original_language, quality_profile_id, root_folder_path, \
           series_type AS kind, status, year, monitored, \
           (SELECT json_group_array(t.tag_id) FROM series_tags t WHERE t.series_id = s.id) AS tags \
         FROM series s WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Ok(false) };
    let status = series_status_value(
        row.status
            .parse::<SeriesStatus>()
            .ok()
            .unwrap_or(SeriesStatus::Unknown),
    );
    let subject = subject(
        row,
        |k| series_type_value(k.parse::<SeriesType>().ok().unwrap_or(SeriesType::Standard)),
        status,
    )?;
    write(conn, "series_tags", "series_id", id, &rules, subject).await
}

/// Applies the rules to a film (a Tanukarr film is a series of the anime type when it comes
/// from an anime catalogue); true when its tags changed.
pub async fn apply_movie(conn: &mut SqliteConnection, id: i64) -> DbResult<bool> {
    let rules = list(&mut *conn).await?;
    if rules.is_empty() {
        return Ok(false);
    }
    let row: Option<TitleRow> = sqlx::query_as(
        "SELECT genres, studio AS network, original_country, original_language, quality_profile_id, root_folder_path, \
           metadata_provider AS kind, status, year, monitored, \
           (SELECT json_group_array(t.tag_id) FROM movie_tags t WHERE t.movie_id = m.id) AS tags \
         FROM movies m WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Ok(false) };
    let status = movie_status_value(
        row.status
            .parse::<MovieStatus>()
            .ok()
            .unwrap_or(MovieStatus::Unknown),
    );
    let subject = subject(
        row,
        |provider| match provider.parse::<MetadataProvider>().ok() {
            Some(MetadataProvider::AniList | MetadataProvider::MyAnimeList) => {
                series_type_value(SeriesType::Anime)
            }
            _ => series_type_value(SeriesType::Standard),
        },
        status,
    )?;
    write(conn, "movie_tags", "movie_id", id, &rules, subject).await
}

async fn write(
    conn: &mut SqliteConnection,
    table: &str,
    column: &str,
    id: i64,
    rules: &[AutoTag],
    subject: AutoTagSubject,
) -> DbResult<bool> {
    let changes = auto_tagging::tag_changes(rules, &subject);
    let mut tags = subject.tags.clone();
    if !changes.apply(&mut tags) {
        return Ok(false);
    }
    // Table and column names come from the two callers above, never from input.
    let delete = format!("DELETE FROM {table} WHERE {column} = ?");
    sqlx::query(sqlx::AssertSqlSafe(delete))
        .bind(id)
        .execute(&mut *conn)
        .await?;
    // A rule may name a tag deleted since; only existing tags are linked.
    let insert = format!(
        "INSERT OR IGNORE INTO {table} ({column}, tag_id) SELECT ?, id FROM tags WHERE id = ?"
    );
    for tag in &tags {
        sqlx::query(sqlx::AssertSqlSafe(insert.clone()))
            .bind(id)
            .bind(tag)
            .execute(&mut *conn)
            .await?;
    }
    tracing::debug!(table, id, added = changes.add.len(), "auto tags applied");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{movies, series};
    use serde_json::json;

    fn rule(name: &str, implementation: &str, fields: serde_json::Value, tag: i64) -> AutoTag {
        AutoTag {
            id: 0,
            name: name.into(),
            remove_tags_automatically: true,
            specifications: vec![CustomFormatSpecification {
                name: name.into(),
                implementation: implementation.into(),
                negate: false,
                required: false,
                fields: fields.as_object().cloned().unwrap_or_default(),
            }],
            tags: vec![tag],
        }
    }

    #[tokio::test]
    async fn rules_tag_and_untag_series_and_films() {
        let db = crate::test_support::db().await;
        let anime = crate::tags::create(db.pool(), "anime").await.unwrap();
        let gb = crate::tags::create(db.pool(), "british").await.unwrap();
        let mut conn = db.pool().acquire().await.unwrap();
        let s = series::tests::sample("Show", "1");
        let sid = series::insert(&mut conn, &s).await.unwrap();
        sqlx::query(
            "UPDATE series SET original_country = 'gb', series_type = 'anime' WHERE id = ?",
        )
        .bind(sid)
        .execute(&mut *conn)
        .await
        .unwrap();
        let mid = movies::insert(&mut conn, &movies::tests::sample("Film", "2"))
            .await
            .unwrap();

        // No rules: nothing happens.
        assert!(!apply_series(&mut conn, sid).await.unwrap());

        create(
            &mut *conn,
            &rule(
                "Anime",
                "SeriesTypeSpecification",
                json!({"value": 2}),
                anime,
            ),
        )
        .await
        .unwrap();
        create(
            &mut *conn,
            &rule(
                "British",
                "OriginalCountrySpecification",
                json!({"value": ["GB"]}),
                gb,
            ),
        )
        .await
        .unwrap();
        assert!(matches!(
            create(
                &mut *conn,
                &rule("anime", "MonitoredSpecification", json!({}), gb)
            )
            .await,
            Err(DbError::Conflict(_))
        ));

        assert!(apply_series(&mut conn, sid).await.unwrap());
        assert!(!apply_series(&mut conn, sid).await.unwrap());
        let tags: Vec<i64> = sqlx::query_scalar(
            "SELECT tag_id FROM series_tags WHERE series_id = ? ORDER BY tag_id",
        )
        .bind(sid)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(tags, vec![anime, gb]);

        // The sample film comes from AniList, so it is anime (and not British).
        assert!(apply_movie(&mut conn, mid).await.unwrap());
        let tags: Vec<i64> = sqlx::query_scalar("SELECT tag_id FROM movie_tags WHERE movie_id = ?")
            .bind(mid)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        assert_eq!(tags, vec![anime]);

        // "Remove tags automatically" takes the tag away once the series stops matching.
        sqlx::query("UPDATE series SET original_country = 'us' WHERE id = ?")
            .bind(sid)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert!(apply_series(&mut conn, sid).await.unwrap());
        let tags: Vec<i64> =
            sqlx::query_scalar("SELECT tag_id FROM series_tags WHERE series_id = ?")
                .bind(sid)
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        assert_eq!(tags, vec![anime]);

        // Tags in use by a rule are counted and cleaned up with the tag.
        let listed = crate::tags::list(&mut *conn).await.unwrap();
        assert_eq!(listed.iter().find(|t| t.id == gb).unwrap().usage_count, 1);
        drop(conn);
        crate::tags::delete(&db, gb).await.unwrap();
        let rules = list(db.pool()).await.unwrap();
        assert!(
            rules
                .iter()
                .find(|r| r.name == "British")
                .unwrap()
                .tags
                .is_empty()
        );
    }
}
