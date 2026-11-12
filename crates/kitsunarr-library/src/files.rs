//! Library file operations: Sonarr's `DiskScanService` (with `MediaFileTableCleanupService`),
//! `RenameEpisodeFileService` and `MediaFileDeletionService`.
//!
//! Safety rules, as for imports: files are only renamed inside the series folder, an existing
//! destination is never overwritten, and deleted files go to the recycle bin when one is set.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use kitsunarr_db::history::{self, NewHistory};
use kitsunarr_db::library::series::{self, NewEpisodeFile};
use kitsunarr_db::{Database, quality, settings};
use kitsunarr_decision as decision;
use kitsunarr_domain::Language;
pub use kitsunarr_domain::api::library::RenamePreview;
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};
use kitsunarr_domain::quality::{QualityModel, Revision};
use kitsunarr_parser::{
    is_scene_title, parse_languages, parse_path, parse_quality, parse_release_group,
};

use crate::file_names;
use crate::import::{
    aggregate_quality, extension, is_within, remove_or_recycle, stem, video_files,
};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn relative(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// What a disk scan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// File records whose file is gone (`MissingFromDisk`).
    pub removed: usize,
    /// Files found in the series folder and added to the library.
    pub added: usize,
    /// Files whose size changed.
    pub updated: usize,
    /// Files found that could not be matched, with the reason.
    pub skipped: Vec<String>,
}

impl ScanSummary {
    pub fn describe(&self) -> String {
        let mut s = format!(
            "{} files added, {} missing files removed, {} sizes updated",
            self.added, self.removed, self.updated
        );
        if !self.skipped.is_empty() {
            s.push_str(&format!("; not matched: {}", self.skipped.join("; ")));
        }
        s
    }
}

/// `DiskScanService.Scan`: forgets files that are gone, adds the video files in the series
/// folder that the library does not know (as they are, without moving them), and updates sizes.
pub async fn scan_series(db: &Database, series_id: i64) -> Result<ScanSummary, String> {
    let detail = series::get_detail(db, series_id).await.map_err(err)?;
    let series_path = PathBuf::from(&detail.item.path);
    let root = Path::new(&detail.root_folder_path);
    let files: Vec<PathBuf> = if series_path.is_dir() {
        video_files(&series_path)
    } else {
        // A missing series folder means its files are gone only if the root folder is there
        // and not empty (a disconnected drive must not empty the library).
        if !root.is_dir() {
            return Err(format!(
                "Series' root folder ({}) doesn't exist.",
                root.display()
            ));
        }
        if std::fs::read_dir(root)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
        {
            return Err(format!(
                "Series' root folder ({}) is empty.",
                root.display()
            ));
        }
        Vec::new()
    };
    let on_disk: HashSet<String> = files.iter().map(|f| relative(f, &series_path)).collect();
    let mut summary = ScanSummary::default();
    let mut conn = db.pool().acquire().await.map_err(err)?;

    // `MediaFileTableCleanupService.Clean`.
    let records = series::file_records(db, series_id).await.map_err(err)?;
    let episodes = series::list_episodes(db, series_id, None)
        .await
        .map_err(err)?;
    for r in &records {
        if on_disk.contains(&r.relative_path.replace('\\', "/")) {
            continue;
        }
        let linked: Vec<i64> = episodes
            .iter()
            .filter(|e| e.file.as_ref().is_some_and(|f| f.id == r.id))
            .map(|e| e.id)
            .collect();
        record_deletion(
            &mut conn,
            series_id,
            &linked,
            r,
            &detail.item.path,
            "MissingFromDisk",
        )
        .await?;
        series::delete_episode_file(&mut conn, r.id)
            .await
            .map_err(err)?;
        summary.removed += 1;
    }

    // Sizes of the files still there.
    for r in records
        .iter()
        .filter(|r| on_disk.contains(&r.relative_path.replace('\\', "/")))
    {
        let size = std::fs::metadata(series_path.join(&r.relative_path))
            .map(|m| m.len() as i64)
            .unwrap_or(r.size);
        if size != r.size {
            series::set_file_size(&mut conn, r.id, size)
                .await
                .map_err(err)?;
            summary.updated += 1;
        }
    }

    // Unknown files: matched like an import (`ImportDecisionMaker` with `newDownload: false`).
    let known: HashSet<String> = records
        .iter()
        .map(|r| r.relative_path.replace('\\', "/"))
        .collect();
    let unmapped: Vec<&PathBuf> = files
        .iter()
        .filter(|f| !known.contains(&relative(f, &series_path)))
        .collect();
    if !unmapped.is_empty() {
        let episodes = series::list_episodes(db, series_id, None)
            .await
            .map_err(err)?;
        let model = crate::decision_input::series_model(db, &detail, &episodes)
            .await
            .map_err(err)?;
        let original = model.original_language.map(|l| l.id).unwrap_or(0);
        let mut taken: HashSet<i64> = model
            .episodes
            .iter()
            .filter(|e| e.file.is_some())
            .map(|e| e.id)
            .collect();
        for path in unmapped {
            let rel = relative(path, &series_path);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Some(info) = parse_path(&path.to_string_lossy()) else {
                summary.skipped.push(format!("{rel}: unable to parse"));
                continue;
            };
            let mapped = decision::map_episodes(&info, &model);
            if mapped.is_empty() {
                summary
                    .skipped
                    .push(format!("{rel}: invalid season or episode"));
                continue;
            }
            if mapped.iter().any(|e| taken.contains(&e.id)) {
                summary
                    .skipped
                    .push(format!("{rel}: the episode already has a file"));
                continue;
            }
            let ids: Vec<i64> = mapped.iter().map(|e| e.id).collect();
            let quality = aggregate_quality(&[parse_quality(&name)]);
            let found = parse_languages(&name);
            let languages: Vec<i32> = if found.is_empty() || (found.len() == 1 && found[0].id == 0)
            {
                vec![original]
            } else {
                found.iter().map(|l| l.id).collect()
            };
            let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
            let file_id = series::upsert_episode_file(
                &mut conn,
                &NewEpisodeFile {
                    series_id,
                    season_number: mapped[0].season_number,
                    relative_path: rel.clone(),
                    size,
                    added_at: now(),
                    scene_name: is_scene_title(&stem(path)).then(|| stem(path)),
                    release_group: parse_release_group(&name),
                    quality_id: quality.quality_id,
                    revision_version: quality.revision.version,
                    revision_real: quality.revision.real,
                    is_repack: quality.revision.is_repack,
                    languages,
                    media_info: None,
                    original_path: None,
                    indexer_flags: 0,
                    release_type: 0,
                },
            )
            .await
            .map_err(err)?;
            series::link_episodes_to_file(&mut conn, &ids, file_id)
                .await
                .map_err(err)?;
            taken.extend(ids);
            summary.added += 1;
        }
    }
    sqlx::query("UPDATE series SET last_disk_scan_at = ? WHERE id = ?")
        .bind(now())
        .bind(series_id)
        .execute(&mut *conn)
        .await
        .ok();
    tracing::info!(series_id, summary = %summary.describe(), "disk scan");
    Ok(summary)
}

/// "episodeFileDeleted" history, one entry per episode, with upstream's reason.
async fn record_deletion(
    conn: &mut sqlx::SqliteConnection,
    series_id: i64,
    episode_ids: &[i64],
    r: &series::FileRecord,
    series_path: &str,
    reason: &str,
) -> Result<(), String> {
    let path = format!("{}/{}", series_path.trim_end_matches('/'), r.relative_path);
    for episode_id in episode_ids {
        history::insert(
            conn,
            &NewHistory {
                event_type: "episodeFileDeleted".into(),
                date: now(),
                source_title: path.clone(),
                series_id: Some(series_id),
                episode_id: Some(*episode_id),
                movie_id: None,
                quality_id: Some(r.quality_id as i32),
                revision: serde_json::to_value(Revision {
                    version: r.revision_version as i32,
                    real: r.revision_real as i32,
                    is_repack: r.is_repack,
                })
                .ok(),
                languages: r.language_ids(),
                custom_format_score: None,
                download_id: None,
                data: serde_json::json!({
                    "reason": reason,
                    "releaseGroup": r.release_group,
                    "size": r.size.to_string(),
                }),
            },
        )
        .await
        .map_err(err)?;
    }
    Ok(())
}

/// `RenameEpisodeFileService.GetRenamePreviews`: the files of the series (or of one season)
/// whose path would change.
pub async fn rename_previews(
    db: &Database,
    series_id: i64,
    season: Option<i32>,
) -> Result<Vec<RenamePreview>, String> {
    let mut naming: NamingSettings = settings::get_or_default(db.pool(), settings::KEY_NAMING)
        .await
        .map_err(err)?;
    // Tanukarr: a manual rename always applies the naming format.
    naming.rename_episodes = true;
    let detail = series::get_detail(db, series_id).await.map_err(err)?;
    let episodes = series::list_episodes(db, series_id, None)
        .await
        .map_err(err)?;
    let model = crate::decision_input::series_model(db, &detail, &episodes)
        .await
        .map_err(err)?;
    let quality_titles: std::collections::HashMap<i32, String> =
        quality::list_definitions(db.pool())
            .await
            .map_err(err)?
            .into_iter()
            .map(|d| (d.quality_id, d.title))
            .collect();
    let (formats, _) =
        decision::compile_formats(&quality::list_custom_formats(db.pool()).await.map_err(err)?);
    let (naming_series, _) = naming_series(db, &detail).await?;
    let mut out = Vec::new();
    for r in series::file_records(db, series_id).await.map_err(err)? {
        let mut linked: Vec<&decision::Episode> = model
            .episodes
            .iter()
            .filter(|e| e.file.as_ref().is_some_and(|f| f.id == r.id))
            .collect();
        if linked.is_empty() {
            continue;
        }
        linked.sort_by_key(|e| (e.season_number, e.episode_number));
        if season.is_some_and(|s| linked[0].season_number != s) {
            continue;
        }
        let quality = QualityModel {
            quality_id: r.quality_id as i32,
            revision: Revision {
                version: r.revision_version as i32,
                real: r.revision_real as i32,
                is_repack: r.is_repack,
            },
        };
        let file_name = Path::new(&r.relative_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let languages: Vec<&'static Language> = r
            .language_ids()
            .into_iter()
            .filter_map(Language::by_id)
            .collect();
        let release_title = r
            .scene_name
            .clone()
            .unwrap_or_else(|| stem(Path::new(&r.relative_path)));
        let matched = decision::custom_format::evaluate(
            &formats,
            &decision::CustomFormatInput {
                release_title: &release_title,
                file_name: Some(&file_name),
                release_group: r.release_group.as_deref(),
                quality: quality.quality(),
                size: r.size,
                indexer_flags: r.indexer_flags as u32,
                languages: &languages,
                release_type: kitsunarr_parser::ReleaseType::Unknown,
                original_language: model.original_language,
            },
        );
        let file = file_names::File {
            id: r.id,
            scene_name: r.scene_name.clone(),
            relative_path: Some(r.relative_path.clone()),
            path: None,
            release_group: r.release_group.clone(),
            release_hash: None,
            quality,
            quality_title: quality_titles
                .get(&quality.quality_id)
                .cloned()
                .unwrap_or_else(|| {
                    kitsunarr_domain::Quality::by_id(quality.quality_id)
                        .unwrap_or_else(kitsunarr_domain::Quality::unknown)
                        .name
                        .to_string()
                }),
            custom_formats: matched
                .iter()
                .map(|f| (f.name.clone(), f.include_when_renaming))
                .collect(),
        };
        let naming_episodes: Vec<file_names::Episode> = linked
            .iter()
            .map(|e| file_names::Episode {
                season_number: e.season_number,
                episode_number: e.episode_number,
                absolute_episode_number: e.absolute_episode_number,
                title: e.title.clone().unwrap_or_default(),
                air_date: e.air_date.clone(),
            })
            .collect();
        let new_path = match file_names::build_relative_path(
            &naming_episodes,
            &naming_series,
            &file,
            &extension(Path::new(&r.relative_path)),
            detail.season_folder,
            "",
            &naming,
        ) {
            Ok(p) => p.trim_start_matches('/').to_string(),
            Err(e) => {
                tracing::warn!(file = %r.relative_path, error = %e, "cannot name the file");
                continue;
            }
        };
        if new_path != r.relative_path.replace('\\', "/") {
            out.push(RenamePreview {
                episode_file_id: r.id,
                season_number: linked[0].season_number,
                episode_numbers: linked.iter().map(|e| e.episode_number).collect(),
                existing_path: r.relative_path.clone(),
                new_path,
            });
        }
    }
    out.sort_by(|a, b| {
        (a.season_number, a.episode_numbers.first())
            .cmp(&(b.season_number, b.episode_numbers.first()))
    });
    Ok(out)
}

/// `RenameEpisodeFileService.RenameFiles`: renames the chosen files (all previewed files when
/// `file_ids` is empty). Returns how many were renamed and the problems.
pub async fn rename_files(
    db: &Database,
    series_id: i64,
    file_ids: &[i64],
) -> Result<(Vec<(i64, String, String)>, Vec<String>), String> {
    let detail = series::get_detail(db, series_id).await.map_err(err)?;
    let series_path = PathBuf::from(&detail.item.path);
    let previews: Vec<RenamePreview> = rename_previews(db, series_id, None)
        .await?
        .into_iter()
        .filter(|p| file_ids.is_empty() || file_ids.contains(&p.episode_file_id))
        .collect();
    let episodes = series::list_episodes(db, series_id, None)
        .await
        .map_err(err)?;
    let mut conn = db.pool().acquire().await.map_err(err)?;
    let (mut renamed, mut problems) = (Vec::new(), Vec::new());
    for p in previews {
        let src = series_path.join(&p.existing_path);
        let dst = series_path.join(&p.new_path);
        if !is_within(&dst, &series_path) || !is_within(&src, &series_path) {
            problems.push(format!("{}: outside the series folder", p.existing_path));
            continue;
        }
        if !src.is_file() {
            problems.push(format!("{}: the file is missing", p.existing_path));
            continue;
        }
        if dst.exists() {
            problems.push(format!(
                "{}: {} already exists",
                p.existing_path, p.new_path
            ));
            continue;
        }
        if let Some(parent) = dst.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            problems.push(format!("{}: {e}", p.existing_path));
            continue;
        }
        let moved = std::fs::rename(&src, &dst).is_ok()
            || (std::fs::copy(&src, &dst).is_ok() && std::fs::remove_file(&src).is_ok());
        if !moved {
            let _ = std::fs::remove_file(&dst).ok().filter(|_| src.is_file());
            problems.push(format!("{}: could not be renamed", p.existing_path));
            continue;
        }
        series::set_file_path(&mut conn, p.episode_file_id, &p.new_path)
            .await
            .map_err(err)?;
        // `ExtraFileRenamer`: the file's extras follow its new name.
        crate::extras::rename_for_file(db, &series_path, p.episode_file_id, &src, &dst).await;
        for e in episodes
            .iter()
            .filter(|e| e.file.as_ref().is_some_and(|f| f.id == p.episode_file_id))
        {
            history::insert(
                &mut conn,
                &NewHistory {
                    event_type: "episodeFileRenamed".into(),
                    date: now(),
                    source_title: p.existing_path.clone(),
                    series_id: Some(series_id),
                    episode_id: Some(e.id),
                    movie_id: None,
                    quality_id: None,
                    revision: None,
                    languages: Vec::new(),
                    custom_format_score: None,
                    download_id: None,
                    data: serde_json::json!({
                        "sourcePath": src.to_string_lossy(),
                        "sourceRelativePath": p.existing_path,
                        "path": dst.to_string_lossy(),
                        "relativePath": p.new_path,
                    }),
                },
            )
            .await
            .map_err(err)?;
        }
        // An emptied season folder goes away, as upstream does after renames.
        if let Some(old_dir) = src.parent()
            && old_dir != series_path
            && std::fs::read_dir(old_dir).is_ok_and(|mut d| d.next().is_none())
        {
            let _ = std::fs::remove_dir(old_dir);
        }
        renamed.push((
            p.episode_file_id,
            p.existing_path.clone(),
            p.new_path.clone(),
        ));
    }
    Ok((renamed, problems))
}

/// `MediaFileDeletionService.DeleteEpisodeFile`: the file goes to the recycle bin (or is
/// deleted), the record is removed and its episodes lose their file.
pub async fn delete_file(db: &Database, series_id: i64, file_id: i64) -> Result<(), String> {
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    let detail = series::get_detail(db, series_id).await.map_err(err)?;
    let r = series::file_records(db, series_id)
        .await
        .map_err(err)?
        .into_iter()
        .find(|f| f.id == file_id)
        .ok_or_else(|| "The file is not in this series.".to_string())?;
    let series_path = PathBuf::from(&detail.item.path);
    let path = series_path.join(&r.relative_path);
    if !is_within(&path, &series_path) {
        return Err("The file is outside the series folder.".into());
    }
    if path.is_file() {
        remove_or_recycle(&path, &media, Path::new(&detail.root_folder_path))
            .map_err(|e| e.replace("Failed to import episode, unable", "Unable"))?;
    }
    let episodes = series::list_episodes(db, series_id, None)
        .await
        .map_err(err)?;
    let linked: Vec<i64> = episodes
        .iter()
        .filter(|e| e.file.as_ref().is_some_and(|f| f.id == file_id))
        .map(|e| e.id)
        .collect();
    let mut conn = db.pool().acquire().await.map_err(err)?;
    record_deletion(
        &mut conn,
        series_id,
        &linked,
        &r,
        &detail.item.path,
        "Manual",
    )
    .await?;
    series::delete_episode_file(&mut conn, file_id)
        .await
        .map_err(err)?;
    Ok(())
}

/// `DeleteSeriesService` with "delete files": the series folder goes to the recycle bin (or is
/// deleted), only when it lies inside its root folder.
pub async fn delete_series_folder(db: &Database, series_id: i64) -> Result<(), String> {
    let media: MediaManagementSettings =
        settings::get_or_default(db.pool(), settings::KEY_MEDIA_MANAGEMENT)
            .await
            .map_err(err)?;
    let detail = series::get_detail(db, series_id).await.map_err(err)?;
    let path = PathBuf::from(&detail.item.path);
    let root = PathBuf::from(&detail.root_folder_path);
    if !path.exists() {
        return Ok(());
    }
    if root.as_os_str().is_empty() || path == root || !is_within(&path, &root) {
        return Err(
            "The series folder is not inside its root folder, so it was not deleted.".into(),
        );
    }
    match media
        .recycle_bin
        .as_deref()
        .filter(|b| !b.trim().is_empty())
    {
        None => std::fs::remove_dir_all(&path).map_err(err),
        Some(bin) => {
            let name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
            std::fs::create_dir_all(bin).map_err(err)?;
            let mut target = Path::new(bin).join(&name);
            let mut n = 2;
            while target.exists() {
                target = Path::new(bin).join(format!("{}_{n}", name.to_string_lossy()));
                n += 1;
            }
            if std::fs::rename(&path, &target).is_err() {
                copy_dir(&path, &target).map_err(err)?;
                std::fs::remove_dir_all(&path).map_err(err)?;
            }
            crate::tools::touch_all(&target);
            Ok(())
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let meta = std::fs::symlink_metadata(entry.path())?;
        if meta.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if meta.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The series as the naming formats see it, with Tanukarr's per-series season folder titles
/// overriding the catalogue's, and its naming options.
pub(crate) async fn naming_series(
    db: &Database,
    detail: &kitsunarr_domain::api::library::SeriesDetail,
) -> Result<
    (
        file_names::Series,
        kitsunarr_domain::api::library_tools::SeriesNamingOptions,
    ),
    String,
> {
    let options = series::naming_options(db, detail.item.id)
        .await
        .map_err(err)?;
    let mut season_titles: Vec<(i32, String)> = detail
        .seasons
        .iter()
        .filter(|s| !options.season_titles.contains_key(&s.season_number))
        .filter_map(|s| s.title.clone().map(|t| (s.season_number, t)))
        .collect();
    season_titles.extend(options.season_titles.iter().map(|(n, t)| (*n, t.clone())));
    Ok((
        file_names::Series {
            season_titles,
            ..file_names::Series::from_parts(
                &detail.item.title,
                detail.item.year,
                detail.item.series_type,
                &detail.external_ids,
            )
        },
        options,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_read_well() {
        let s = ScanSummary {
            removed: 1,
            added: 2,
            updated: 0,
            skipped: vec!["x.mkv: unable to parse".into()],
        };
        assert_eq!(
            s.describe(),
            "2 files added, 1 missing files removed, 0 sizes updated; not matched: x.mkv: unable to parse"
        );
    }
}
