//! Key/value settings stored as JSON.

use serde::{Serialize, de::DeserializeOwned};
use sqlx::SqliteExecutor;

use crate::{DbResult, from_json, now, to_json};

pub const KEY_API_KEY: &str = "auth.apiKey";
pub const KEY_NAMING: &str = "naming";
pub const KEY_MEDIA_MANAGEMENT: &str = "mediaManagement";
pub const KEY_GENERAL: &str = "general";
pub const KEY_METADATA_SOURCES: &str = "metadataSources";
pub const KEY_INDEXER_OPTIONS: &str = "indexerOptions";
pub const KEY_DOWNLOAD_CLIENT_OPTIONS: &str = "downloadClientOptions";
pub const KEY_IMPORT_LIST_OPTIONS: &str = "importListOptions";

pub async fn get<T: DeserializeOwned>(
    ex: impl SqliteExecutor<'_>,
    key: &str,
) -> DbResult<Option<T>> {
    let raw: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(ex)
        .await?;
    raw.map(|r| from_json(&r)).transpose()
}

pub async fn get_or_default<T: DeserializeOwned + Default>(
    ex: impl SqliteExecutor<'_>,
    key: &str,
) -> DbResult<T> {
    Ok(get(ex, key).await?.unwrap_or_default())
}

pub async fn set<T: Serialize + ?Sized>(
    ex: impl SqliteExecutor<'_>,
    key: &str,
    value: &T,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(to_json(value)?)
    .bind(now())
    .execute(ex)
    .await?;
    Ok(())
}

/// Inserts only when the key is absent; returns whether a value was written.
pub async fn set_if_absent<T: Serialize + ?Sized>(
    ex: impl SqliteExecutor<'_>,
    key: &str,
    value: &T,
) -> DbResult<bool> {
    let r = sqlx::query("INSERT OR IGNORE INTO settings (key, value, updated_at) VALUES (?, ?, ?)")
        .bind(key)
        .bind(to_json(value)?)
        .bind(now())
        .execute(ex)
        .await?;
    Ok(r.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kitsunarr_domain::api::settings::NamingSettings;

    #[tokio::test]
    async fn round_trip() {
        let db = crate::test_support::db().await;
        let mut naming: NamingSettings = get_or_default(db.pool(), KEY_NAMING).await.unwrap();
        naming.rename_episodes = true;
        set(db.pool(), KEY_NAMING, &naming).await.unwrap();
        let back: NamingSettings = get(db.pool(), KEY_NAMING).await.unwrap().unwrap();
        assert!(back.rename_episodes);
        assert!(
            !set_if_absent(db.pool(), KEY_NAMING, &NamingSettings::default())
                .await
                .unwrap()
        );
        let still: NamingSettings = get(db.pool(), KEY_NAMING).await.unwrap().unwrap();
        assert!(still.rename_episodes, "set_if_absent must not overwrite");
    }
}
