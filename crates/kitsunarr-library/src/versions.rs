//! Retained episode versions: Tanukarr's `RetainedVersionService`.
//!
//! With "keep versions" on for a series, a file an upgrade replaces is moved to
//! `<series>/Plex Versions/<id>/<name>` (a folder disk scans skip) and its record is kept as a
//! JSON snapshot. Activating a version copies it back to its old place under the same file
//! record, and keeps the file it replaces as a new version; the activated archive stays.

use std::path::{Path, PathBuf};

use kitsunarr_db::Database;
use kitsunarr_domain::api::library_tools::RetainedVersion;
use serde_json::Value;

/// The folder versions live in, inside the series folder (excluded from disk scans).
pub const FOLDER: &str = "Plex Versions";

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// Keeps an episode file the import is about to replace. The row is inserted first and removed
/// again if the move fails (Tanukarr's order).
pub async fn preserve(
    db: &Database,
    series_id: i64,
    series_path: &Path,
    file_id: i64,
    episode_ids: &[i64],
) -> Result<(), String> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT relative_path, json_object('seasonNumber', season_number, 'relativePath', relative_path, 'size', size, \
         'addedAt', added_at, 'sceneName', scene_name, 'releaseGroup', release_group, 'qualityId', quality_id, \
         'revisionVersion', revision_version, 'revisionReal', revision_real, 'isRepack', is_repack, \
         'languages', json(languages), 'mediaInfo', json(media_info), 'originalPath', original_path, \
         'indexerFlags', indexer_flags, 'releaseType', release_type) FROM episode_files WHERE id = ?",
    )
    .bind(file_id)
    .fetch_optional(db.pool())
    .await
    .map_err(err)?;
    let Some((relative, metadata)) = row else {
        return Err("The replaced file has no record.".into());
    };
    let source = series_path.join(&relative);
    let name = source
        .file_name()
        .ok_or("The replaced file has no name.")?
        .to_owned();
    let archive_relative = PathBuf::from(FOLDER)
        .join(uuid::Uuid::new_v4().simple().to_string())
        .join(&name);
    let target = series_path.join(&archive_relative);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO retained_versions (series_id, relative_path, archive_path, metadata, episode_ids, created_at) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(series_id)
    .bind(&relative)
    .bind(archive_relative.to_string_lossy().replace('\\', "/"))
    .bind(&metadata)
    .bind(serde_json::to_string(episode_ids).unwrap_or_else(|_| "[]".into()))
    .bind(kitsunarr_db::now())
    .fetch_one(db.pool())
    .await
    .map_err(err)?;
    let moved = target
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| move_file(&source, &target));
    if let Err(e) = moved {
        let _ = sqlx::query("DELETE FROM retained_versions WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await;
        return Err(format!("The replaced file could not be kept: {e}"));
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    relative_path: String,
    archive_path: String,
    metadata: String,
    episode_ids: String,
    created_at: String,
}

async fn rows(db: &Database, series_id: i64) -> Result<Vec<Row>, String> {
    sqlx::query_as(
        "SELECT id, relative_path, archive_path, metadata, episode_ids, created_at FROM retained_versions \
         WHERE series_id = ? ORDER BY created_at DESC, id DESC",
    )
    .bind(series_id)
    .fetch_all(db.pool())
    .await
    .map_err(err)
}

async fn series_path(db: &Database, series_id: i64) -> Result<PathBuf, String> {
    let p: Option<String> = sqlx::query_scalar("SELECT path FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_optional(db.pool())
        .await
        .map_err(err)?;
    p.map(PathBuf::from)
        .ok_or_else(|| "The series does not exist.".into())
}

pub async fn list(db: &Database, series_id: i64) -> Result<Vec<RetainedVersion>, String> {
    let base = series_path(db, series_id).await?;
    Ok(rows(db, series_id)
        .await?
        .into_iter()
        .map(|r| {
            let m: Value = serde_json::from_str(&r.metadata).unwrap_or_default();
            RetainedVersion {
                id: r.id,
                created: r.created_at,
                available: base.join(&r.archive_path).is_file(),
                relative_path: r.relative_path,
                archive_path: r.archive_path,
                size: m["size"].as_i64().unwrap_or(0),
                quality_id: m["qualityId"].as_i64().unwrap_or(0) as i32,
                release_group: m["releaseGroup"].as_str().map(str::to_string),
                episode_ids: serde_json::from_str(&r.episode_ids).unwrap_or_default(),
            }
        })
        .collect())
}

/// Deletes a kept version and its file (Tanukarr has no way to prune them).
pub async fn delete(db: &Database, series_id: i64, version_id: i64) -> Result<(), String> {
    let base = series_path(db, series_id).await?;
    let row = rows(db, series_id)
        .await?
        .into_iter()
        .find(|r| r.id == version_id)
        .ok_or("The version does not exist.")?;
    let file = base.join(&row.archive_path);
    if file.is_file() {
        std::fs::remove_file(&file).map_err(err)?;
        if let Some(dir) = file.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
    sqlx::query("DELETE FROM retained_versions WHERE id = ?")
        .bind(version_id)
        .execute(db.pool())
        .await
        .map_err(err)?;
    Ok(())
}

/// `RetainedVersionService.Activate`: the version must cover exactly the episodes of one active
/// file; the active file becomes a new version and the archive is copied (kept) into its old
/// place under the same record.
pub async fn activate(db: &Database, series_id: i64, version_id: i64) -> Result<String, String> {
    let base = series_path(db, series_id).await?;
    let row = rows(db, series_id)
        .await?
        .into_iter()
        .find(|r| r.id == version_id)
        .ok_or("The version does not exist.")?;
    let episode_ids: Vec<i64> = serde_json::from_str(&row.episode_ids).unwrap_or_default();
    if episode_ids.is_empty() {
        return Err("The version has no episodes; use Manual Import.".into());
    }
    let linked: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, series_id, episode_file_id FROM episodes WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(&row.episode_ids)
    .fetch_all(db.pool())
    .await
    .map_err(err)?;
    if linked.len() != episode_ids.len() || linked.iter().any(|(_, s, _)| *s != series_id) {
        return Err(
            "The version's episodes are not all episodes of this series; use Manual Import.".into(),
        );
    }
    let files: std::collections::HashSet<i64> = linked.iter().filter_map(|(_, _, f)| *f).collect();
    let [active] = files.iter().copied().collect::<Vec<_>>()[..] else {
        return Err(
            "The version's episodes do not share exactly one current file; use Manual Import."
                .into(),
        );
    };
    if linked.iter().any(|(_, _, f)| f.is_none()) {
        return Err("Some of the version's episodes have no file; use Manual Import.".into());
    }
    let active_episodes: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM episodes WHERE episode_file_id = ? ORDER BY id")
            .bind(active)
            .fetch_all(db.pool())
            .await
            .map_err(err)?;
    let mut wanted = episode_ids.clone();
    wanted.sort_unstable();
    if active_episodes != wanted {
        return Err(
            "The current file covers other episodes than the version; use Manual Import.".into(),
        );
    }
    let archive = base.join(&row.archive_path);
    if !archive.is_file() {
        return Err("The kept file is missing.".into());
    }
    let active_relative: String =
        sqlx::query_scalar("SELECT relative_path FROM episode_files WHERE id = ?")
            .bind(active)
            .fetch_one(db.pool())
            .await
            .map_err(err)?;
    let destination = base.join(&row.relative_path);
    if destination.exists() && row.relative_path != active_relative {
        return Err(format!(
            "{} exists and is not the current file.",
            destination.display()
        ));
    }
    // Copy the archive to staging first; the archive itself is kept.
    let staging = base
        .join(FOLDER)
        .join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&staging).map_err(err)?;
    let staged = staging.join(archive.file_name().unwrap_or_default());
    let result = async {
        std::fs::copy(&archive, &staged).map_err(err)?;
        // The current file becomes a version.
        preserve(db, series_id, &base, active, &episode_ids).await?;
        if let Some(dir) = destination.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        move_file(&staged, &destination).map_err(err)?;
        let m: Value = serde_json::from_str(&row.metadata).unwrap_or_default();
        sqlx::query(
            "UPDATE episode_files SET relative_path = ?, size = ?, scene_name = ?, release_group = ?, quality_id = ?, \
             revision_version = ?, revision_real = ?, is_repack = ?, languages = ?, media_info = ?, original_path = ?, \
             indexer_flags = ?, release_type = ? WHERE id = ?",
        )
        .bind(&row.relative_path)
        .bind(m["size"].as_i64().unwrap_or(0))
        .bind(m["sceneName"].as_str())
        .bind(m["releaseGroup"].as_str())
        .bind(m["qualityId"].as_i64().unwrap_or(0))
        .bind(m["revisionVersion"].as_i64().unwrap_or(1))
        .bind(m["revisionReal"].as_i64().unwrap_or(0))
        .bind(m["isRepack"].as_i64().unwrap_or(0))
        .bind(m.get("languages").filter(|v| v.is_array()).map_or_else(|| "[]".to_string(), Value::to_string))
        .bind(m.get("mediaInfo").filter(|v| !v.is_null()).map(Value::to_string))
        .bind(m["originalPath"].as_str())
        .bind(m["indexerFlags"].as_i64().unwrap_or(0))
        .bind(m["releaseType"].as_i64().unwrap_or(0))
        .bind(active)
        .execute(db.pool())
        .await
        .map_err(err)?;
        Ok::<_, String>(())
    }
    .await;
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    Ok(format!("{} is the current file again.", row.relative_path))
}
