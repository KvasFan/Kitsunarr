//! Tags. Media links live in join tables; provider tags are JSON arrays that are cleaned up
//! when a tag is deleted.

use kitsunarr_domain::api::settings::TagItem;
use sqlx::{FromRow, SqliteExecutor};

use crate::{Database, DbError, DbResult};

#[derive(Debug, Clone, FromRow)]
struct TagRow {
    id: i64,
    label: String,
    usage_count: i64,
}

const PROVIDER_TABLES: &[&str] = &[
    "indexers",
    "download_clients",
    "notifications",
    "import_lists",
    "delay_profiles",
    "release_profiles",
    "auto_tags",
];

pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<TagItem>> {
    let rows: Vec<TagRow> = sqlx::query_as(
        "SELECT t.id, t.label, \
           (SELECT COUNT(*) FROM series_tags st WHERE st.tag_id = t.id) + \
           (SELECT COUNT(*) FROM movie_tags mt WHERE mt.tag_id = t.id) + \
           (SELECT COUNT(*) FROM indexers x, json_each(x.tags) j WHERE j.value = t.id) + \
           (SELECT COUNT(*) FROM download_clients x, json_each(x.tags) j WHERE j.value = t.id) + \
           (SELECT COUNT(*) FROM notifications x, json_each(x.tags) j WHERE j.value = t.id) + \
           (SELECT COUNT(*) FROM import_lists x, json_each(x.tags) j WHERE j.value = t.id) + \
           (SELECT COUNT(*) FROM delay_profiles x, json_each(x.tags) j WHERE j.value = t.id) + \
           (SELECT COUNT(*) FROM release_profiles x, json_each(x.tags) j WHERE j.value = t.id) + \
           (SELECT COUNT(*) FROM auto_tags x, json_each(x.tags) j WHERE j.value = t.id) AS usage_count \
         FROM tags t ORDER BY t.label COLLATE NOCASE",
    )
    .fetch_all(ex)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TagItem {
            id: r.id,
            label: r.label,
            usage_count: r.usage_count,
        })
        .collect())
}

/// Normalises a label (trimmed, lower-case, inner whitespace collapsed to `-`).
pub fn normalize_label(label: &str) -> Result<String, DbError> {
    let label = label
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if label.is_empty() || label.chars().count() > 64 {
        return Err(DbError::Invalid("tag label must be 1-64 characters".into()));
    }
    Ok(label)
}

pub async fn create(ex: impl SqliteExecutor<'_>, label: &str) -> DbResult<i64> {
    let label = normalize_label(label)?;
    sqlx::query_scalar("INSERT INTO tags (label) VALUES (?) RETURNING id")
        .bind(&label)
        .fetch_one(ex)
        .await
        .map_err(|e| DbError::from_write(e, "tag"))
}

/// Returns the id of the tag with `label`, creating it if needed.
pub async fn get_or_create(db: &Database, label: &str) -> DbResult<i64> {
    let label = normalize_label(label)?;
    if let Some(id) = sqlx::query_scalar("SELECT id FROM tags WHERE label = ?")
        .bind(&label)
        .fetch_optional(db.pool())
        .await?
    {
        return Ok(id);
    }
    create(db.pool(), &label).await
}

/// Renames a tag (Sonarr's `PUT /tag/{id}`); its links are kept.
pub async fn rename(ex: impl SqliteExecutor<'_>, id: i64, label: &str) -> DbResult<()> {
    let label = normalize_label(label)?;
    let r = sqlx::query("UPDATE tags SET label = ? WHERE id = ?")
        .bind(&label)
        .bind(id)
        .execute(ex)
        .await
        .map_err(|e| DbError::from_write(e, "tag"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// What uses a tag (Sonarr's `TagDetails`), by kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagUsage {
    pub series: Vec<i64>,
    pub movies: Vec<i64>,
    pub indexers: Vec<i64>,
    pub download_clients: Vec<i64>,
    pub notifications: Vec<i64>,
    pub import_lists: Vec<i64>,
    pub delay_profiles: Vec<i64>,
    pub release_profiles: Vec<i64>,
    pub auto_tags: Vec<i64>,
}

pub async fn usage(db: &Database, id: i64) -> DbResult<TagUsage> {
    let ids = |sql: &'static str| async move {
        sqlx::query_scalar::<_, i64>(sql)
            .bind(id)
            .fetch_all(db.pool())
            .await
    };
    let provider = |table: &'static str| async move {
        // Table names come from the static list, never from input.
        let sql = format!(
            "SELECT x.id FROM {table} x WHERE EXISTS (SELECT 1 FROM json_each(x.tags) j WHERE j.value = ?) ORDER BY x.id"
        );
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_all(db.pool())
            .await
    };
    Ok(TagUsage {
        series: ids("SELECT series_id FROM series_tags WHERE tag_id = ? ORDER BY series_id")
            .await?,
        movies: ids("SELECT movie_id FROM movie_tags WHERE tag_id = ? ORDER BY movie_id").await?,
        indexers: provider("indexers").await?,
        download_clients: provider("download_clients").await?,
        notifications: provider("notifications").await?,
        import_lists: provider("import_lists").await?,
        delay_profiles: provider("delay_profiles").await?,
        release_profiles: provider("release_profiles").await?,
        auto_tags: provider("auto_tags").await?,
    })
}

pub async fn delete(db: &Database, id: i64) -> DbResult<()> {
    let mut tx = db.pool().begin().await?;
    for table in PROVIDER_TABLES {
        // Table names come from the static list above, never from input.
        let sql = format!(
            "UPDATE {table} SET tags = (SELECT COALESCE(json_group_array(j.value), '[]') FROM json_each({table}.tags) j WHERE j.value <> ?1) \
             WHERE EXISTS (SELECT 1 FROM json_each({table}.tags) j WHERE j.value = ?1)"
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let r = sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    tx.commit().await?;
    Ok(())
}

pub async fn existing_ids(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM tags")
        .fetch_all(ex)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tags_are_renamed_and_their_usage_listed() {
        let db = crate::test_support::db().await;
        let a = create(db.pool(), "old").await.unwrap();
        rename(db.pool(), a, " New  Name ").await.unwrap();
        assert!(
            list(db.pool())
                .await
                .unwrap()
                .iter()
                .any(|t| t.id == a && t.label == "new-name")
        );
        assert!(matches!(
            rename(db.pool(), 9999, "x").await,
            Err(DbError::NotFound)
        ));
        let b = create(db.pool(), "other").await.unwrap();
        assert!(rename(db.pool(), b, "new-name").await.is_err());
        sqlx::query("INSERT INTO delay_profiles (enable_usenet, enable_torrent, preferred_protocol, usenet_delay, torrent_delay, bypass_if_highest_quality, bypass_if_above_custom_format_score, minimum_custom_format_score, sort_order, tags) VALUES (1, 1, 'usenet', 0, 0, 0, 0, 0, 5, ?)")
            .bind(format!("[{a}]"))
            .execute(db.pool())
            .await
            .unwrap();
        let u = usage(&db, a).await.unwrap();
        assert_eq!(u.delay_profiles.len(), 1);
        assert!(u.series.is_empty() && u.indexers.is_empty());
    }

    #[tokio::test]
    async fn create_list_delete_cleans_provider_references() {
        let db = crate::test_support::db().await;
        let a = create(db.pool(), " 4K  Remux ").await.unwrap();
        let b = get_or_create(&db, "anime").await.unwrap();
        assert_eq!(get_or_create(&db, "ANIME").await.unwrap(), b);
        assert!(matches!(
            create(db.pool(), "4k remux").await,
            Err(DbError::Conflict(_))
        ));
        sqlx::query(
            "INSERT INTO indexers (name, implementation, protocol, settings, tags, created_at) VALUES ('i', 'Torznab', 'torrent', '{}', ?, 'now')",
        )
        .bind(format!("[{a},{b}]"))
        .execute(db.pool())
        .await
        .unwrap();
        let tags = list(db.pool()).await.unwrap();
        assert_eq!(tags.iter().find(|t| t.id == a).unwrap().usage_count, 1);
        assert_eq!(tags.iter().find(|t| t.id == a).unwrap().label, "4k-remux");

        delete(&db, a).await.unwrap();
        let remaining: String = sqlx::query_scalar("SELECT tags FROM indexers")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(remaining, format!("[{b}]"));
        assert!(matches!(delete(&db, a).await, Err(DbError::NotFound)));
    }
}
