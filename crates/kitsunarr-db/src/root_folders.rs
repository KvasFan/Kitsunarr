//! Root folders.

use kitsunarr_domain::MediaKind;
use sqlx::{FromRow, SqliteExecutor};

use crate::{DbError, DbResult, now};

#[derive(Debug, Clone, FromRow)]
pub struct RootFolder {
    pub id: i64,
    pub path: String,
    pub media_kind: Option<String>,
    pub created_at: String,
}

impl RootFolder {
    pub fn kind(&self) -> Option<MediaKind> {
        self.media_kind.as_deref().and_then(|k| k.parse().ok())
    }
}

pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<RootFolder>> {
    Ok(sqlx::query_as("SELECT * FROM root_folders ORDER BY path")
        .fetch_all(ex)
        .await?)
}

/// Canonical form used for comparisons: trailing separators removed (except the root).
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    let stripped = trimmed.trim_end_matches(['/', '\\']);
    if stripped.is_empty() {
        trimmed.chars().take(1).collect()
    } else {
        stripped.to_string()
    }
}

pub async fn create(
    ex: impl SqliteExecutor<'_>,
    path: &str,
    kind: Option<MediaKind>,
) -> DbResult<i64> {
    let path = normalize_path(path);
    if path.is_empty() {
        return Err(DbError::Invalid("path is required".into()));
    }
    sqlx::query_scalar(
        "INSERT INTO root_folders (path, media_kind, created_at) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(&path)
    .bind(kind.map(|k| k.as_str()))
    .bind(now())
    .fetch_one(ex)
    .await
    .map_err(|e| DbError::from_write(e, "root folder"))
}

pub async fn delete(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<()> {
    let r = sqlx::query("DELETE FROM root_folders WHERE id = ?")
        .bind(id)
        .execute(ex)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_normalisation() {
        assert_eq!(normalize_path("/media/tv/"), "/media/tv");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path(" /media "), "/media");
        assert_eq!(normalize_path("D:\\TV\\"), "D:\\TV");
    }

    #[tokio::test]
    async fn create_and_reject_duplicates() {
        let db = crate::test_support::db().await;
        create(db.pool(), "/media/tv/", Some(MediaKind::Series))
            .await
            .unwrap();
        assert!(matches!(
            create(db.pool(), "/media/tv", None).await,
            Err(DbError::Conflict(_))
        ));
        let all = list(db.pool()).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind(), Some(MediaKind::Series));
    }
}
