//! Delay profiles, release profiles, remote path mappings and import list exclusions.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteExecutor};

use crate::{DbError, DbResult, from_json, to_json};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelayProfile {
    pub id: i64,
    pub enable_usenet: bool,
    pub enable_torrent: bool,
    /// `usenet` or `torrent`.
    pub preferred_protocol: String,
    pub usenet_delay: i64,
    pub torrent_delay: i64,
    pub bypass_if_highest_quality: bool,
    pub bypass_if_above_custom_format_score: bool,
    pub minimum_custom_format_score: i64,
    pub sort_order: i64,
    pub tags: Vec<i64>,
}

#[derive(FromRow)]
struct DelayRow {
    id: i64,
    enable_usenet: bool,
    enable_torrent: bool,
    preferred_protocol: String,
    usenet_delay: i64,
    torrent_delay: i64,
    bypass_if_highest_quality: bool,
    bypass_if_above_custom_format_score: bool,
    minimum_custom_format_score: i64,
    sort_order: i64,
    tags: String,
}

pub async fn list_delay_profiles(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<DelayProfile>> {
    let rows: Vec<DelayRow> = sqlx::query_as("SELECT * FROM delay_profiles ORDER BY sort_order")
        .fetch_all(ex)
        .await?;
    rows.into_iter()
        .map(|r| {
            Ok(DelayProfile {
                id: r.id,
                enable_usenet: r.enable_usenet,
                enable_torrent: r.enable_torrent,
                preferred_protocol: r.preferred_protocol,
                usenet_delay: r.usenet_delay,
                torrent_delay: r.torrent_delay,
                bypass_if_highest_quality: r.bypass_if_highest_quality,
                bypass_if_above_custom_format_score: r.bypass_if_above_custom_format_score,
                minimum_custom_format_score: r.minimum_custom_format_score,
                sort_order: r.sort_order,
                tags: from_json(&r.tags)?,
            })
        })
        .collect()
}

pub async fn create_delay_profile(ex: impl SqliteExecutor<'_>, d: &DelayProfile) -> DbResult<i64> {
    if d.preferred_protocol != "usenet" && d.preferred_protocol != "torrent" {
        return Err(DbError::Invalid(
            "preferred protocol must be usenet or torrent".into(),
        ));
    }
    sqlx::query_scalar(
        "INSERT INTO delay_profiles (enable_usenet, enable_torrent, preferred_protocol, usenet_delay, torrent_delay, \
         bypass_if_highest_quality, bypass_if_above_custom_format_score, minimum_custom_format_score, sort_order, tags) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(d.enable_usenet)
    .bind(d.enable_torrent)
    .bind(&d.preferred_protocol)
    .bind(d.usenet_delay)
    .bind(d.torrent_delay)
    .bind(d.bypass_if_highest_quality)
    .bind(d.bypass_if_above_custom_format_score)
    .bind(d.minimum_custom_format_score)
    .bind(d.sort_order)
    .bind(to_json(&d.tags)?)
    .fetch_one(ex)
    .await
    .map_err(|e| DbError::from_write(e, "delay profile"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfile {
    pub id: i64,
    pub name: Option<String>,
    pub enabled: bool,
    pub required: Vec<String>,
    pub ignored: Vec<String>,
    /// Empty = applies to every indexer.
    pub indexer_ids: Vec<i64>,
    pub tags: Vec<i64>,
}

#[derive(FromRow)]
struct ReleaseRow {
    id: i64,
    name: Option<String>,
    enabled: bool,
    required: String,
    ignored: String,
    indexer_ids: String,
    tags: String,
}

pub async fn list_release_profiles(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<ReleaseProfile>> {
    let rows: Vec<ReleaseRow> = sqlx::query_as("SELECT * FROM release_profiles ORDER BY id")
        .fetch_all(ex)
        .await?;
    rows.into_iter()
        .map(|r| {
            Ok(ReleaseProfile {
                id: r.id,
                name: r.name,
                enabled: r.enabled,
                required: from_json(&r.required)?,
                ignored: from_json(&r.ignored)?,
                indexer_ids: from_json(&r.indexer_ids)?,
                tags: from_json(&r.tags)?,
            })
        })
        .collect()
}

pub async fn create_release_profile(
    ex: impl SqliteExecutor<'_>,
    r: &ReleaseProfile,
) -> DbResult<i64> {
    sqlx::query_scalar(
        "INSERT INTO release_profiles (name, enabled, required, ignored, indexer_ids, tags) VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&r.name)
    .bind(r.enabled)
    .bind(to_json(&r.required)?)
    .bind(to_json(&r.ignored)?)
    .bind(to_json(&r.indexer_ids)?)
    .bind(to_json(&r.tags)?)
    .fetch_one(ex)
    .await
    .map_err(|e| DbError::from_write(e, "release profile"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RemotePathMapping {
    pub id: i64,
    pub host: String,
    pub remote_path: String,
    pub local_path: String,
}

pub async fn list_remote_path_mappings(
    ex: impl SqliteExecutor<'_>,
) -> DbResult<Vec<RemotePathMapping>> {
    Ok(
        sqlx::query_as("SELECT * FROM remote_path_mappings ORDER BY host, remote_path")
            .fetch_all(ex)
            .await?,
    )
}

pub async fn create_remote_path_mapping(
    ex: impl SqliteExecutor<'_>,
    host: &str,
    remote: &str,
    local: &str,
) -> DbResult<i64> {
    if host.trim().is_empty() || remote.trim().is_empty() || local.trim().is_empty() {
        return Err(DbError::Invalid(
            "host, remote path and local path are required".into(),
        ));
    }
    sqlx::query_scalar("INSERT INTO remote_path_mappings (host, remote_path, local_path) VALUES (?, ?, ?) RETURNING id")
        .bind(host.trim())
        .bind(remote.trim())
        .bind(local.trim())
        .fetch_one(ex)
        .await
        .map_err(|e| DbError::from_write(e, "remote path mapping"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ImportListExclusion {
    pub id: i64,
    /// Catalogue the identifier belongs to (`tvdb`, `aniList`, ...).
    pub provider: String,
    pub external_id: String,
    pub title: Option<String>,
}

pub async fn list_exclusions(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<ImportListExclusion>> {
    Ok(
        sqlx::query_as("SELECT * FROM import_list_exclusions ORDER BY title COLLATE NOCASE")
            .fetch_all(ex)
            .await?,
    )
}

/// Inserts an exclusion; returns the existing id if the same identity is already excluded.
pub async fn upsert_exclusion(
    conn: &mut sqlx::SqliteConnection,
    provider: &str,
    external_id: &str,
    title: Option<&str>,
) -> DbResult<i64> {
    if let Some(id) = sqlx::query_scalar(
        "SELECT id FROM import_list_exclusions WHERE provider = ? AND external_id = ?",
    )
    .bind(provider)
    .bind(external_id)
    .fetch_optional(&mut *conn)
    .await?
    {
        return Ok(id);
    }
    Ok(sqlx::query_scalar("INSERT INTO import_list_exclusions (provider, external_id, title) VALUES (?, ?, ?) RETURNING id")
        .bind(provider)
        .bind(external_id)
        .bind(title)
        .fetch_one(&mut *conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rules_round_trip() {
        let db = crate::test_support::db().await;
        create_delay_profile(
            db.pool(),
            &DelayProfile {
                id: 0,
                enable_usenet: true,
                enable_torrent: true,
                preferred_protocol: "torrent".into(),
                usenet_delay: 60,
                torrent_delay: 120,
                bypass_if_highest_quality: true,
                bypass_if_above_custom_format_score: false,
                minimum_custom_format_score: 0,
                sort_order: 1,
                tags: vec![],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            list_delay_profiles(db.pool()).await.unwrap()[0].torrent_delay,
            120
        );
        create_release_profile(
            db.pool(),
            &ReleaseProfile {
                id: 0,
                name: Some("No CAM".into()),
                enabled: true,
                required: vec![],
                ignored: vec!["CAM".into()],
                indexer_ids: vec![],
                tags: vec![],
            },
        )
        .await
        .unwrap();
        assert_eq!(
            list_release_profiles(db.pool()).await.unwrap()[0].ignored,
            vec!["CAM"]
        );
        create_remote_path_mapping(db.pool(), "qbittorrent", "/downloads/", "/media/downloads/")
            .await
            .unwrap();
        assert!(
            create_remote_path_mapping(db.pool(), "qbittorrent", "/downloads/", "/x")
                .await
                .is_err()
        );
        let mut conn = db.pool().acquire().await.unwrap();
        let a = upsert_exclusion(&mut conn, "tvdb", "73255", Some("House"))
            .await
            .unwrap();
        let b = upsert_exclusion(&mut conn, "tvdb", "73255", None)
            .await
            .unwrap();
        assert_eq!(a, b);
    }
}
