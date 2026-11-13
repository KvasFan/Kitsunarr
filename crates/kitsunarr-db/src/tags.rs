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
