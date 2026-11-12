//! Companion app keys (Tanukarr's `ProAccessKeys`): scoped, expiring keys for the Sonarr v3 API.

use sqlx::{FromRow, SqlitePool};

use crate::{DbError, DbResult, now};

/// At most this many keys exist.
pub const MAX_KEYS: i64 = 50;

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct CompanionKey {
    pub id: String,
    pub name: String,
    pub permission: String,
    pub created_at: String,
    pub expires_at: String,
}

pub async fn list(pool: &SqlitePool) -> DbResult<Vec<CompanionKey>> {
    Ok(sqlx::query_as(
        "SELECT id, name, permission, created_at, expires_at FROM companion_keys ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?)
}

/// Stores a key by the hash of its token; `id` is a random identifier.
pub async fn create(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    hash: &str,
    permission: &str,
    expires_at: &str,
) -> DbResult<CompanionKey> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM companion_keys")
        .fetch_one(pool)
        .await?;
    if count >= MAX_KEYS {
        return Err(DbError::Invalid(format!(
            "at most {MAX_KEYS} companion keys can exist"
        )));
    }
    sqlx::query(
        "INSERT INTO companion_keys (id, name, hash, permission, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(hash)
    .bind(permission)
    .bind(now())
    .bind(expires_at)
    .execute(pool)
    .await
    .map_err(|e| DbError::from_write(e, "companion key"))?;
    Ok(sqlx::query_as(
        "SELECT id, name, permission, created_at, expires_at FROM companion_keys WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

pub async fn delete(pool: &SqlitePool, id: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM companion_keys WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The unexpired key with this token hash.
pub async fn find_valid(pool: &SqlitePool, hash: &str) -> DbResult<Option<CompanionKey>> {
    Ok(sqlx::query_as(
        "SELECT id, name, permission, created_at, expires_at FROM companion_keys WHERE hash = ? AND expires_at > ?",
    )
    .bind(hash)
    .bind(now())
    .fetch_optional(pool)
    .await?)
}
