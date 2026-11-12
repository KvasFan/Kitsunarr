//! Importing finished film downloads (Tanukarr's film path through `ImportDecisionMaker` and
//! `VerifyMovieCoverage`): the download's main video file becomes the film's file.
//!
//! The largest video file that is not a sample is taken (a film is one feature). It must be
//! better than the film's current file by the quality profile (an upgrade), it is linked,
//! copied or moved as episode imports are, named with the movie file format, and the file it
//! replaces goes to the recycle bin.

use std::path::{Path, PathBuf};

use kitsunarr_db::Database;
use kitsunarr_db::history::{self, GrabRecord, NewHistory};
use kitsunarr_db::library::movies;
use kitsunarr_decision::ProfileView;
use kitsunarr_domain::api::settings::{MediaManagementSettings, NamingSettings};
use kitsunarr_domain::naming;
use kitsunarr_domain::quality::Revision;
use kitsunarr_parser::{ParsedQuality, parse_languages, parse_quality, parse_release_group};

use crate::import::{
    FileResult, ImportedFile, Outcome, aggregate_quality_with_media, extension, is_within, stem,
    video_files,
};

/// A finished download grabbed for a film.
pub struct MovieDownload<'a> {
    pub download_id: &'a str,
    pub title: &'a str,
    pub output_path: &'a Path,
    pub movie_id: i64,
    pub grabs: &'a [GrabRecord],
    pub client_name: &'a str,
    pub client_implementation: &'a str,
    pub move_files: bool,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn is_sample(path: &Path) -> bool {
    stem(path)
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == "sample")
}

/// Imports a finished film download.
pub async fn import_movie_download(
    db: &Database,
    d: &MovieDownload<'_>,
) -> Result<Outcome, String> {
    let detail = movies::get_detail(db, d.movie_id).await.map_err(err)?;
    let movie_path = PathBuf::from(&detail.item.path);
    if !Path::new(&detail.root_folder_path).is_dir() {
        return Ok(blocked("Failed to import movie, Root folder missing."));
    }
    if !d.output_path.exists() {
        return Ok(blocked(&format!(
            "The download's path {} does not exist.",
            d.output_path.display()
        )));
    }
    let files: Vec<PathBuf> = if d.output_path.is_dir() {
        video_files(d.output_path)
    } else {
        vec![d.output_path.to_path_buf()]
    };
    let Some(source) = files
        .iter()
        .filter(|p| !is_sample(p))
        .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .cloned()
    else {
        return Ok(Outcome {
            results: Vec::new(),
            blocked: None,
        });
    };
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let grab = d.grabs.first();
    let grab_title = grab.map(|g| g.source_title.clone()).unwrap_or_default();
    let folder_name = (d.output_path.is_dir())
        .then(|| {
            d.output_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .flatten();
    let media_info = crate::media_info::reader::read(&source).await;
    let mut quality = aggregate_quality_with_media(
        &[
            parse_quality(&name),
            folder_name
                .as_deref()
                .map(parse_quality)
                .unwrap_or_else(ParsedQuality::unknown),
            parse_quality(d.title),
            parse_quality(&grab_title),
        ],
        media_info.as_ref(),
    );
    // The grab's quality wins when the names say nothing (`AggregateQuality` from history).
    if quality.quality_id == 0
        && let Some(q) = grab.and_then(|g| g.quality_id)
    {
        quality.quality_id = q as i32;
        quality.revision = grab
            .and_then(|g| g.revision.as_deref())
            .and_then(|r| serde_json::from_str::<Revision>(r).ok())
            .unwrap_or_default();
    }
    let profile = kitsunarr_db::quality::get_profile(db.pool(), detail.item.quality_profile_id)
        .await
        .map_err(err)?;
    let view = ProfileView(&profile);
    let mut result = FileResult {
        path: source.clone(),
        imported: false,
        episode_ids: Vec::new(),
        errors: Vec::new(),
        imported_file: None,
    };
    // `DetectSample`: measured shorter than the film's minimum.
    if media_info
        .as_ref()
        .is_some_and(|m| m.is_sample(detail.runtime.unwrap_or(0)))
    {
        result.errors.push("Sample".into());
        return Ok(Outcome {
            results: vec![result],
            blocked: None,
        });
    }
    // `UpgradeSpecification`: a file of the film must be improved on.
    if let Some(f) = &detail.file {
        let order = view.compare_qualities(quality.quality_id, f.quality_id);
        let existing: Option<(i64, i64, bool)> = sqlx::query_as(
            "SELECT revision_version, revision_real, is_repack FROM movie_files WHERE id = ?",
        )
        .bind(f.id)
        .fetch_optional(db.pool())
        .await
        .map_err(err)?;
        let existing_revision = existing.map_or_else(Revision::default, |(v, r, p)| Revision {
            version: v as i32,
            real: r as i32,
            is_repack: p,
        });
        let better = order == std::cmp::Ordering::Greater
            || (order == std::cmp::Ordering::Equal
                && kitsunarr_decision::profile::compare_revision(
                    &quality.revision,
                    &existing_revision,
                ) == std::cmp::Ordering::Greater);
        if !better {
            result
                .errors
                .push("Not an upgrade for existing movie file(s)".into());
            return Ok(Outcome {
                results: vec![result],
                blocked: None,
            });
        }
    }
    let naming_settings: NamingSettings =
        kitsunarr_db::settings::get_or_default(db.pool(), kitsunarr_db::settings::KEY_NAMING)
            .await
            .map_err(err)?;
    let media: MediaManagementSettings = kitsunarr_db::settings::get_or_default(
        db.pool(),
        kitsunarr_db::settings::KEY_MEDIA_MANAGEMENT,
    )
    .await
    .map_err(err)?;
    let release_group = parse_release_group(&grab_title)
        .or_else(|| parse_release_group(d.title))
        .or_else(|| parse_release_group(&name));
    let quality_name = crate::notify::quality_name(i64::from(quality.quality_id));
    let quality_full = if quality.revision.version > 1 {
        format!("{quality_name} Proper")
    } else {
        quality_name.clone()
    };
    let base = if naming_settings.rename_episodes {
        naming::movie_file_name(
            &naming_settings.movie_file_format,
            &detail.item.title,
            detail.item.year,
            &quality_full,
            release_group.as_deref(),
            naming_settings.colon_replacement_format,
            &naming_settings.custom_colon_replacement_format,
        )
    } else {
        stem(&source)
    };
    let relative = format!("{base}{}", extension(&source));
    let dest = movie_path.join(&relative);
    if !is_within(&dest, &movie_path) {
        result
            .errors
            .push("Failed to import movie, the destination is outside the movie folder.".into());
        return Ok(Outcome {
            results: vec![result],
            blocked: None,
        });
    }
    let old_relative = detail.file.as_ref().map(|f| f.relative_path.clone());
    if dest.exists() && old_relative.as_deref() != Some(relative.as_str()) {
        result
            .errors
            .push("Failed to import movie, Destination already exists.".into());
        return Ok(Outcome {
            results: vec![result],
            blocked: None,
        });
    }
    std::fs::create_dir_all(&movie_path).map_err(err)?;
    // The new file goes in next to the old one first, so a failed transfer loses nothing.
    let staging = movie_path.join(format!(
        ".kitsunarr-import-{}",
        uuid::Uuid::new_v4().simple()
    ));
    if let Err(e) = transfer(db, &media, &source, &staging, d.move_files).await {
        let _ = std::fs::remove_file(&staging);
        result.errors.push(e);
        return Ok(Outcome {
            results: vec![result],
            blocked: None,
        });
    }
    let mut replaced = Vec::new();
    if let Some(old) = &old_relative {
        let old_path = movie_path.join(old);
        if old_path.exists() {
            let root = PathBuf::from(&detail.root_folder_path);
            if let Err(e) = crate::import::remove_or_recycle(&old_path, &media, &root) {
                let _ = std::fs::remove_file(&staging);
                result.errors.push(e);
                return Ok(Outcome {
                    results: vec![result],
                    blocked: None,
                });
            }
        }
        replaced.push(old.clone());
    }
    std::fs::rename(&staging, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        format!("Failed to import movie: {e}")
    })?;
    let size = std::fs::metadata(&dest)
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let languages: Vec<i32> = parse_languages(&grab_title)
        .iter()
        .chain(parse_languages(&name).iter())
        .map(|l| l.id)
        .take(1)
        .collect();
    let now = kitsunarr_db::now();
    let mut conn = db.pool().acquire().await.map_err(err)?;
    let file_id = movies::upsert_file(
        &mut conn,
        &movies::NewMovieFile {
            movie_id: d.movie_id,
            relative_path: relative.clone(),
            size,
            added_at: now.clone(),
            scene_name: Some(crate::import::clean_file_name(d.title)).filter(|t| !t.is_empty()),
            release_group,
            quality_id: quality.quality_id,
            revision_version: quality.revision.version,
            revision_real: quality.revision.real,
            is_repack: quality.revision.is_repack,
            languages: if languages.is_empty() {
                vec![1]
            } else {
                languages.clone()
            },
            media_info: media_info
                .as_ref()
                .and_then(|m| serde_json::to_value(m).ok()),
            original_path: Some(source.display().to_string()),
            edition: None,
        },
    )
    .await
    .map_err(err)?;
    let data = serde_json::json!({
        "fileId": file_id,
        "droppedPath": source.display().to_string(),
        "importedPath": dest.display().to_string(),
        "downloadClient": d.client_implementation,
        "downloadClientName": d.client_name,
        "releaseGroup": grab.and_then(|g| serde_json::from_str::<serde_json::Value>(&g.data).ok()).and_then(|v| v["releaseGroup"].as_str().map(str::to_string)),
    });
    history::insert(
        &mut conn,
        &NewHistory {
            event_type: "downloadFolderImported".into(),
            date: now,
            source_title: d.title.to_string(),
            series_id: None,
            episode_id: None,
            movie_id: Some(d.movie_id),
            quality_id: Some(quality.quality_id),
            revision: serde_json::to_value(quality.revision).ok(),
            languages,
            custom_format_score: None,
            download_id: Some(d.download_id.to_string()),
            data,
        },
    )
    .await
    .map_err(err)?;
    result.imported = true;
    result.imported_file = Some(ImportedFile {
        file_id,
        source_path: source.display().to_string(),
        replaced,
    });
    Ok(Outcome {
        results: vec![result],
        blocked: None,
    })
}

fn blocked(message: &str) -> Outcome {
    Outcome {
        results: Vec::new(),
        blocked: Some(message.to_string()),
    }
}

/// Moves (Usenet) or links/copies (torrents, which keep seeding) the file, as episode imports
/// do; Tanukarr's hardlink-only setting never copies.
async fn transfer(
    db: &Database,
    media: &MediaManagementSettings,
    from: &Path,
    to: &Path,
    move_files: bool,
) -> Result<(), String> {
    let tools: kitsunarr_domain::api::settings::LibraryToolsSettings =
        kitsunarr_db::settings::get_or_default(
            db.pool(),
            kitsunarr_db::settings::KEY_LIBRARY_TOOLS,
        )
        .await
        .unwrap_or_default();
    if tools.hardlink_only && !move_files {
        return std::fs::hard_link(from, to).map_err(|e| {
            format!("Hardlink-only import cannot link this download to the library ({e}). Existing files have been kept.")
        });
    }
    if move_files && std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if !move_files && media.use_hardlinks && std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| format!("Failed to import movie: {e}"))?;
    if move_files {
        let _ = std::fs::remove_file(from);
    }
    Ok(())
}
